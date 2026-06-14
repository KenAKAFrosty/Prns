use crate::crypto::{X25519PublicKey, X25519SecretKey};
use crate::engine::commands::{CommandId, CommandOutcome, SendSingle, SendSingleError};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::{EncryptError, RemoteIdentity, ENCRYPTION_IV_LEN};
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{
    CulledReceipt, ExpiredReceipt, OutstandingReceipt, ReceiptKind,
};
use crate::routing::NextHop;
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext,
    WirePacketHeader,
};

/// RNS 1.3.1 `Reticulum.DEFAULT_PER_HOP_TIMEOUT` (6s), serving both as the
/// first-hop fallback (`Transport.first_hop_timeout` without bitrate data)
/// and the per-hop increment (`Packet.TIMEOUT_PER_HOP`).
pub const DEFAULT_FIRST_HOP_TIMEOUT_MS: u64 = 6_000;
pub const DEFAULT_PER_HOP_TIMEOUT_MS: u64 = 6_000;

pub const SEND_SINGLE_ENTROPY_LEN: usize = 32 + ENCRYPTION_IV_LEN;

/// One send's worth of sealing entropy: the ephemeral X25519 secret and the
/// token IV. Move-only and never shown. Consuming it seals exactly one
/// packet, so one draw can never key two.
pub struct SendSingleEntropy([u8; SEND_SINGLE_ENTROPY_LEN]);

impl SendSingleEntropy {
    pub const LEN: usize = SEND_SINGLE_ENTROPY_LEN;

    pub const fn new(bytes: [u8; SEND_SINGLE_ENTROPY_LEN]) -> Self {
        Self(bytes)
    }

    fn into_parts(self) -> (X25519SecretKey, [u8; ENCRYPTION_IV_LEN]) {
        let mut ephemeral = [0u8; 32];
        ephemeral.copy_from_slice(&self.0[..32]);
        let mut iv = [0u8; ENCRYPTION_IV_LEN];
        iv.copy_from_slice(&self.0[32..]);
        (X25519SecretKey::new(ephemeral), iv)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendSingleDispatch {
    pub wire_len: usize,
    pub fire_on: InterfaceId,
    pub culled: Option<CulledReceipt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteSendSingleError {
    RouteVanished,
    Seal(EncryptError),
    Serialize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSingleRejection {
    RouteVanished,
    Serialize,
}

impl From<SendSingleRejection> for WriteSendSingleError {
    fn from(rejection: SendSingleRejection) -> Self {
        match rejection {
            SendSingleRejection::RouteVanished => Self::RouteVanished,
            SendSingleRejection::Serialize => Self::Serialize,
        }
    }
}

#[must_use]
pub enum SendSingleWriteOutcome {
    Written(SendSingleDispatch),
    Rejected {
        rejection: SendSingleRejection,
        unspent_entropy: SendSingleEntropy,
    },
    Failed {
        failure: EncryptError,
    },
}

impl<S: StorageLayout> EngineState<S> {
    pub(crate) fn ingest_send_single(&self, id: CommandId, send: SendSingle) -> CommandOutcome {
        let Some(retained) = self.routing_table.retained_announce_for(&send.destination) else {
            return CommandOutcome::SendSingleRejected {
                id,
                error: SendSingleError::NoRouteToDestination,
            };
        };
        if retained.hops > 1 && retained.next_hop == NextHop::Direct {
            return CommandOutcome::SendSingleRejected {
                id,
                error: SendSingleError::NotDirectlyReachable,
            };
        }
        CommandOutcome::OwesSendSingle { id, send }
    }

    /// Seal `send`'s payload to the peer's announced ratchet (identity key
    /// when it never announced one; RNS 1.3.1 `Destination.encrypt`), frame
    /// it directly into `buf`, and track the receipt that will settle `id`.
    pub fn write_commanded_send_single(
        &mut self,
        id: CommandId,
        send: &SendSingle,
        now: InstantMillis,
        entropy: SendSingleEntropy,
        buf: &mut [u8],
    ) -> SendSingleWriteOutcome {
        use SendSingleWriteOutcome::{Failed, Rejected, Written};

        let Some(retained) = self.routing_table.retained_announce_for(&send.destination) else {
            return Rejected {
                rejection: SendSingleRejection::RouteVanished,
                unspent_entropy: entropy,
            };
        };
        let hops = retained.hops;
        let fire_on = retained.receiving_interface;
        let public_keys = retained.announce.public_keys;
        let maybe_ratchet = retained.announce.maybe_ratchet;

        // RNS 1.3.1 `Transport.outbound`: a packet for a destination more than
        // one hop away is injected into transport, addressed at the relay that
        // announced the route, instead of broadcast at the destination.
        let (propagation, transport_id) = match retained.next_hop {
            NextHop::Direct => (PropagationType::Broadcast, None),
            NextHop::Via(via) => (PropagationType::Transport, Some(via)),
        };
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id,
            destination: send.destination,
            context: WireContext::None,
        };
        let Ok(header_len) = header.write(buf) else {
            return Rejected {
                rejection: SendSingleRejection::Serialize,
                unspent_entropy: entropy,
            };
        };

        let remote = RemoteIdentity::from_public_keys(public_keys.encryption, public_keys.signing);
        let (ephemeral_secret, iv) = entropy.into_parts();
        let sealed_result = match maybe_ratchet {
            Some(ratchet) => remote.encrypt_to_ratchet(
                &X25519PublicKey(*ratchet.as_bytes()),
                &ephemeral_secret,
                &iv,
                &send.payload,
                &mut buf[header_len..],
            ),
            None => remote.encrypt(
                &ephemeral_secret,
                &iv,
                &send.payload,
                &mut buf[header_len..],
            ),
        };

        let sealed_len = match sealed_result {
            Ok(x) => x,
            Err(error) => return Failed { failure: error },
        };
        let wire_len = header_len + sealed_len;

        let packet_hash = PacketHash::of_data_fields(
            DestinationType::Single,
            &send.destination,
            WireContext::None,
            &buf[header_len..wire_len],
        );
        let timeout_at = InstantMillis(
            now.0
                .saturating_add(DEFAULT_FIRST_HOP_TIMEOUT_MS)
                .saturating_add(DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(hops))),
        );
        let culled = self.receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: id,
            kind: ReceiptKind::SendSingle,
            peer_signing_key: public_keys.signing,
            sent_at: now,
            timeout_at,
        });

        Written(SendSingleDispatch {
            wire_len,
            fire_on,
            culled,
        })
    }

