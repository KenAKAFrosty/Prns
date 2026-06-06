use crate::crypto::{X25519PublicKey, X25519SecretKey};
use crate::engine::commands::{CommandId, CommandOutcome, SendSingle, SendSingleError};
use crate::engine::receipts::{CulledReceipt, OutstandingReceipt};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::{EncryptError, RemoteIdentity, ENCRYPTION_IV_LEN};
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;
use crate::routing::storage::EngineStorage;
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

impl<S: EngineStorage> EngineState<S> {
    pub(crate) fn ingest_send_single(&self, id: CommandId, send: SendSingle) -> CommandOutcome {
        let Some(retained) = self.routing_table.retained_announce_for(&send.destination) else {
            return CommandOutcome::SendSingleRejected {
                id,
                error: SendSingleError::NoRouteToDestination,
            };
        };
        if retained.hops > 1 {
            return CommandOutcome::SendSingleRejected {
                id,
                error: SendSingleError::NotDirectlyReachable,
            };
        }
        CommandOutcome::OwesSendSingle { id, send }
    }

    /// Seal `send`'s payload to the peer's announced ratchet (identity key
    /// when it never announced one — RNS 1.3.1 `Destination.encrypt`), frame
    /// it directly into `buf`, and track the receipt that will settle `id`.
    pub fn write_commanded_send_single(
        &mut self,
        id: CommandId,
        send: &SendSingle,
        now: InstantMillis,
        entropy: SendSingleEntropy,
        buf: &mut [u8],
    ) -> Result<SendSingleDispatch, WriteSendSingleError> {
        let retained = self
            .routing_table
            .retained_announce_for(&send.destination)
            .ok_or(WriteSendSingleError::RouteVanished)?;
        let hops = retained.hops;
        let fire_on = retained.receiving_interface;
        let public_keys = retained.announce.public_keys;
        let maybe_ratchet = retained.announce.maybe_ratchet;

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination: send.destination,
            context: WireContext::None,
        };
        let header_len = header
            .write(buf)
            .map_err(|_| WriteSendSingleError::Serialize)?;

