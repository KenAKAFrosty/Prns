use crate::crypto::{x25519_keys_for_seal, X25519PublicKey, X25519SecretKey, X25519SharedSecret};
use crate::engine::{
    CommandId, CommandOutcome, SendSinglePacket, SendSinglePacketPayload, SendSinglePacketRejection,
};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::{
    seal_finish, EncryptError, IdentityHash, IdentitySigningPublicKey, RemoteIdentity,
    ENCRYPTION_IV_LEN,
};
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

/// RNS 1.3.5 `Reticulum.DEFAULT_PER_HOP_TIMEOUT` (6s), serving both as the
/// first-hop fallback (`Transport.first_hop_timeout` without bitrate data)
/// and the per-hop increment (`Packet.TIMEOUT_PER_HOP`).
pub const DEFAULT_FIRST_HOP_TIMEOUT_MS: u64 = 6_000;
pub const DEFAULT_PER_HOP_TIMEOUT_MS: u64 = 6_000;

pub const SEND_SINGLE_ENTROPY_LEN: usize = 32 + ENCRYPTION_IV_LEN;

/// Move-only and never shown; consuming it seals exactly one packet, so one draw
/// can never key two.
pub struct SendSinglePacketEntropy([u8; SEND_SINGLE_ENTROPY_LEN]);

