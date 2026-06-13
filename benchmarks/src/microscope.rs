use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, Directive, EngineCommand,
    EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled, RatchetPolicy,
    SendSingle, SendSinglePayload, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{InboundPacket, InterfaceConfig, InterfaceId};
use personal_rns::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
use personal_rns::reactor::interfaces::tcp::core as tcp_core;
use personal_rns::routing::announce::defaults::{JitterSeed, DEFAULT_REBROADCAST_JITTER_WINDOW_MS};
use personal_rns::routing::delivery::Delivery;
use personal_rns::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;

const WIRE: InterfaceId = InterfaceId::new([0xC7; 16]);
const NOW: InstantMillis = InstantMillis(1_000);
const JITTER: JitterSeed = JitterSeed(7);
pub const PAYLOAD_LEN: usize = 300;

const IF_UP: InterfaceId = InterfaceId::new([0xA1; 16]);
const IF_DOWN: InterfaceId = InterfaceId::new([0xD0; 16]);
const SETUP_NOW: InstantMillis = InstantMillis(1_000);
const REBROADCAST_NOW: InstantMillis =
    InstantMillis(1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
const FORWARD_NOW: InstantMillis = InstantMillis(2_000);

/// Deterministic entropy (splitmix64): every run pulls the identical stream, so a
/// measured difference between runs is the code, never the keys.
struct Splitmix(u64);

impl Splitmix {
    fn fill(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(8) {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut word = self.0;
            word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            word ^= word >> 31;
            chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
        }
    }
}

/// One complete SINGLE round trip held as three replayable stages over two live
/// engines. Every receipt settles within its own cycle, so engine state stays
/// bounded no matter how many iterations the harness runs.
pub struct Cycle {
    initiator: EngineState<GrowableHeap>,
    responder: EngineState<GrowableHeap>,
    initiator_entropy: Splitmix,
    responder_entropy: Splitmix,
    interfaces: Vec<InterfaceConfig>,
    destination: DestinationHash,
    payload: [u8; PAYLOAD_LEN],
    next_id: u64,
    sealed: Vec<u8>,
    pub proof: Vec<u8>,
}

impl Cycle {
    pub fn new() -> Self {
        let mut responder =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x11; IDENTITY_SECRET_KEY_LEN]));
        let responder_identity = responder.held_identity_hashes()[0];
        let destination = responder
            .register_single_destination(
                &responder_identity,
                "bench",
                &["cycle"],
                b"",
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the bench destination");
        let initiator =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x22; IDENTITY_SECRET_KEY_LEN]));
        let interfaces = vec![tcp_core::descriptor(WIRE, tcp_core::TCP_BITRATE_GUESS_BPS)];

        let mut cycle = Self {
            initiator,
            responder,
            initiator_entropy: Splitmix(1),
            responder_entropy: Splitmix(2),
            interfaces,
            destination,
            payload: [0xAB; PAYLOAD_LEN],
            next_id: 1,
            sealed: Vec::with_capacity(1024),
            proof: Vec::with_capacity(1024),
        };

        let mut announce = Vec::with_capacity(1024);
        let issued = IssuedCommand {
            id: CommandId(0),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };
        cycle.responder.ingest_command_into(
            issued,
            &cycle.interfaces,
            NOW,
            &mut |bytes| cycle.responder_entropy.fill(bytes),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    announce.extend_from_slice(bytes);
                }
            },
        );
        assert!(!announce.is_empty(), "responder emitted its announce");

        let mut heard = false;
        cycle.initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: &mut announce,
            },
            JITTER,
            &cycle.interfaces,
            NOW,
            &mut |bytes| cycle.initiator_entropy.fill(bytes),
            &mut |_| true,
            &mut |reaction| {
                if matches!(
                    reaction,
                    EngineReaction::Journaled(Journaled::AnnounceHeard { .. })
                ) {
                    heard = true;
                }
            },
        );
        assert!(heard, "initiator learned the destination");
        cycle
    }

    /// Act one, on the initiator: ephemeral X25519 keygen + DH, HKDF, token seal,
    /// receipt registration — everything `SendSingle` costs before the wire.
    pub fn seal(&mut self) {
        let issued = IssuedCommand {
            id: CommandId(self.next_id),
            command: EngineCommand::SendSingle(SendSingle {
                destination: self.destination,
                payload: SendSinglePayload::from_slice(&self.payload).expect("payload fits"),
            }),
        };
        self.next_id += 1;
        let Self {
            initiator,
            initiator_entropy,
            interfaces,
            sealed,
            ..
        } = self;
        sealed.clear();
        initiator.ingest_command_into(
            issued,
            interfaces,
            NOW,
            &mut |bytes| initiator_entropy.fill(bytes),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    sealed.extend_from_slice(bytes);
                }
            },
        );
        assert!(!self.sealed.is_empty(), "send sealed a frame");
    }

    /// Act two, on the responder: in-place DH + HKDF + token open, then the implicit
    /// proof — an Ed25519 sign — back the way the packet came.
    pub fn deliver_prove(&mut self) {
        let mut delivered = false;
        let Self {
            responder,
            responder_entropy,
            interfaces,
            sealed,
            proof,
            ..
        } = self;
        proof.clear();
        responder.ingest_packet_into(
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: sealed,
            },
            JITTER,
            interfaces,
            NOW,
            &mut |bytes| responder_entropy.fill(bytes),
            &mut |_| true,
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::Delivered(Delivery::Single(_))) => {
                    delivered = true;
                }
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    proof.extend_from_slice(bytes);
                }
                _ => {}
            },
        );
        assert!(delivered, "responder delivered the single");
        assert!(!self.proof.is_empty(), "responder proved the single");
    }

    /// Act three, on the initiator: Ed25519 verify against the announced identity,
    /// and the receipt settles `Delivered`.
    pub fn settle(&mut self) {
        let mut proof = core::mem::take(&mut self.proof);
        self.settle_frame(&mut proof);
        self.proof = proof;
    }

    pub fn settle_frame(&mut self, proof: &mut [u8]) {
        let mut settled = false;
        let Self {
            initiator,
            initiator_entropy,
            interfaces,
            ..
        } = self;
        initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: proof,
            },
            JITTER,
            interfaces,
            NOW,
            &mut |bytes| initiator_entropy.fill(bytes),
            &mut |_| true,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled {
                    settlement: Settlement::SendSingle(Ok(_)),
                    ..
                }) = reaction
                {
                    settled = true;
                }
            },
        );
        assert!(settled, "proof verified and the receipt settled");
    }
}

