use crate::crypto::{x25519_keys_for_seal, X25519PublicKey, X25519SecretKey, X25519SharedSecret};
use crate::engine::{
    CommandId, CommandOutcome, SendSinglePacket, SendSinglePacketPayload, SendSinglePacketRejection,
};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::{
    seal_finish, EncryptError, IdentityHash, IdentitySigningPublicKey, RemoteIdentity,
    ENCRYPTION_IV_LEN,
};
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{CulledReceipt, OutstandingReceipt, ReceiptKind};
use crate::routing::routes::RouteEvidenceHandle;
use crate::routing::timing::{single_packet_timeout_ms, FirstHopTiming};
use crate::routing::NextHop;
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader,
};

/// RNS 1.4.2 `Reticulum.DEFAULT_PER_HOP_TIMEOUT` (6s), serving both as the first-hop fallback (`Transport.first_hop_timeout` without bitrate data) and the per-hop increment (`Packet.TIMEOUT_PER_HOP`).
pub use crate::routing::timing::{
    DEFAULT_FIRST_HOP_TIMEOUT_MS, DEFAULT_PER_HOP_TIMEOUT_MS, DEFAULT_PER_HOP_TIMEOUT_SECONDS,
};

pub const SEND_SINGLE_ENTROPY_LEN: usize = 32 + ENCRYPTION_IV_LEN;

