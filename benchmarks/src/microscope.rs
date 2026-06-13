use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, Directive, EngineCommand,
    EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled, RatchetPolicy,
    SendSingle, SendSinglePayload, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{InboundPacket, InterfaceConfig, InterfaceId};
use personal_rns::reactor::interfaces::tcp::core as tcp_core;
use personal_rns::routing::announce::defaults::JitterSeed;
use personal_rns::routing::delivery::Delivery;
use personal_rns::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;

const WIRE: InterfaceId = InterfaceId::new([0xC7; 16]);
const NOW: InstantMillis = InstantMillis(1_000);
const JITTER: JitterSeed = JitterSeed(7);
pub const PAYLOAD_LEN: usize = 300;

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