impl SendSinglePacketEntropy {
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
pub struct SendSinglePacketDispatch {
    pub wire_len: usize,
    pub fire_on: InterfaceId,
    pub culled: Option<CulledReceipt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteSendSinglePacketError {
    RouteVanished,
    Seal(EncryptError),
    Serialize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSinglePacketWriteRejection {
    RouteVanished,
    Serialize,
}

impl From<SendSinglePacketWriteRejection> for WriteSendSinglePacketError {
    fn from(rejection: SendSinglePacketWriteRejection) -> Self {
        match rejection {
            SendSinglePacketWriteRejection::RouteVanished => Self::RouteVanished,
            SendSinglePacketWriteRejection::Serialize => Self::Serialize,
        }
    }
}

#[must_use]
pub enum SendSinglePacketWriteOutcome {
    Written(SendSinglePacketDispatch),
    Rejected {
        rejection: SendSinglePacketWriteRejection,
        unspent_entropy: SendSinglePacketEntropy,
    },
    Failed {
        failure: EncryptError,
    },
}

struct SendSinglePacketPlan {
    header: WirePacketHeader,
    dh_target: X25519PublicKey,
    recipient_identity_hash: IdentityHash,
    peer_signing_key: IdentitySigningPublicKey,
    timeout_at: InstantMillis,
    fire_on: InterfaceId,
}

/// Move-only: it carries the ephemeral secret, which seals exactly one packet.
pub struct EncryptOwed {
    pub header: WirePacketHeader,
    pub dh_target: X25519PublicKey,
    pub recipient_identity_hash: IdentityHash,
    pub ephemeral_secret: X25519SecretKey,
    pub iv: [u8; ENCRYPTION_IV_LEN],
    pub payload: SendSinglePacketPayload,
    pub command_id: CommandId,
    pub peer_signing_key: IdentitySigningPublicKey,
    pub sent_at: InstantMillis,
    pub timeout_at: InstantMillis,
    pub fire_on: InterfaceId,
}

#[must_use]
#[allow(clippy::large_enum_variant)]
pub enum SendSinglePacketPrepared {
    Owed(EncryptOwed),
    Rejected {
        id: CommandId,
        rejection: SendSinglePacketRejection,
    },
    RouteVanished {
        id: CommandId,
    },
}

#[must_use]
pub enum FinishSendSinglePacketOutcome {
    Written(SendSinglePacketDispatch),
    Failed(WriteSendSinglePacketError),
}

impl<S: StorageLayout> EngineState<S> {
    pub(crate) fn ingest_send_single_packet(
        &self,
        id: CommandId,
        send: SendSinglePacket,
    ) -> CommandOutcome {
        let Some(stored) = self.routing_table.stored_announce_for(&send.destination) else {
            return CommandOutcome::SendSinglePacketRejected {
                id,
                rejection: SendSinglePacketRejection::NoRouteToDestination,
            };
        };
        if stored.hops > 1 && stored.next_hop == NextHop::Direct {
            return CommandOutcome::SendSinglePacketRejected {
                id,
                rejection: SendSinglePacketRejection::NotDirectlyReachable,
            };
        }
        CommandOutcome::OwesSendSinglePacket { id, send }
    }

    /// Seals to the peer's announced ratchet, identity key when it never announced
    /// one (RNS 1.3.5 `Destination.encrypt`).
    pub fn write_commanded_send_single_packet(
        &mut self,
        id: CommandId,
        send: &SendSinglePacket,
        now: InstantMillis,
        entropy: SendSinglePacketEntropy,
        buf: &mut [u8],
    ) -> SendSinglePacketWriteOutcome {
        use SendSinglePacketWriteOutcome::{Failed, Rejected, Written};

        let Some(plan) = self.gather_send_single_plan(send, now) else {
            return Rejected {
                rejection: SendSinglePacketWriteRejection::RouteVanished,
                unspent_entropy: entropy,
            };
        };
        let Ok(header_len) = plan.header.write(buf) else {
            return Rejected {
                rejection: SendSinglePacketWriteRejection::Serialize,
                unspent_entropy: entropy,
            };
        };

        let (ephemeral_secret, iv) = entropy.into_parts();
        let (ephemeral_public, shared) = x25519_keys_for_seal(&ephemeral_secret, &plan.dh_target);
        let sealed_len = match seal_finish(
            &plan.recipient_identity_hash,
            &ephemeral_public,
            &shared,
            &iv,
            &send.payload,
            &mut buf[header_len..],
        ) {
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
        let culled = self.receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: id,
            kind: ReceiptKind::SendSinglePacket,
            peer_signing_key: plan.peer_signing_key,
            sent_at: now,
            timeout_at: plan.timeout_at,
        });

        Written(SendSinglePacketDispatch {
            wire_len,
            fire_on: plan.fire_on,
            culled,
        })
    }

    fn gather_send_single_plan(
        &self,
        send: &SendSinglePacket,
        now: InstantMillis,
    ) -> Option<SendSinglePacketPlan> {
        let stored = self.routing_table.stored_announce_for(&send.destination)?;
        let hops = stored.hops;
        let fire_on = stored.receiving_interface;
        let public_keys = stored.announce.public_keys;
        let ratchet = stored.announce.ratchet;

        // RNS 1.3.5 `Transport.outbound`: hops > 0 is injected into transport, addressed
        // at the relay; hops == 0 (including a sibling behind the same shared instance) is
        // broadcast at the destination, so the instance delivers it locally.
        let (propagation, transport_id) = match stored.next_hop {
            NextHop::Via(via) if hops > 0 => (PropagationType::Transport, Some(via)),
            _ => (PropagationType::Broadcast, None),
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
        let dh_target = match ratchet {
            Some(ratchet) => X25519PublicKey(*ratchet.as_bytes()),
            None => *public_keys.encryption.as_x25519(),
        };
        let recipient_identity_hash =
            RemoteIdentity::from_public_keys(public_keys.encryption, public_keys.signing)
                .identity_hash();
        let timeout_at = InstantMillis(
            now.0
                .saturating_add(DEFAULT_FIRST_HOP_TIMEOUT_MS)
                .saturating_add(DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(hops))),
        );
        Some(SendSinglePacketPlan {
            header,
            dh_target,
            recipient_identity_hash,
            peer_signing_key: public_keys.signing,
            timeout_at,
            fire_on,
        })
    }

    /// `&self` and side-effect-free: nothing is tracked until the scalars are back,
    /// so an abandoned obligation leaves no orphan receipt.
    pub fn prepare_send_single_packet_deferred(
        &self,
        id: CommandId,
        send: SendSinglePacket,
        now: InstantMillis,
        entropy: SendSinglePacketEntropy,
    ) -> SendSinglePacketPrepared {
        let send = match self.ingest_send_single_packet(id, send) {
            CommandOutcome::OwesSendSinglePacket { send, .. } => send,
            CommandOutcome::SendSinglePacketRejected { id, rejection } => {
                return SendSinglePacketPrepared::Rejected { id, rejection }
            }
            _ => return SendSinglePacketPrepared::RouteVanished { id },
        };
        let Some(plan) = self.gather_send_single_plan(&send, now) else {
            return SendSinglePacketPrepared::RouteVanished { id };
        };
        let (ephemeral_secret, iv) = entropy.into_parts();
        SendSinglePacketPrepared::Owed(EncryptOwed {
            header: plan.header,
            dh_target: plan.dh_target,
            recipient_identity_hash: plan.recipient_identity_hash,
            ephemeral_secret,
            iv,
            payload: send.payload,
            command_id: id,
            peer_signing_key: plan.peer_signing_key,
            sent_at: now,
            timeout_at: plan.timeout_at,
            fire_on: plan.fire_on,
        })
    }

    /// The same bytes and row the inline path produces; the only difference is where
    /// the X25519 ran.
    pub fn finish_send_single_packet_deferred(
        &mut self,
        owed: EncryptOwed,
        ephemeral_public: X25519PublicKey,
        shared: X25519SharedSecret,
        buf: &mut [u8],
    ) -> FinishSendSinglePacketOutcome {
        let Ok(header_len) = owed.header.write(buf) else {
            return FinishSendSinglePacketOutcome::Failed(WriteSendSinglePacketError::Serialize);
        };
        let sealed_len = match seal_finish(
            &owed.recipient_identity_hash,
            &ephemeral_public,
            &shared,
            &owed.iv,
            &owed.payload,
            &mut buf[header_len..],
        ) {
            Ok(x) => x,
            Err(error) => {
                return FinishSendSinglePacketOutcome::Failed(WriteSendSinglePacketError::Seal(
                    error,
                ))
            }
        };
        let wire_len = header_len + sealed_len;

        let packet_hash = PacketHash::of_data_fields(
            DestinationType::Single,
            &owed.header.destination,
            WireContext::None,
            &buf[header_len..wire_len],
        );
        let culled = self.receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: owed.command_id,
            kind: ReceiptKind::SendSinglePacket,
            peer_signing_key: owed.peer_signing_key,
            sent_at: owed.sent_at,
            timeout_at: owed.timeout_at,
        });