/// Move-only and never shown; consuming it seals exactly one packet, so one draw can never key two.
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
    pub wire_bytes: usize,
    pub fire_on: InterfaceId,
    pub culled: Option<CulledReceipt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSinglePacketWriteError {
    RouteVanished,
    Seal(EncryptError),
    Serialize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSinglePacketWriteRejection {
    RouteVanished,
    Serialize,
}

impl From<SendSinglePacketWriteRejection> for SendSinglePacketWriteError {
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

/// Everything the receipt row and dispatch need that the seal itself does not produce.
struct TrackedSend {
    command_id: CommandId,
    peer_signing_key: IdentitySigningPublicKey,
    sent_at: InstantMillis,
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
    Failed(SendSinglePacketWriteError),
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

    /// Seals to the peer's announced ratchet, identity key when it never announced one (RNS 1.4.2 `Destination.encrypt`).
    pub fn write_commanded_send_single_packet(
        &mut self,
        id: CommandId,
        send: &SendSinglePacket,
        now: InstantMillis,
        entropy: SendSinglePacketEntropy,
        buf: &mut [u8],
    ) -> SendSinglePacketWriteOutcome {
        self.write_commanded_send_single_packet_with_interfaces(
            id,
            send,
            now,
            entropy,
            AttachedInterfaces::new(&[]),
            buf,
        )
    }

    pub fn write_commanded_send_single_packet_with_interfaces(
        &mut self,
        id: CommandId,
        send: &SendSinglePacket,
        now: InstantMillis,
        entropy: SendSinglePacketEntropy,
        interfaces: AttachedInterfaces<'_>,
        buf: &mut [u8],
    ) -> SendSinglePacketWriteOutcome {
        self.write_commanded_send_single_packet_with_timing(
            id,
            send,
            now,
            entropy,
            FirstHopTiming {
                interfaces,
                shared_instance_floor_ms: None,
            },
            buf,
        )
    }

    pub fn write_commanded_send_single_packet_with_timing(
        &mut self,
        id: CommandId,
        send: &SendSinglePacket,
        now: InstantMillis,
        entropy: SendSinglePacketEntropy,
        timing: FirstHopTiming<'_>,
        buf: &mut [u8],
    ) -> SendSinglePacketWriteOutcome {
        use SendSinglePacketWriteOutcome::{Failed, Rejected, Written};

        let Some(plan) = self.gather_send_single_plan(send, now, timing) else {
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

        Written(self.track_sealed_send_single_packet(
            &plan.header,
            buf,
            header_len,
            sealed_len,
            TrackedSend {
                command_id: id,
                peer_signing_key: plan.peer_signing_key,
                sent_at: now,
                timeout_at: plan.timeout_at,
                fire_on: plan.fire_on,
            },
        ))
    }

    /// RNS 1.4.2 `Transport.outbound`: a destination more than one hop out rides transport, addressed at the relay that announced it; anything nearer is broadcast at the destination itself.
    /// The reference's behind-a-shared-instance inject (`hops == 1`) has no analog here: an engine is always its own instance.
    /// Intentional deviation: the reference also slides the path-table clock (`IDX_PT_TIMESTAMP`) on every transport-injected send, so merely sending keeps a route alive; ours slides on evidence only (link activation, returned proof).
    fn gather_send_single_plan(
        &self,
        send: &SendSinglePacket,
        now: InstantMillis,
        timing: FirstHopTiming<'_>,
    ) -> Option<SendSinglePacketPlan> {
        let stored = self.routing_table.stored_announce_for(&send.destination)?;
        let hops = stored.hops;
        let fire_on = stored.receiving_interface;
        let public_keys = stored.announce.public_keys;
        let ratchet = stored.announce.ratchet;

        let (propagation, transport_id) = match stored.next_hop {
            NextHop::Via(via) if hops > 1 => (PropagationType::Transport, Some(via)),
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
            address: send.destination.to_address(),
            context: WireContext::None,
        };
        let dh_target = match ratchet {
            Some(ratchet) => X25519PublicKey(*ratchet.as_bytes()),
            None => *public_keys.encryption.as_x25519(),
        };
        let recipient_identity_hash =
            RemoteIdentity::from_public_keys(public_keys.encryption, public_keys.signing)
                .identity_hash();
        let bitrate = timing
            .interfaces
            .descriptor_for(fire_on)
            .filter(|descriptor| descriptor.capabilities.allows_transmit())
            .map(|descriptor| descriptor.bitrate);
        let computed = single_packet_timeout_ms(hops, bitrate);
        let floor = timing.shared_instance_floor_ms.map(|first_hop| {
            first_hop.saturating_add(DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(hops)))
        });
        let timeout_at = InstantMillis(
            now.0
                .saturating_add(floor.map_or(computed, |floor| computed.max(floor))),
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

    /// Captures attribution from the authoritative route at dispatch, never at deferred-crypto
    /// preparation. A changed interface or next hop still permits the already-sealed send, but its
    /// later proof cannot be credited to a route that did not carry that wire plan.
    fn route_evidence_for_send_single_dispatch(
        &self,
        header: &WirePacketHeader,
        fire_on: InterfaceId,
    ) -> Option<RouteEvidenceHandle> {
        let destination = DestinationHash::from_address(header.address);
        let stored = self.routing_table.stored_announce_for(&destination)?;
        if stored.receiving_interface != fire_on {
            return None;
        }
        let (propagation, transport_id) = match stored.next_hop {
            NextHop::Via(via) if stored.hops > 1 => (PropagationType::Transport, Some(via)),
            _ => (PropagationType::Broadcast, None),
        };
        if header.propagation != propagation || header.transport_id != transport_id {
            return None;
        }
        self.routing_table.route_evidence_handle_for(&destination)
    }

    /// `&self` and side-effect-free: nothing is tracked until the scalars are back, so an abandoned obligation leaves no orphan receipt.
    pub fn prepare_send_single_packet_deferred(
        &self,
        id: CommandId,
        send: SendSinglePacket,
        now: InstantMillis,
        entropy: SendSinglePacketEntropy,
    ) -> SendSinglePacketPrepared {
        self.prepare_send_single_packet_deferred_with_interfaces(
            id,
            send,
            now,
            entropy,
            AttachedInterfaces::new(&[]),
        )
    }

    pub fn prepare_send_single_packet_deferred_with_interfaces(
        &self,
        id: CommandId,
        send: SendSinglePacket,
        now: InstantMillis,
        entropy: SendSinglePacketEntropy,
        interfaces: AttachedInterfaces<'_>,
    ) -> SendSinglePacketPrepared {
        self.prepare_send_single_packet_deferred_with_timing(
            id,
            send,
            now,
            entropy,
            FirstHopTiming {
                interfaces,
                shared_instance_floor_ms: None,
            },
        )
    }

    pub fn prepare_send_single_packet_deferred_with_timing(
        &self,
        id: CommandId,
        send: SendSinglePacket,
        now: InstantMillis,
        entropy: SendSinglePacketEntropy,
        timing: FirstHopTiming<'_>,
    ) -> SendSinglePacketPrepared {
        let send = match self.ingest_send_single_packet(id, send) {
            CommandOutcome::OwesSendSinglePacket { send, .. } => send,
            CommandOutcome::SendSinglePacketRejected { id, rejection } => {
                return SendSinglePacketPrepared::Rejected { id, rejection }
            }
            _ => return SendSinglePacketPrepared::RouteVanished { id },
        };
        let Some(plan) = self.gather_send_single_plan(&send, now, timing) else {
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

    /// The same bytes and row the inline path produces; the only difference is where the X25519 ran.
    pub fn finish_send_single_packet_deferred(
        &mut self,
        owed: EncryptOwed,
        ephemeral_public: X25519PublicKey,
        shared: X25519SharedSecret,
        buf: &mut [u8],
    ) -> FinishSendSinglePacketOutcome {
        let Ok(header_len) = owed.header.write(buf) else {
            return FinishSendSinglePacketOutcome::Failed(SendSinglePacketWriteError::Serialize);
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
                return FinishSendSinglePacketOutcome::Failed(SendSinglePacketWriteError::Seal(
                    error,
                ))
            }
        };

        FinishSendSinglePacketOutcome::Written(self.track_sealed_send_single_packet(
            &owed.header,
            buf,
            header_len,
            sealed_len,
            TrackedSend {
                command_id: owed.command_id,
                peer_signing_key: owed.peer_signing_key,
                sent_at: owed.sent_at,
                timeout_at: owed.timeout_at,
                fire_on: owed.fire_on,
            },
        ))
    }

    /// The shared tail of the inline and deferred paths: one law for the receipt row and its dispatch, so the two seals cannot drift.
    fn track_sealed_send_single_packet(
        &mut self,
        header: &WirePacketHeader,
        buf: &[u8],
        header_len: usize,
        sealed_len: usize,
        tracked: TrackedSend,
    ) -> SendSinglePacketDispatch {
        let wire_bytes = header_len + sealed_len;
        let packet_hash = PacketHash::of_data_fields(
            DestinationType::Single,
            &header.address,
            WireContext::None,
            &buf[header_len..wire_bytes],
        );
        let route_evidence = self.route_evidence_for_send_single_dispatch(header, tracked.fire_on);
        let culled = self.receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: tracked.command_id,
            kind: ReceiptKind::SendSinglePacket { route_evidence },
            peer_signing_key: tracked.peer_signing_key,
            sent_at: tracked.sent_at,
            timeout_at: tracked.timeout_at,
        });
        SendSinglePacketDispatch {
            wire_bytes,
            fire_on: tracked.fire_on,
            culled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{
        AnnounceAppData, AnnounceIngest, AnnounceNow, AnnounceTarget, CommandOutcome,
        IngestPacketOutcome, IssuedCommand, PrnsCommand, RatchetPolicy, SendSinglePacketPayload,
    };
    use crate::interfaces::InboundPacket;
    use crate::interfaces::{AttachedInterfaces, BitrateBps};
    use crate::routing::delivery::receipts::ExpiredReceipt;
    use crate::routing::delivery::{Delivery, SingleDelivery};
    use crate::routing::routes::RouteResponsiveness;
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
            destination: DestinationHash::from_address(header.address),
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
            AttachedInterfaces::new(&transporting_interfaces()),
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

    fn replace_peer_route_path(
        state: &mut EngineState<TestStorageLayout>,
        receiving_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) {
        let (public_keys, dotted_name_hash, announce_id, ratchet, signature, app_data) = {
            let stored = state
                .routing_table
                .stored_announce_for(&peer_destination())
                .unwrap();
            (
                stored.announce.public_keys,
                stored.announce.dotted_name_hash,
                stored.announce.announce_id,
                stored.announce.ratchet,
                stored.announce.signature,
                stored.announce.app_data.to_vec(),
            )
        };
        let next_hop = NextHop::Direct;
        let evidence_id =
            state.route_evidence_id_for_update(&peer_destination(), receiving_interface, next_hop);
        let interfaces = transporting_interfaces();
        assert_eq!(
            state.routing_table.upsert_route(
                &crate::routing::AnnounceArrival {
                    announce: crate::routing::announce::Announce {
                        destination: peer_destination(),
                        public_keys,
                        dotted_name_hash,
                        announce_id,
                        ratchet,
                        signature,
                        app_data: &app_data,
                    },
                    hops: 1,
                    arrived_at,
                    receiving_interface,
                    next_hop,
                    is_path_response: false,
                },
                evidence_id,
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
            ),
            crate::routing::UpsertRouteOutcome::Updated,
        );
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
        (state, buf[..dispatch.wire_bytes].to_vec())
    }

    fn unratcheted_neighbor_with_a_prepared_send(
        payload: &[u8],
        sent_at: u64,
    ) -> (EngineState<TestStorageLayout>, EncryptOwed) {
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
        let SendSinglePacketPrepared::Owed(owed) = state.prepare_send_single_packet_deferred(
            CommandId(7),
            send_of(payload),
            InstantMillis(sent_at),
            vector_send_entropy(),
        ) else {
            panic!("a routed send prepares an encrypt obligation");
        };
        (state, owed)
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
            &buf[..dispatch.wire_bytes],
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
            address: proven.proof_destination().to_address(),
            context: WireContext::None,
        };
        let mut bytes = std::vec![0u8; crate::wire::HEADER_MIN_LEN + payload.len()];
        let written = header.write(&mut bytes).unwrap();
        bytes[written..].copy_from_slice(payload);
        bytes
    }

    fn resolve_deferred_proof(
        state: &mut EngineState<TestStorageLayout>,
        proof: &[u8],
        arrived_at: InstantMillis,
    ) -> crate::routing::proof::DeferredProof {
        let (header, payload) = WirePacketHeader::parse(proof).unwrap();
        state
            .settle_receipt_proof_deferred(
                payload,
                &DestinationHash::from_address(header.address),
                PacketHash::of_wire_packet(proof).unwrap(),
                arrived_at,
            )
            .expect("the proof resolves its outstanding receipt")
    }

    #[test]
    fn a_send_to_a_ratcheted_neighbor_reproduces_the_rns_1_4_2_wire() {
        let mut state = hearer();
        hear_announce(
            &mut state,
            &bytes_from_hex(RNS_1_4_2_RATCHETED_ANNOUNCE),
            arrival(),
        );
        let send = send_of(b"ratchet-parity");

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: PrnsCommand::SendSinglePacket(send.clone()),
                },
                AttachedInterfaces::new(&[]),
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
            &buf[..dispatch.wire_bytes],
            bytes_from_hex(RNS_1_4_2_SEALED_TO_RATCHET).as_slice()
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
        assert_eq!(&buf[..dispatch.wire_bytes], expected.as_slice());
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
            &deferred_buf[..deferred.wire_bytes],
            &inline_buf[..inline.wire_bytes],
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
                    command: PrnsCommand::SendSinglePacket(send),
                },
                AttachedInterfaces::new(&[]),
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
        let mut relayed = bytes_from_hex(RNS_1_4_2_RATCHETED_ANNOUNCE);
        relayed[1] = 1;
        hear_announce(&mut state, &relayed, arrival());

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: PrnsCommand::SendSinglePacket(send_of(b"too-far")),
                },
                AttachedInterfaces::new(&[]),
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
            &bytes_from_hex(RNS_1_4_2_RETRANSMITTED_ANNOUNCE),
            arrival(),
        );
        let send = send_of(b"ratchet-parity");

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: PrnsCommand::SendSinglePacket(send.clone()),
                },
                AttachedInterfaces::new(&[]),
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
            &buf[..dispatch.wire_bytes],
            bytes_from_hex(RNS_1_4_2_SEALED_TO_RATCHET_VIA_TRANSPORT).as_slice(),
            "the sealed packet rides transport addressed at the announcing relay",
        );
        assert_eq!(dispatch.fire_on, arrival());
        assert_eq!(dispatch.culled, None);
        assert_eq!(state.receipts.len(), 1);
    }

    #[test]
    fn a_via_route_one_hop_out_is_broadcast_at_the_destination() {
        let mut state = hearer();
        let mut relayed = bytes_from_hex(RNS_1_4_2_RETRANSMITTED_ANNOUNCE);
        relayed[1] = 0;
        hear_announce(&mut state, &relayed, arrival());

        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = state
            .write_commanded_send_single_packet(
                CommandId(7),
                &send_of(b"nearby-after-all"),
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .dispatched();

        let (header, _) = WirePacketHeader::parse(&buf[..dispatch.wire_bytes])
            .expect("the dispatched send is a parseable wire packet");
        assert_eq!(
            header.propagation,
            PropagationType::Broadcast,
            "RNS 1.4.2 Transport.outbound only rides transport past one hop",
        );
        assert_eq!(header.transport_id, None);
        assert_eq!(header.address, peer_destination().to_address());
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

        let mut wire = buf[..dispatch.wire_bytes].to_vec();
        assert_eq!(
            peer.ingest_packet_with(
                plain_data_packet(&mut wire),
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: peer_destination(),
                    context: crate::wire::WireContext::None,
                    plaintext: b"loopback-hello",
                    opened_by: crate::identity::OpenedBy::Ratchet(
                        crate::crypto::ratchets::RatchetId::of_secret(
                            &crate::crypto::X25519SecretKey::new([0xAB; 32]),
                        ),
                    ),
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
            &bytes_from_hex(RNS_1_4_2_RATCHETED_ANNOUNCE),
            arrival(),
        );
        let route_evidence = state
            .routing_table
            .route_evidence_handle_for(&peer_destination())
            .unwrap();

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
                kind: ReceiptKind::SendSinglePacket {
                    route_evidence: Some(route_evidence),
                },
            }),
        );
        assert_eq!(state.receipts.len(), 8);
    }

    #[test]
    fn a_python_minted_proof_settles_the_tracked_send_with_its_rtt() {
        use crate::engine::{DeliveryEvidence, DeliveryProof, PacketReceiptDelivered, ProofIngest};

        let (mut state, wire) = unratcheted_neighbor_with_a_tracked_send(b"proof-parity", 1_000);
        assert_eq!(wire, bytes_from_hex(RNS_1_4_2_SEALED_FOR_PROOF));
        state
            .routing_table
            .mark_responsiveness(&peer_destination(), RouteResponsiveness::Unresponsive);

        let mut proof = bytes_from_hex(RNS_1_4_2_IMPLICIT_PROOF);
        let proof_packet_hash = PacketHash::of_wire_packet(&proof).unwrap();
        assert_eq!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_250),
                    source_interface: arrival(),
                    bytes: &mut proof,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(ProofIngest::SendSinglePacketDelivered {
                id: CommandId(7),
                delivered: PacketReceiptDelivered {
                    rtt: crate::units::RttMillis::new(250),
                    evidence: DeliveryEvidence::Proof(DeliveryProof::Implicit(proof_packet_hash)),
                },
            }),
        );
        assert_eq!(state.receipts.len(), 0);
        let row = state.routing_table.path_row(&peer_destination()).unwrap();
        assert_eq!(row.last_route_activity_at, InstantMillis(1_250));
        assert_eq!(row.responsiveness, RouteResponsiveness::Responsive);

        state
            .routing_table
            .mark_responsiveness(&peer_destination(), RouteResponsiveness::Unresponsive);

        let mut replay = bytes_from_hex(RNS_1_4_2_IMPLICIT_PROOF);
        assert_eq!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_300),
                    source_interface: arrival(),
                    bytes: &mut replay,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(ProofIngest::Ignored),
            "settlement removed the receipt, so a replayed proof finds nothing",
        );
        let row = state.routing_table.path_row(&peer_destination()).unwrap();
        assert_eq!(row.last_route_activity_at, InstantMillis(1_250));
        assert_eq!(row.responsiveness, RouteResponsiveness::Unresponsive);
    }

    #[test]
    fn deferred_proof_evidence_waits_for_verification_and_uses_arrival_time() {
        use crate::crypto::{ed25519_sign, Ed25519SecretKey, Ed25519Verifier};
        use crate::engine::ProofIngest;

        let (mut state, wire) = unratcheted_neighbor_with_a_tracked_send(b"proof-parity", 1_000);
        let proven = PacketHash::of_wire_packet(&wire).unwrap();
        state
            .routing_table
            .mark_responsiveness(&peer_destination(), RouteResponsiveness::Unresponsive);

        let forged_signature = ed25519_sign(&Ed25519SecretKey::new([0x99; 32]), proven.as_bytes());
        let forged_packet = proof_packet(&forged_signature.0, &proven);
        let forged = resolve_deferred_proof(&mut state, &forged_packet, InstantMillis(1_200));
        assert!(
            Ed25519Verifier::new(forged.signing_key.as_ed25519())
                .unwrap()
                .verify(forged.packet_hash.as_bytes(), &forged.signature)
                .is_err(),
            "the worker rejects the forged proof",
        );
        assert_eq!(state.receipts.len(), 1);
        let row = state.routing_table.path_row(&peer_destination()).unwrap();
        assert_eq!(row.last_route_activity_at, InstantMillis(0));
        assert_eq!(row.responsiveness, RouteResponsiveness::Unresponsive);

        let proof = bytes_from_hex(RNS_1_4_2_IMPLICIT_PROOF);
        let verified = resolve_deferred_proof(&mut state, &proof, InstantMillis(1_250));
        assert!(Ed25519Verifier::new(verified.signing_key.as_ed25519())
            .unwrap()
            .verify(verified.packet_hash.as_bytes(), &verified.signature)
            .is_ok(),);
        let ProofIngest::SendSinglePacketDelivered { id, .. } = verified.ingest else {
            panic!("the deferred proof belongs to the ordinary send");
        };

        let row = state.routing_table.path_row(&peer_destination()).unwrap();
        assert_eq!(row.last_route_activity_at, InstantMillis(0));
        assert_eq!(row.responsiveness, RouteResponsiveness::Unresponsive);
        assert_eq!(state.receipts.len(), 1, "resolve is read-only");
        assert_eq!(
            state.settle_resolved_receipt_proof(id, &verified.packet_hash, verified.arrived_at,),
            crate::engine::ResolvedReceiptSettlement::Settled,
        );

        let row = state.routing_table.path_row(&peer_destination()).unwrap();
        assert_eq!(
            row.last_route_activity_at,
            InstantMillis(1_250),
            "worker latency cannot replace the packet's arrival timestamp",
        );
        assert_eq!(row.responsiveness, RouteResponsiveness::Responsive);
        assert!(state.receipts.is_empty());
        assert_eq!(
            state.settle_resolved_receipt_proof(id, &verified.packet_hash, InstantMillis(1_300),),
            crate::engine::ResolvedReceiptSettlement::NoMatchingReceipt,
            "a duplicate worker result names why it cannot settle again",
        );
    }

    #[test]
    fn a_deferred_send_with_a_changed_route_plan_tracks_no_route_credit() {
        let (mut state, owed) = unratcheted_neighbor_with_a_prepared_send(b"proof-parity", 1_000);
        let (ephemeral_public, shared) =
            crate::crypto::x25519_keys_for_seal(&owed.ephemeral_secret, &owed.dh_target);
        replace_peer_route_path(
            &mut state,
            InterfaceId::new([0xB2; 8]),
            InstantMillis(1_100),
        );
        state
            .routing_table
            .mark_responsiveness(&peer_destination(), RouteResponsiveness::Unresponsive);

        let mut wire = [0u8; BROADCAST_MTU];
        let FinishSendSinglePacketOutcome::Written(dispatch) =
            state.finish_send_single_packet_deferred(owed, ephemeral_public, shared, &mut wire)
        else {
            panic!("the saved plan still sends normally");
        };
        assert_eq!(dispatch.fire_on, arrival(), "the saved plan owns this send");
        assert_eq!(
            &wire[..dispatch.wire_bytes],
            bytes_from_hex(RNS_1_4_2_SEALED_FOR_PROOF).as_slice(),
        );

        let mut proof = bytes_from_hex(RNS_1_4_2_IMPLICIT_PROOF);
        assert!(matches!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_250),
                    source_interface: arrival(),
                    bytes: &mut proof,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(crate::engine::ProofIngest::SendSinglePacketDelivered {
                id: CommandId(7),
                ..
            }),
        ));
        let row = state.routing_table.path_row(&peer_destination()).unwrap();
        assert_eq!(row.last_route_activity_at, InstantMillis(0));
        assert_eq!(row.responsiveness, RouteResponsiveness::Unresponsive);
    }

    #[test]
    fn an_explicit_proof_settles_the_send_too() {
        use crate::crypto::{ed25519_sign, Ed25519SecretKey};
        use crate::engine::{DeliveryEvidence, DeliveryProof, PacketReceiptDelivered, ProofIngest};
        use crate::routing::proof::EXPLICIT_PROOF_PAYLOAD_LEN;

        let (mut state, wire) = unratcheted_neighbor_with_a_tracked_send(b"explicitly", 2_000);
        let proven = PacketHash::of_wire_packet(&wire).unwrap();
        let signature = ed25519_sign(&Ed25519SecretKey::new([0x11; 32]), proven.as_bytes());

        let mut payload = [0u8; EXPLICIT_PROOF_PAYLOAD_LEN];
        payload[..32].copy_from_slice(proven.as_bytes());
        payload[32..].copy_from_slice(&signature.0);
        let mut packet = proof_packet(&payload, &proven);
        let proof_packet_hash = PacketHash::of_wire_packet(&packet).unwrap();

        assert_eq!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(2_500),
                    source_interface: arrival(),
                    bytes: &mut packet,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(ProofIngest::SendSinglePacketDelivered {
                id: CommandId(7),
                delivered: PacketReceiptDelivered {
                    rtt: crate::units::RttMillis::new(500),
                    evidence: DeliveryEvidence::Proof(DeliveryProof::Explicit(proof_packet_hash)),
                },
            }),
        );
        assert_eq!(state.receipts.len(), 0);
    }

    #[test]
    fn a_valid_proof_cannot_credit_a_replacement_route() {
        use crate::engine::{DeliveryEvidence, DeliveryProof, PacketReceiptDelivered, ProofIngest};

        let (mut state, _) = unratcheted_neighbor_with_a_tracked_send(b"proof-parity", 1_000);
        let original = state
            .routing_table
            .route_evidence_handle_for(&peer_destination())
            .unwrap();
        replace_peer_route_path(
            &mut state,
            InterfaceId::new([0xB2; 8]),
            InstantMillis(1_100),
        );
        let replacement = state
            .routing_table
            .route_evidence_handle_for(&peer_destination())
            .unwrap();
        assert_ne!(replacement.id, original.id);
        state
            .routing_table
            .mark_responsiveness(&peer_destination(), RouteResponsiveness::Unresponsive);

        let mut proof = bytes_from_hex(RNS_1_4_2_IMPLICIT_PROOF);
        let proof_packet_hash = PacketHash::of_wire_packet(&proof).unwrap();
        assert_eq!(
            state.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_250),
                    source_interface: arrival(),
                    bytes: &mut proof,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(ProofIngest::SendSinglePacketDelivered {
                id: CommandId(7),
                delivered: PacketReceiptDelivered {
                    rtt: crate::units::RttMillis::new(250),
                    evidence: DeliveryEvidence::Proof(DeliveryProof::Implicit(proof_packet_hash)),
                },
            }),
        );
        let row = state.routing_table.path_row(&peer_destination()).unwrap();
        assert_eq!(row.last_route_activity_at, InstantMillis(0));
        assert_eq!(row.responsiveness, RouteResponsiveness::Unresponsive);
    }

    #[test]
    fn a_forged_proof_leaves_the_send_outstanding() {
        use crate::crypto::{ed25519_sign, Ed25519SecretKey};
        use crate::engine::ProofIngest;

        let (mut state, wire) = unratcheted_neighbor_with_a_tracked_send(b"unforgeable", 1_000);
        state
            .routing_table
            .mark_responsiveness(&peer_destination(), RouteResponsiveness::Unresponsive);
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
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Proof(ProofIngest::Ignored),
        );
        assert_eq!(state.receipts.len(), 1, "the timeout still owns the send");
        let row = state.routing_table.path_row(&peer_destination()).unwrap();
        assert_eq!(row.last_route_activity_at, InstantMillis(0));
        assert_eq!(row.responsiveness, RouteResponsiveness::Unresponsive);
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
                AttachedInterfaces::new(&transporting_interfaces()),
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
            &bytes_from_hex(RNS_1_4_2_RATCHETED_ANNOUNCE),
            arrival(),
        );
        let route_evidence = state
            .routing_table
            .route_evidence_handle_for(&peer_destination())
            .unwrap();
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

        assert_eq!(state.receipts.pop_expired(InstantMillis(12_999)), None);
        assert_eq!(
            state.receipts.pop_expired(InstantMillis(13_000)),
            Some(ExpiredReceipt {
                command_id: CommandId(7),
                kind: ReceiptKind::SendSinglePacket {
                    route_evidence: Some(route_evidence),
                },
            }),
        );
        assert_eq!(state.receipts.pop_expired(InstantMillis(13_000)), None);
        assert_eq!(state.receipts.len(), 0);
    }

    #[test]
    fn a_single_receipt_uses_the_selected_egress_bitrate() {
        let mut state = hearer();
        hear_announce(
            &mut state,
            &bytes_from_hex(RNS_1_4_2_RATCHETED_ANNOUNCE),
            arrival(),
        );
        let mut selected = routable_descriptor(arrival());
        selected.bitrate = BitrateBps::guess(250);
        let mut unrelated = routable_descriptor(InterfaceId::new([0xB2; 8]));
        unrelated.bitrate = BitrateBps::guess(5);
        let interfaces = [selected, unrelated];
        let mut buf = [0u8; BROADCAST_MTU];

        state
            .write_commanded_send_single_packet_with_interfaces(
                CommandId(7),
                &send_of(b"slow-egress"),
                InstantMillis(1_000),
                vector_send_entropy(),
                AttachedInterfaces::new(&interfaces),
                &mut buf,
            )
            .dispatched();

        assert_eq!(state.receipts.pop_expired(InstantMillis(28_999)), None);
        assert_eq!(
            state
                .receipts
                .pop_expired(InstantMillis(29_000))
                .map(|expired| expired.command_id),
            Some(CommandId(7)),
        );
    }

    #[test]
    fn a_single_receipt_falls_back_when_the_selected_egress_cannot_transmit() {
        let mut state = hearer();
        hear_announce(
            &mut state,
            &bytes_from_hex(RNS_1_4_2_RATCHETED_ANNOUNCE),
            arrival(),
        );
        let mut selected = routable_descriptor(arrival());
        selected.bitrate = BitrateBps::guess(5);
        selected.capabilities.egress = crate::interfaces::EgressCapability::Disabled;
        let interfaces = [selected];
        let mut buf = [0u8; BROADCAST_MTU];

        state
            .write_commanded_send_single_packet_with_interfaces(
                CommandId(7),
                &send_of(b"disabled-egress"),
                InstantMillis(1_000),
                vector_send_entropy(),
                AttachedInterfaces::new(&interfaces),
                &mut buf,
            )
            .dispatched();

        assert_eq!(state.receipts.pop_expired(InstantMillis(12_999)), None);
        assert_eq!(
            state
                .receipts
                .pop_expired(InstantMillis(13_000))
                .map(|expired| expired.command_id),
            Some(CommandId(7)),
        );
    }
}