    /// Drain one sent SINGLE whose proof never arrived. Call repeatedly until `None` to fully drain.
    /// Every pop is that command's timeout settlement.
    pub fn pop_timed_out_receipt(&mut self, now: InstantMillis) -> Option<ExpiredReceipt> {
        self.receipts.pop_expired(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{
        AnnounceAppData, AnnounceIngest, AnnounceNow, AnnounceTarget, CommandOutcome,
        EngineCommand, IngestPacketOutcome, IssuedCommand, RatchetPolicy, SendSinglePayload,
    };
    use crate::interfaces::InboundPacket;
    use crate::routing::delivery::{Delivery, SingleDelivery};
    use crate::wire::{DestinationHash, BROADCAST_MTU};

    impl SendSingleWriteOutcome {
        #[track_caller]
        pub fn dispatched(self) -> SendSingleDispatch {
            match self {
                Self::Written(dispatch) => dispatch,
                Self::Rejected {
                    rejection: error, ..
                } => {
                    panic!("expected Written, got Rejected({error:?})")
                }
                Self::Failed { failure: error } => {
                    panic!("expected Written, got Failed({error:?})")
                }
            }
        }

        #[track_caller]
        pub fn rejection(self) -> (SendSingleRejection, SendSingleEntropy) {
            match self {
                Self::Rejected {
                    rejection: error,
                    unspent_entropy: entropy,
                } => (error, entropy),
                Self::Written(dispatch) => panic!("expected Rejected, got Written({dispatch:?})"),
                Self::Failed { failure: error } => {
                    panic!("expected Rejected, got Failed({error:?})")
                }
            }
        }
    }

    const PEER_DESTINATION_HEX: &str = "c3cfae69b36bb6e3bbfd96a3b5867a59";

    fn peer_destination() -> DestinationHash {
        DestinationHash::new(hx(PEER_DESTINATION_HEX).try_into().unwrap())
    }

    fn vector_send_entropy() -> SendSingleEntropy {
        let mut bytes = [0x33u8; SendSingleEntropy::LEN];
        bytes[32..].fill(0x44);
        SendSingleEntropy::new(bytes)
    }

    fn hearer() -> EngineState<Cap> {
        EngineState::new(second_secret_key())
    }

    fn hear_announce(
        state: &mut EngineState<Cap>,
        wire: &[u8],
        arrival: crate::interfaces::InterfaceId,
    ) {
        let (header, _) =
            WirePacketHeader::parse(wire).expect("the announce fixture is a parseable wire packet");
        let announced = crate::engine::AcceptedAnnounce {
            destination: header.destination,
            hops: header.hops + 1,
            rebroadcast: crate::engine::RebroadcastDecision::Scheduled,
        };
        let mut raw = wire.to_vec();
        let outcome = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: arrival,
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            outcome,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(announced)),
            "the announce fixture must take a route before sending",
        );
    }