        FinishSendSinglePacketOutcome::Written(SendSinglePacketDispatch {
            wire_len,
            fire_on: owed.fire_on,
            culled,
        })
    }

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
        EngineCommand, IngestPacketOutcome, IssuedCommand, RatchetPolicy, SendSinglePacketPayload,
    };
    use crate::interfaces::InboundPacket;
    use crate::routing::delivery::{Delivery, SingleDelivery};
    use crate::wire::{DestinationHash, BROADCAST_MTU};

    impl SendSinglePacketWriteOutcome {
        #[track_caller]
        pub fn dispatched(self) -> SendSinglePacketDispatch {
            match self {
                Self::Written(dispatch) => dispatch,
                Self::Rejected { rejection, .. } => {
                    panic!("expected Written, got Rejected({rejection:?})")
                }
                Self::Failed { failure: error } => {
                    panic!("expected Written, got Failed({error:?})")
                }
            }
        }

        #[track_caller]
        pub fn rejection(self) -> (SendSinglePacketWriteRejection, SendSinglePacketEntropy) {
            match self {
                Self::Rejected {
                    rejection,
                    unspent_entropy: entropy,
                } => (rejection, entropy),
                Self::Written(dispatch) => panic!("expected Rejected, got Written({dispatch:?})"),
                Self::Failed { failure: error } => {
                    panic!("expected Rejected, got Failed({error:?})")
                }
            }
        }
    }

    const PEER_DESTINATION_HEX: &str = "c3cfae69b36bb6e3bbfd96a3b5867a59";

    fn peer_destination() -> DestinationHash {
        DestinationHash::new(bytes_from_hex(PEER_DESTINATION_HEX).try_into().unwrap())
    }

    fn vector_send_entropy() -> SendSinglePacketEntropy {
        let mut bytes = [0x33u8; SendSinglePacketEntropy::LEN];
        bytes[32..].fill(0x44);
        SendSinglePacketEntropy::new(bytes)
    }

    fn hearer() -> EngineState<TestStorageLayout> {
        EngineState::new(second_secret_key())
    }

    fn hear_announce(
        state: &mut EngineState<TestStorageLayout>,
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
        let outcome = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: arrival,
                bytes: &mut raw,
            },
            &mut |_| {},
            &transporting_interfaces(),
            &mut |_| {},
            None,
        );
        assert_eq!(
            outcome,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(announced)),
            "the announce fixture must take a route before sending",
        );
    }

    fn send_of(payload: &[u8]) -> SendSinglePacket {
        SendSinglePacket {
            destination: peer_destination(),
            payload: SendSinglePacketPayload::from_slice(payload).unwrap(),
        }
    }

    fn arrival() -> crate::interfaces::InterfaceId {
        crate::interfaces::InterfaceId::new([0xA1; 8])
    }

    fn unratcheted_neighbor_with_a_tracked_send(
        payload: &[u8],
        sent_at: u64,
    ) -> (EngineState<TestStorageLayout>, std::vec::Vec<u8>) {
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
                &mut test_fill_entropy,
                &mut announce_buf,
            )
            .written_len();

        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single_packet(
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
                &mut test_fill_entropy,
                &mut announce_buf,
            )
            .written_len();
        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());

        let stranger = SendSinglePacket {
            destination: DestinationHash::new([0xEE; 16]),
            payload: SendSinglePacketPayload::from_slice(b"retry-me").unwrap(),
        };
        let mut buf = [0u8; BROADCAST_MTU];
        let (error, came_home) = state
            .write_commanded_send_single_packet(
                CommandId(6),
                &stranger,
                InstantMillis(500),
                vector_send_entropy(),
                &mut buf,
            )
            .rejection();
        assert_eq!(error, SendSinglePacketWriteRejection::RouteVanished);

        let dispatch = state
            .write_commanded_send_single_packet(
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
    fn a_send_to_a_ratcheted_neighbor_reproduces_the_rns_1_3_5_wire() {
        let mut state = hearer();
        hear_announce(
            &mut state,
            &bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE),
            arrival(),
        );
        let send = send_of(b"ratchet-parity");

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::SendSinglePacket(send.clone()),
                },
                &[],
            ),
            CommandOutcome::OwesSendSinglePacket {
                id: CommandId(7),
                send: send.clone(),
            },
        );

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single_packet(
                CommandId(7),
                &send,
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .dispatched();

        assert_eq!(
            &buf[..dispatch.wire_len],
            bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET).as_slice()
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
                &mut test_fill_entropy,
                &mut announce_buf,
            )
            .written_len();

        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());
        let send = send_of(b"hello-by-key");

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single_packet(
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
    fn the_deferred_send_path_seals_the_byte_identical_wire_and_receipt_as_inline() {
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
                &mut test_fill_entropy,
                &mut announce_buf,
            )
            .written_len();

        let mut inline_state = hearer();
        hear_announce(&mut inline_state, &announce_buf[..announce_len], arrival());
        let mut inline_buf = [0u8; BROADCAST_MTU];
        let inline = inline_state
            .write_commanded_send_single_packet(
                CommandId(7),
                &send_of(b"hello-deferred"),
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut inline_buf,
            )
            .dispatched();

        let mut deferred_state = hearer();
        hear_announce(
            &mut deferred_state,
            &announce_buf[..announce_len],
            arrival(),
        );
        let SendSinglePacketPrepared::Owed(owed) = deferred_state
            .prepare_send_single_packet_deferred(
                CommandId(7),
                send_of(b"hello-deferred"),
                InstantMillis(1_000),
                vector_send_entropy(),
            )
        else {
            panic!("a routed send prepares an encrypt obligation");
        };
        assert_eq!(
            deferred_state.receipts.len(),
            0,
            "prepare tracks nothing until the pool returns the scalars",
        );
        let (ephemeral_public, shared) =
            crate::crypto::x25519_keys_for_seal(&owed.ephemeral_secret, &owed.dh_target);
        let mut deferred_buf = [0u8; BROADCAST_MTU];
        let FinishSendSinglePacketOutcome::Written(deferred) = deferred_state
            .finish_send_single_packet_deferred(owed, ephemeral_public, shared, &mut deferred_buf)
        else {
            panic!("the finished seal writes the packet");
        };

        assert_eq!(
            &deferred_buf[..deferred.wire_len],
            &inline_buf[..inline.wire_len],
            "the pooled scalar mults seal the exact same wire bytes as the inline encrypt",
        );
        assert_eq!(deferred.fire_on, inline.fire_on);
        assert_eq!(
            deferred_state.receipts.len(),
            1,
            "finish tracks exactly the one receipt the inline path would have",
        );
    }

    #[test]
    fn a_send_with_no_route_is_rejected() {
        let mut state = hearer();
        let send = send_of(b"into-the-void");
        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::SendSinglePacket(send),
                },
                &[],
            ),
            CommandOutcome::SendSinglePacketRejected {
                id: CommandId(7),
                rejection: SendSinglePacketRejection::NoRouteToDestination,
            },
        );
        assert_eq!(state.receipts.len(), 0);
    }

    #[test]
    fn a_multi_hop_route_with_no_relay_to_address_is_not_directly_reachable() {
        let mut state = hearer();
        let mut relayed = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        relayed[1] = 1;
        hear_announce(&mut state, &relayed, arrival());

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::SendSinglePacket(send_of(b"too-far")),
                },
                &[],
            ),
            CommandOutcome::SendSinglePacketRejected {
                id: CommandId(7),
                rejection: SendSinglePacketRejection::NotDirectlyReachable,
            },
        );
    }

    #[test]
    fn a_send_to_a_multi_hop_destination_is_addressed_at_its_relay() {
        let mut state = hearer();
        hear_announce(
            &mut state,
            &bytes_from_hex(RNS_1_3_5_RETRANSMITTED_ANNOUNCE),
            arrival(),
        );
        let send = send_of(b"ratchet-parity");

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::SendSinglePacket(send.clone()),
                },
                &[],
            ),
            CommandOutcome::OwesSendSinglePacket {
                id: CommandId(7),
                send: send.clone(),
            },
        );

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single_packet(
                CommandId(7),
                &send,
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .dispatched();

        assert_eq!(
            &buf[..dispatch.wire_len],
            bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET_VIA_TRANSPORT).as_slice(),
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
                &mut test_fill_entropy,
                &mut announce_buf,
            )
            .written_len();

        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());
        let send = send_of(b"loopback-hello");

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single_packet(
                CommandId(7),
                &send,
                InstantMillis(1_000),
                {
                    let mut bytes = [0x77u8; SendSinglePacketEntropy::LEN];
                    bytes[32..].fill(0x0B);
                    SendSinglePacketEntropy::new(bytes)
                },
                &mut buf,
            )
            .dispatched();

        let mut wire = buf[..dispatch.wire_len].to_vec();
        assert_eq!(
            peer.ingest_packet_with(
                plain_data_packet(&mut wire),
                &mut |_| {},
                &transporting_interfaces(),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: peer_destination(),
                    context: crate::wire::WireContext::None,
                    plaintext: b"loopback-hello",
                    arrived_at: InstantMillis(1_000),
                    source_interface: crate::interfaces::InterfaceId::new([0x07; 8]),
                }),
                proof: crate::routing::proof::ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_ninth_send_culls_the_stalest_receipt() {
        let mut state = hearer();
        hear_announce(
            &mut state,
            &bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE),
            arrival(),
        );

        let mut buf = [0u8; BROADCAST_MTU];
        for i in 1..=8u64 {
            let dispatch = state
                .write_commanded_send_single_packet(
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
            .write_commanded_send_single_packet(
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
                kind: ReceiptKind::SendSinglePacket,
            }),
        );
        assert_eq!(state.receipts.len(), 8);
    }

    #[test]
    fn a_python_minted_proof_settles_the_tracked_send_with_its_rtt() {
        use crate::engine::{PacketReceiptDelivered, ProofIngest};

        let (mut state, wire) = unratcheted_neighbor_with_a_tracked_send(b"proof-parity", 1_000);
        assert_eq!(wire, bytes_from_hex(RNS_1_3_5_SEALED_FOR_PROOF));

        let mut proof = bytes_from_hex(RNS_1_3_5_IMPLICIT_PROOF);
        assert_eq!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_250),
                    source_interface: arrival(),
                    bytes: &mut proof,
                },
                &mut |_| {},
                &transporting_interfaces(),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(ProofIngest::SendSinglePacketDelivered {
                id: CommandId(7),
                delivered: PacketReceiptDelivered {
                    rtt: crate::units::RttMillis::new(250),
                },
            }),
        );
        assert_eq!(state.receipts.len(), 0);

        let mut replay = bytes_from_hex(RNS_1_3_5_IMPLICIT_PROOF);
        assert_eq!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_300),
                    source_interface: arrival(),
                    bytes: &mut replay,
                },
                &mut |_| {},
                &transporting_interfaces(),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(ProofIngest::Ignored),
            "settlement removed the receipt, so a replayed proof finds nothing",
        );
    }

    #[test]
    fn an_explicit_proof_settles_the_send_too() {
        use crate::crypto::{ed25519_sign, Ed25519SecretKey};
        use crate::engine::{PacketReceiptDelivered, ProofIngest};
        use crate::routing::proof::EXPLICIT_PROOF_PAYLOAD_LEN;

        let (mut state, wire) = unratcheted_neighbor_with_a_tracked_send(b"explicitly", 2_000);
        let proven = PacketHash::of_wire_packet(&wire).unwrap();
        let signature = ed25519_sign(&Ed25519SecretKey::new([0x11; 32]), proven.as_bytes());

        let mut payload = [0u8; EXPLICIT_PROOF_PAYLOAD_LEN];
        payload[..32].copy_from_slice(proven.as_bytes());
        payload[32..].copy_from_slice(&signature.0);
        let mut packet = proof_packet(&payload, &proven);

        assert_eq!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(2_500),
                    source_interface: arrival(),
                    bytes: &mut packet,
                },
                &mut |_| {},
                &transporting_interfaces(),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(ProofIngest::SendSinglePacketDelivered {
                id: CommandId(7),
                delivered: PacketReceiptDelivered {
                    rtt: crate::units::RttMillis::new(500),
                },
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
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_250),
                    source_interface: arrival(),
                    bytes: &mut packet,
                },
                &mut |_| {},
                &transporting_interfaces(),
                &mut |_| {},
                None,
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
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: arrival(),
                    bytes: &mut packet,
                },
                &mut |_| {},
                &transporting_interfaces(),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(ProofIngest::Ignored),
        );
    }

    #[test]
    fn a_timed_out_send_pops_once_for_its_settlement() {
        let mut state = hearer();
        hear_announce(
            &mut state,
            &bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE),
            arrival(),
        );
        let mut buf = [0u8; BROADCAST_MTU];
        state
            .write_commanded_send_single_packet(
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
                kind: ReceiptKind::SendSinglePacket,
            }),
        );
        assert_eq!(state.pop_timed_out_receipt(InstantMillis(13_000)), None);
        assert_eq!(state.receipts.len(), 0);
    }
}