/// The transport/relay forwarding path in isolation: a three-engine line —
/// initiator → relay → upstream — where the relay is a pure transport node that
/// switches blind ciphertext between two interfaces. `new` learns the routes the
/// real way (upstream announces, relay hears it and rebroadcasts downstream
/// re-stamped with its own transport id, initiator hears that), so a SINGLE the
/// initiator seals carries the relay's transport id and transits it for real.
/// `seal_single` mints a fresh distinct SINGLE on the initiator (new ephemeral
/// each call, so the relay never dedups it); `forward` is the measured hot path —
/// the relay ingesting that SINGLE and emitting it back out toward the upstream.
pub struct Forward {
    upstream: EngineState<GrowableHeap>,
    relay: EngineState<GrowableHeap>,
    initiator: EngineState<GrowableHeap>,
    upstream_entropy: Splitmix,
    relay_entropy: Splitmix,
    initiator_entropy: Splitmix,
    up_view: Vec<InterfaceConfig>,
    relay_view: Vec<InterfaceConfig>,
    down_view: Vec<InterfaceConfig>,
    destination: DestinationHash,
    payload: [u8; PAYLOAD_LEN],
    next_id: u64,
    single: Vec<u8>,
    scratch: Vec<u8>,
}

impl Forward {
    pub fn new() -> Self {
        let mut upstream =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x41; IDENTITY_SECRET_KEY_LEN]));
        let upstream_identity = upstream.held_identity_hashes()[0];
        let destination = upstream
            .register_single_destination(
                &upstream_identity,
                "bench",
                &["forward"],
                b"",
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the forward destination");
        let relay =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x52; IDENTITY_SECRET_KEY_LEN]));
        let initiator =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x63; IDENTITY_SECRET_KEY_LEN]));

        let mut forward = Self {
            upstream,
            relay,
            initiator,
            upstream_entropy: Splitmix(11),
            relay_entropy: Splitmix(22),
            initiator_entropy: Splitmix(33),
            up_view: vec![tcp_core::descriptor(IF_UP, tcp_core::TCP_BITRATE_GUESS_BPS)],
            relay_view: vec![
                tcp_core::descriptor(IF_UP, tcp_core::TCP_BITRATE_GUESS_BPS),
                tcp_core::descriptor(IF_DOWN, tcp_core::TCP_BITRATE_GUESS_BPS),
            ],
            down_view: vec![tcp_core::descriptor(IF_DOWN, tcp_core::TCP_BITRATE_GUESS_BPS)],
            destination,
            payload: [0xCD; PAYLOAD_LEN],
            next_id: 1,
            single: Vec::with_capacity(1024),
            scratch: vec![0u8; MAX_WIRE_FRAME_LEN],
        };

        forward.learn_routes();
        forward
    }

    fn learn_routes(&mut self) {
        let mut announce = Vec::with_capacity(1024);
        let issued = IssuedCommand {
            id: CommandId(0),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination: self.destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };
        {
            let Self {
                upstream,
                upstream_entropy,
                up_view,
                ..
            } = self;
            upstream.ingest_command_into(
                issued,
                up_view,
                SETUP_NOW,
                &mut |bytes| upstream_entropy.fill(bytes),
                &mut |reaction| {
                    if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                        announce.extend_from_slice(bytes);
                    }
                },
            );
        }
        assert!(!announce.is_empty(), "upstream emitted its announce");

        {
            let Self {
                relay,
                relay_entropy,
                relay_view,
                ..
            } = self;
            let mut heard = false;
            relay.ingest_packet_into(
                InboundPacket {
                    arrived_at: SETUP_NOW,
                    source_interface: IF_UP,
                    bytes: &mut announce,
                },
                JITTER,
                relay_view,
                SETUP_NOW,
                &mut |bytes| relay_entropy.fill(bytes),
                &mut |_| true,
                &mut |reaction| {
                    if matches!(
                        reaction,
                        EngineReaction::Journaled(Journaled::AnnounceHeard { .. })
                    ) {
                        heard = true;
                    }
                },
            );
            assert!(heard, "relay heard the upstream announce");
        }
        assert_eq!(self.relay.route_count(), 1, "relay learned the route");

        let mut rebroadcast = Vec::with_capacity(1024);
        {
            let Self {
                relay, relay_view, ..
            } = self;
            relay.fire_due_scheduled_announces(REBROADCAST_NOW, relay_view, &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { bytes, target, .. }) =
                    reaction
                {
                    if target == IF_DOWN {
                        rebroadcast.extend_from_slice(bytes);
                    }
                }
            });
        }
        assert!(
            !rebroadcast.is_empty(),
            "relay rebroadcast the announce downstream"
        );

        {
            let Self {
                initiator,
                initiator_entropy,
                down_view,
                ..
            } = self;
            let mut heard = false;
            initiator.ingest_packet_into(
                InboundPacket {
                    arrived_at: REBROADCAST_NOW,
                    source_interface: IF_DOWN,
                    bytes: &mut rebroadcast,
                },
                JITTER,
                down_view,
                REBROADCAST_NOW,
                &mut |bytes| initiator_entropy.fill(bytes),
                &mut |_| true,
                &mut |reaction| {
                    if matches!(
                        reaction,
                        EngineReaction::Journaled(Journaled::AnnounceHeard { .. })
                    ) {
                        heard = true;
                    }
                },
            );
            assert!(heard, "initiator heard the relayed announce");
        }
    }

    pub fn seal_single(&mut self) {
        let issued = IssuedCommand {
            id: CommandId(self.next_id),
            command: EngineCommand::SendSingle(SendSingle {
                destination: self.destination,
                payload: SendSinglePayload::from_slice(&self.payload).expect("payload fits"),
            }),
        };
        self.next_id += 1;
        let Self {
            initiator,
            initiator_entropy,
            down_view,
            single,
            ..
        } = self;
        single.clear();
        initiator.ingest_command_into(
            issued,
            down_view,
            FORWARD_NOW,
            &mut |bytes| initiator_entropy.fill(bytes),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    single.extend_from_slice(bytes);
                }
            },
        );
        assert!(!self.single.is_empty(), "initiator sealed a single via the relay");
    }

    pub fn seal_many(&mut self, count: usize) -> Vec<Vec<u8>> {
        let mut frames = Vec::with_capacity(count);
        for _ in 0..count {
            self.seal_single();
            frames.push(self.single.clone());
        }
        frames
    }

    pub fn forward(&mut self) -> bool {
        let mut single = core::mem::take(&mut self.single);
        let forwarded = self.forward_frame(&mut single);
        self.single = single;
        forwarded
    }

    pub fn forward_frame(&mut self, frame: &mut [u8]) -> bool {
        let mut forwarded = false;
        let Self {
            relay,
            relay_entropy,
            relay_view,
            scratch,
            ..
        } = self;
        relay.ingest_packet_into(
            InboundPacket {
                arrived_at: FORWARD_NOW,
                source_interface: IF_DOWN,
                bytes: frame,
            },
            JITTER,
            relay_view,
            FORWARD_NOW,
            &mut |bytes| relay_entropy.fill(bytes),
            &mut |_| true,
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { target, fill }) = reaction {
                    if target == IF_UP && fill(&mut scratch[..]).is_some() {
                        forwarded = true;
                    }
                }
            },
        );
        forwarded
    }
}