    fn send_of(payload: &[u8]) -> SendSingle {
        SendSingle {
            destination: peer_destination(),
            payload: SendSinglePayload::from_slice(payload).unwrap(),
        }
    }

    fn arrival() -> crate::interfaces::InterfaceId {
        crate::interfaces::InterfaceId::new([0xA1; 16])
    }

    fn unratcheted_neighbor_with_a_tracked_send(
        payload: &[u8],
        sent_at: u64,
    ) -> (EngineState<Cap>, std::vec::Vec<u8>) {
        let mut announcer = personal_node_announcer();
        let mut announce_buf = [0u8; BROADCAST_MTU];
        let announce_len = announcer
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(100),
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();

        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single(
                CommandId(7),
                &send_of(payload),
                InstantMillis(sent_at),
                vector_send_entropy(),
                &mut buf,
            )
            .dispatched();
        (state, buf[..dispatch.wire_len].to_vec())
    }

    #[test]
    fn a_rejected_send_hands_the_entropy_home_for_a_byte_identical_retry() {
        let (_, expected_wire) = unratcheted_neighbor_with_a_tracked_send(b"retry-me", 1_000);

        let mut announcer = personal_node_announcer();
        let mut announce_buf = [0u8; BROADCAST_MTU];
        let announce_len = announcer
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(100),
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();
        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());

        let stranger = SendSingle {
            destination: DestinationHash::new([0xEE; 16]),
            payload: SendSinglePayload::from_slice(b"retry-me").unwrap(),
        };
        let mut buf = [0u8; BROADCAST_MTU];
        let (error, came_home) = state
            .write_commanded_send_single(
                CommandId(6),
                &stranger,
                InstantMillis(500),
                vector_send_entropy(),
                &mut buf,
            )
            .rejection();
        assert_eq!(error, SendSingleRejection::RouteVanished);