        let remote = RemoteIdentity::from_public_keys(public_keys.encryption, public_keys.signing);
        let (ephemeral_secret, iv) = entropy.into_parts();
        let sealed = match maybe_ratchet {
            Some(ratchet) => remote
                .encrypt_to_ratchet(
                    &X25519PublicKey(*ratchet.as_bytes()),
                    &ephemeral_secret,
                    &iv,
                    &send.payload,
                    &mut buf[header_len..],
                )
                .map_err(WriteSendSingleError::Seal)?,
            None => remote
                .encrypt(
                    &ephemeral_secret,
                    &iv,
                    &send.payload,
                    &mut buf[header_len..],
                )
                .map_err(WriteSendSingleError::Seal)?,
        };
        let wire_len = header_len + sealed;

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
            peer_signing_key: public_keys.signing,
            sent_at: now,
            timeout_at,
        });

        Ok(SendSingleDispatch {
            wire_len,
            fire_on,
            culled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{
        AnnounceIngest, CommandOutcome, EngineCommand, IngestPacketOutcome, IssuedCommand,
        NextScheduledEngineWork, RatchetPolicy, SendSinglePayload,
    };
    use crate::interfaces::InboundPacket;
    use crate::routing::delivery::{Delivery, SingleDelivery};
    use crate::wire::{DestinationHash, MTU};

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
        let mut raw = wire.to_vec();
        let outcome = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: arrival,
                bytes: &mut raw,
            },
            TEST_ENTROPY,
        );
        assert_eq!(
            outcome,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted),
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

    #[test]
    fn a_send_to_a_ratcheted_neighbor_reproduces_the_rns_1_3_1_wire() {
        let mut state = hearer();
        hear_announce(&mut state, &hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE), arrival());
        let send = send_of(b"ratchet-parity");

        assert_eq!(
            state.ingest_command(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::SendSingle(send.clone()),
            }),
            CommandOutcome::OwesSendSingle {
                id: CommandId(7),
                send: send.clone(),
            },
        );

        let mut buf = [0u8; MTU];
        let dispatch = state
            .write_commanded_send_single(
                CommandId(7),
                &send,
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .unwrap();

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
        let mut announce_buf = [0u8; MTU];
        let announce_len = announcer
            .write_due_self_announce(
                InstantMillis(100),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .unwrap()
            .unwrap();

        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());
        let send = send_of(b"hello-by-key");

        let mut buf = [0u8; MTU];
        let dispatch = state
            .write_commanded_send_single(
                CommandId(7),
                &send,
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .unwrap();

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
            state.ingest_command(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::SendSingle(send),
            }),
            CommandOutcome::SendSingleRejected {
                id: CommandId(7),
                error: SendSingleError::NoRouteToDestination,
            },
        );
        assert_eq!(state.receipts.len(), 0);
    }

    #[test]
    fn a_multi_hop_destination_is_not_directly_reachable_yet() {
        let mut state = hearer();
        let mut relayed = hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE);
        relayed[1] = 1;
        hear_announce(&mut state, &relayed, arrival());

        assert_eq!(
            state.ingest_command(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::SendSingle(send_of(b"too-far")),
            }),
            CommandOutcome::SendSingleRejected {
                id: CommandId(7),
                error: SendSingleError::NotDirectlyReachable,
            },
        );
    }

    #[test]
    fn a_sent_packet_round_trips_into_the_peer_engine() {
        let mut peer = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let mut announce_buf = [0u8; MTU];
        let announce_len = peer
            .write_due_self_announce(
                InstantMillis(100),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .unwrap()
            .unwrap();

        let mut state = hearer();
        hear_announce(&mut state, &announce_buf[..announce_len], arrival());
        let send = send_of(b"loopback-hello");

        let mut buf = [0u8; MTU];
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
            .unwrap();

        let mut wire = buf[..dispatch.wire_len].to_vec();
        assert_eq!(
            peer.ingest_packet(plain_data_packet(&mut wire), TEST_ENTROPY),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: peer_destination(),
                    context: crate::wire::WireContext::None,
                    plaintext: b"loopback-hello",
                    arrived_at: InstantMillis(1_000),
                    source_interface: crate::interfaces::InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );
    }

    #[test]
    fn a_send_schedules_the_receipt_timeout_into_next_wakeup() {
        let mut state = hearer();
        hear_announce(&mut state, &hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE), arrival());

        let mut buf = [0u8; MTU];
        let dispatch = state
            .write_commanded_send_single(
                CommandId(7),
                &send_of(b"timed"),
                InstantMillis(1_000),
                vector_send_entropy(),
                &mut buf,
            )
            .unwrap();
        assert_eq!(dispatch.culled, None);

        let _ = tick_capture(&mut state, InstantMillis(9_000));
        assert_eq!(
            state.next_wakeup(InstantMillis(9_500)),
            NextScheduledEngineWork::At(InstantMillis(13_000)),
            "one hop: 6s first-hop + 6s per-hop from sent_at 1_000",
        );
        assert_eq!(
            state.next_wakeup(InstantMillis(13_000)),
            NextScheduledEngineWork::Immediate,
        );
    }

    #[test]
    fn a_ninth_send_culls_the_stalest_receipt() {
        let mut state = hearer();
        hear_announce(&mut state, &hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE), arrival());

        let mut buf = [0u8; MTU];
        for i in 1..=8u64 {
            let dispatch = state
                .write_commanded_send_single(
                    CommandId(i),
                    &send_of(&[i as u8]),
                    InstantMillis(1_000 * i),
                    vector_send_entropy(),
                    &mut buf,
                )
                .unwrap();
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
            .unwrap();
        assert_eq!(
            dispatch.culled,
            Some(crate::engine::receipts::CulledReceipt {
                command_id: CommandId(1),
            }),
        );
        assert_eq!(state.receipts.len(), 8);
    }
}