        let dispatch = state
            .write_commanded_send_single(
                CommandId(7),
                &send_of(b"retry-me"),
                InstantMillis(1_000),
                came_home,
                &mut buf,
            )
            .dispatched();
        assert_eq!(
            &buf[..dispatch.wire_len],
            &expected_wire[..],
            "the unit that came home seals byte-identical wire on the retry",
        );
    }

    fn proof_packet(payload: &[u8], proven: &PacketHash) -> std::vec::Vec<u8> {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Proof,
            hops: 0,
            transport_id: None,
            destination: proven.proof_destination(),
            context: WireContext::None,
        };
        let mut bytes = std::vec![0u8; crate::wire::HEADER_MIN_LEN + payload.len()];
        let written = header.write(&mut bytes).unwrap();
        bytes[written..].copy_from_slice(payload);
        bytes
    }

    #[test]
    fn a_send_to_a_ratcheted_neighbor_reproduces_the_rns_1_3_1_wire() {
        let mut state = hearer();
        hear_announce(&mut state, &hx(RATCHETED_ANNOUNCE_RNS_WIRE), arrival());
        let send = send_of(b"ratchet-parity");

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::SendSingle(send.clone()),
                },
                &[],
            ),
            CommandOutcome::OwesSendSingle {
                id: CommandId(7),
                send: send.clone(),
            },
        );

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single(
                CommandId(7),
                &send,
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .dispatched();

        assert_eq!(
            &buf[..dispatch.wire_len],
            hx(RAW_SEALED_TO_RATCHET).as_slice()
        );
        assert_eq!(dispatch.fire_on, arrival());
        assert_eq!(dispatch.culled, None);
        assert_eq!(state.receipts.len(), 1);
    }

    #[test]
    fn a_send_to_an_unratcheted_neighbor_seals_to_the_identity_key() {
        let mut announcer = personal_node_announcer();
        let mut announce_buf = [0u8; BROADCAST_MTU];
        let announce_len = announcer
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(100),
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();

        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());
        let send = send_of(b"hello-by-key");

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single(
                CommandId(7),
                &send,
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .dispatched();

        let fixture = crate::identity::in_memory::InMemoryNodeIdentity::from_secret_key_bytes(&{
            let mut bytes = [0u8; crate::identity::IDENTITY_SECRET_KEY_LEN];
            bytes[..32].fill(0x22);
            bytes[32..].fill(0x11);
            bytes
        });
        let expected = sealed_single_packet(&fixture, peer_destination(), b"hello-by-key");
        assert_eq!(&buf[..dispatch.wire_len], expected.as_slice());
    }

    #[test]
    fn a_send_with_no_route_is_rejected() {
        let mut state = hearer();
        let send = send_of(b"into-the-void");
        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::SendSingle(send),
                },
                &[],
            ),
            CommandOutcome::SendSingleRejected {
                id: CommandId(7),
                error: SendSingleError::NoRouteToDestination,
            },
        );
        assert_eq!(state.receipts.len(), 0);
    }

    #[test]
    fn a_multi_hop_route_with_no_relay_to_address_is_not_directly_reachable() {
        let mut state = hearer();
        let mut relayed = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        relayed[1] = 1;
        hear_announce(&mut state, &relayed, arrival());

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::SendSingle(send_of(b"too-far")),
                },
                &[],
            ),
            CommandOutcome::SendSingleRejected {
                id: CommandId(7),
                error: SendSingleError::NotDirectlyReachable,
            },
        );
    }

    #[test]
    fn a_send_to_a_multi_hop_destination_is_addressed_at_its_relay() {
        let mut state = hearer();
        hear_announce(&mut state, &hx(RNS_1_3_1_RETRANSMITTED_ANNOUNCE), arrival());
        let send = send_of(b"ratchet-parity");

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::SendSingle(send.clone()),
                },
                &[],
            ),
            CommandOutcome::OwesSendSingle {
                id: CommandId(7),
                send: send.clone(),
            },
        );

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single(
                CommandId(7),
                &send,
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .dispatched();

        assert_eq!(
            &buf[..dispatch.wire_len],
            hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT).as_slice(),
            "the sealed packet rides transport addressed at the announcing relay",
        );
        assert_eq!(dispatch.fire_on, arrival());
        assert_eq!(dispatch.culled, None);
        assert_eq!(state.receipts.len(), 1);
    }

    #[test]
    fn a_sent_packet_round_trips_into_the_peer_engine() {
        let mut peer = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let mut announce_buf = [0u8; BROADCAST_MTU];
        let announce_len = peer
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(100),
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();

        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());
        let send = send_of(b"loopback-hello");

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single(
                CommandId(7),
                &send,
                InstantMillis(1_000),
                {
                    let mut bytes = [0x77u8; SendSingleEntropy::LEN];
                    bytes[32..].fill(0x0B);
                    SendSingleEntropy::new(bytes)
                },
                &mut buf,
            )
            .dispatched();

        let mut wire = buf[..dispatch.wire_len].to_vec();
        assert_eq!(
            peer.ingest_packet(
                plain_data_packet(&mut wire),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: peer_destination(),
                    context: crate::wire::WireContext::None,
                    plaintext: b"loopback-hello",
                    arrived_at: InstantMillis(1_000),
                    source_interface: crate::interfaces::InterfaceId::new([0x07; 16]),
                }),
                proof: crate::routing::proof::ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_ninth_send_culls_the_stalest_receipt() {
        let mut state = hearer();
        hear_announce(&mut state, &hx(RATCHETED_ANNOUNCE_RNS_WIRE), arrival());

        let mut buf = [0u8; BROADCAST_MTU];
        for i in 1..=8u64 {
            let dispatch = state
                .write_commanded_send_single(
                    CommandId(i),
                    &send_of(&[i as u8]),
                    InstantMillis(1_000 * i),
                    vector_send_entropy(),
                    &mut buf,
                )
                .dispatched();
            assert_eq!(dispatch.culled, None);
        }

        let dispatch = state
            .write_commanded_send_single(
                CommandId(9),
                &send_of(b"the-straw"),
                InstantMillis(9_000),
                vector_send_entropy(),
                &mut buf,
            )
            .dispatched();
        assert_eq!(
            dispatch.culled,
            Some(crate::routing::delivery::receipts::CulledReceipt {
                command_id: CommandId(1),
                kind: ReceiptKind::SendSingle,
            }),
        );
        assert_eq!(state.receipts.len(), 8);
    }

    #[test]
    fn a_python_minted_proof_settles_the_tracked_send_with_its_rtt() {
        use crate::engine::{Delivered, ProofIngest};

        let (mut state, wire) = unratcheted_neighbor_with_a_tracked_send(b"proof-parity", 1_000);
        assert_eq!(wire, hx(RAW_SEALED_FOR_PROOF));

        let mut proof = hx(RNS_1_3_1_IMPLICIT_PROOF);
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_250),
                    source_interface: arrival(),
                    bytes: &mut proof,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Proof(ProofIngest::SendSingleDelivered {
                id: CommandId(7),
                delivered: Delivered { rtt_ms: 250 },
            }),
        );
        assert_eq!(state.receipts.len(), 0);

        let mut replay = hx(RNS_1_3_1_IMPLICIT_PROOF);
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_300),
                    source_interface: arrival(),
                    bytes: &mut replay,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Proof(ProofIngest::Ignored),
            "settlement removed the receipt, so a replayed proof finds nothing",
        );
    }

    #[test]
    fn an_explicit_proof_settles_the_send_too() {
        use crate::crypto::{ed25519_sign, Ed25519SecretKey};
        use crate::engine::{Delivered, ProofIngest};
        use crate::routing::proof::EXPLICIT_PROOF_PAYLOAD_LEN;

        let (mut state, wire) = unratcheted_neighbor_with_a_tracked_send(b"explicitly", 2_000);
        let proven = PacketHash::of_wire_packet(&wire).unwrap();
        let signature = ed25519_sign(&Ed25519SecretKey::new([0x11; 32]), proven.as_bytes());

        let mut payload = [0u8; EXPLICIT_PROOF_PAYLOAD_LEN];
        payload[..32].copy_from_slice(proven.as_bytes());
        payload[32..].copy_from_slice(&signature.0);
        let mut packet = proof_packet(&payload, &proven);

        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(2_500),
                    source_interface: arrival(),
                    bytes: &mut packet,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Proof(ProofIngest::SendSingleDelivered {
                id: CommandId(7),
                delivered: Delivered { rtt_ms: 500 },
            }),
        );
        assert_eq!(state.receipts.len(), 0);
    }

    #[test]
    fn a_forged_proof_leaves_the_send_outstanding() {
        use crate::crypto::{ed25519_sign, Ed25519SecretKey};
        use crate::engine::ProofIngest;

        let (mut state, wire) = unratcheted_neighbor_with_a_tracked_send(b"unforgeable", 1_000);
        let proven = PacketHash::of_wire_packet(&wire).unwrap();
        let forged = ed25519_sign(&Ed25519SecretKey::new([0x99; 32]), proven.as_bytes());
        let mut packet = proof_packet(&forged.0, &proven);

        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_250),
                    source_interface: arrival(),
                    bytes: &mut packet,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Proof(ProofIngest::Ignored),
        );
        assert_eq!(state.receipts.len(), 1, "the timeout still owns the send");
    }

    #[test]
    fn an_alien_length_proof_payload_is_ignored() {
        use crate::engine::ProofIngest;

        let mut state = hearer();
        let mut packet = proof_packet(&[0u8; 65], &PacketHash::new([0xAA; 32]));
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: arrival(),
                    bytes: &mut packet,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Proof(ProofIngest::Ignored),
        );
    }

    #[test]
    fn a_timed_out_send_pops_once_for_its_settlement() {
        let mut state = hearer();
        hear_announce(&mut state, &hx(RATCHETED_ANNOUNCE_RNS_WIRE), arrival());
        let mut buf = [0u8; BROADCAST_MTU];
        state
            .write_commanded_send_single(
                CommandId(7),
                &send_of(b"timed"),
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .dispatched();

        assert_eq!(state.pop_timed_out_receipt(InstantMillis(12_999)), None);
        assert_eq!(
            state.pop_timed_out_receipt(InstantMillis(13_000)),
            Some(ExpiredReceipt {
                command_id: CommandId(7),
                kind: ReceiptKind::SendSingle,
            }),
        );
        assert_eq!(state.pop_timed_out_receipt(InstantMillis(13_000)), None);
        assert_eq!(state.receipts.len(), 0);
    }
}
