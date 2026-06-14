//! The transmit side of a channel: RNS 1.3.1 `Channel.send`'s sequencing and
//! windowed reliability, ported to the engine's command/receipt grammar. A send
//! stamps the channel's next sequence onto an envelope, seals it under the link
//! key as a `CHANNEL`-context packet, and tracks it in the channel's outstanding
//! ring; the peer's proof (an explicit link proof addressed to the link) settles
//! it Delivered. The window bounds how many sends may be in flight unproven —
//! fixed at the RNS initial value here; its RTT-tiered growth is a later slice.

use crate::crypto::{ed25519_verify, Ed25519Signature};
use crate::engine::commands::{
    CommandId, CommandOutcome, Delivered, SendChannel, SendChannelError, SendChannelFailure,
    MAX_SEND_CHANNEL_BODY_LEN,
};
use crate::engine::{EngineState, InstantMillis};
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::links::channel::columns::{ChannelColumns, TxOutcome};
use crate::routing::links::channel::{write_envelope, ENVELOPE_HEADER_LEN};
use crate::routing::links::data::{write_link_packet, LinkDataError};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::routing::proof::EXPLICIT_PROOF_PAYLOAD_LEN;
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, DestinationType, WireContext, HEADER_MIN_LEN};

/// RNS 1.3.1 `Channel.WINDOW`: the initial number of unproven messages a channel
/// keeps in flight. Growth toward the RTT-tiered `WINDOW_MAX` is a later slice;
/// for now the window is fixed at this floor.
pub const CHANNEL_TX_WINDOW: usize = 2;

/// The scratch an outbound envelope needs before sealing: the 6-byte header plus
/// the largest body a channel message carries at the broadcast MTU.
const CHANNEL_PLAINTEXT_CAP: usize = ENVELOPE_HEADER_LEN + MAX_SEND_CHANNEL_BODY_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendChannelDispatch {
    pub wire_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendChannelWriteError {
    LinkVanished,
    Untrackable,
    WindowFull,
    Frame(LinkDataError),
}

impl<S: StorageLayout> EngineState<S> {
    /// Validate a channel send: the link must be ACTIVE and the channel's window
    /// must have room. The window check is read-only (a fresh channel has none
    /// outstanding); the actual sequencing and tracking happen in
    /// [`write_commanded_send_channel`](Self::write_commanded_send_channel).
    pub fn ingest_send_channel(&self, id: CommandId, send: SendChannel) -> CommandOutcome {
        match self.links.phase_for(&send.link_id) {
            None => CommandOutcome::SendChannelRejected {
                id,
                failure: SendChannelFailure::Rejected(SendChannelError::NoSuchLink),
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::SendChannelRejected {
                    id,
                    failure: SendChannelFailure::Rejected(SendChannelError::LinkNotActive),
                }
            }
            Some(LinkPhase::Active { .. }) => {
                let window_full = self.channels.index_of(&send.link_id).is_some_and(|index| {
                    self.channels.outstanding_count(index) >= CHANNEL_TX_WINDOW
                });
                if window_full {
                    CommandOutcome::SendChannelRejected {
                        id,
                        failure: SendChannelFailure::WindowFull,
                    }
                } else {
                    CommandOutcome::OwesSendChannel { id, send }
                }
            }
        }
    }

    /// Stamp the next sequence, frame and seal the envelope as a `CHANNEL` packet
    /// into `buf`, and track it in the channel's outstanding ring so the peer's
    /// proof can settle it. The sequence only advances once the message is
    /// tracked, so a failure leaves no gap for the receiver to stall on.
    pub fn write_commanded_send_channel(
        &mut self,
        id: CommandId,
        send: &SendChannel,
        now: InstantMillis,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<SendChannelDispatch, SendChannelWriteError> {
        let index = self
            .channels
            .ensure(&send.link_id)
            .map_err(|_| SendChannelWriteError::Untrackable)?;
        if self.channels.outstanding_count(index) >= CHANNEL_TX_WINDOW {
            return Err(SendChannelWriteError::WindowFull);
        }
        let sequence = self.channels.next_tx_sequence(index);

        let mut envelope = [0u8; CHANNEL_PLAINTEXT_CAP];
        let plaintext_len = write_envelope(send.message_type, sequence, &send.body, &mut envelope)
            .map_err(|_| SendChannelWriteError::Frame(LinkDataError::PayloadTooLong))?;

        let Some(LinkPhase::Active { key, mtu, .. }) = self.links.phase_for(&send.link_id) else {
            return Err(SendChannelWriteError::LinkVanished);
        };
        let wire_len = write_link_packet(
            &send.link_id,
            key,
            *mtu,
            WireContext::Channel,
            &envelope[..plaintext_len],
            iv,
            buf,
        )
        .map_err(SendChannelWriteError::Frame)?;

        let packet_hash = PacketHash::of_data_fields(
            DestinationType::Link,
            &DestinationHash::new(*send.link_id.as_bytes()),
            WireContext::Channel,
            &buf[HEADER_MIN_LEN..wire_len],
        );
        match self.channels.push_outstanding(index, packet_hash, id, now) {
            TxOutcome::Tracked => {
                self.channels.set_next_tx_sequence(index, sequence.next());
                Ok(SendChannelDispatch { wire_len })
            }
            TxOutcome::Full => Err(SendChannelWriteError::WindowFull),
        }
    }

    /// Settle a channel send against an arriving explicit proof addressed to
    /// `link_id`. Returns the settled command and its round trip, or `None` if
    /// the proof names no outstanding channel send, the link is gone, or the
    /// signature does not check out (in which case the send stays outstanding).
    pub fn settle_channel_ack(
        &mut self,
        link_id: &LinkId,
        payload: &[u8],
        arrived_at: InstantMillis,
    ) -> Option<(CommandId, Delivered)> {
        if payload.len() != EXPLICIT_PROOF_PAYLOAD_LEN {
            return None;
        }
        let index = self.channels.index_of(link_id)?;
        let (named_hash, signature) = payload.split_at(PACKET_HASH_LEN);
        let (Ok(named_hash), Ok(signature)) = (named_hash.try_into(), signature.try_into()) else {
            return None;
        };
        let named_hash = PacketHash::new(named_hash);
        let sub = self
            .channels
            .outstanding_packet_hashes(index)
            .iter()
            .position(|hash| *hash == named_hash)?;

        let Some(LinkPhase::Active { peer_signing, .. }) = self.links.phase_for(link_id) else {
            return None;
        };
        let peer_signing = *peer_signing;
        if ed25519_verify(
            &peer_signing,
            named_hash.as_bytes(),
            &Ed25519Signature(signature),
        )
        .is_err()
        {
            return None;
        }

        let command_id = self.channels.outstanding_command_id(index, sub);
        let sent_at = self.channels.outstanding_sent_at(index, sub);
        self.channels.retire_outstanding(index, sub);
        Some((
            command_id,
            Delivered {
                rtt_ms: arrived_at.0.saturating_sub(sent_at.0),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{
        x25519_diffie_hellman, Ed25519PublicKey, Ed25519SecretKey, X25519PublicKey,
        X25519SecretKey, X25519SharedSecret,
    };
    use crate::engine::commands::{EngineCommand, IssuedCommand, SendChannel, Settlement};
    use crate::engine::test_support::{
        filled_frame, fixed_secret_key, transporting_view, Cap, TEST_ENTROPY,
    };
    use crate::engine::{Delivered, Directive, EngineReaction, Journaled};
    use crate::identity::{in_memory::InMemoryNodeIdentity, IdentitySigner};
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::links::channel::MessageType;
    use crate::routing::links::table::{InitiatedLink, RespondingLink};
    use crate::routing::links::{LinkId, LinkKey};
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::wire::{DestinationHash, BROADCAST_MTU};
    use std::vec::Vec;

    const LANE: [u8; 16] = [0xEE; 16];
    const LINK: [u8; 16] = [0x5C; 16];

    fn shared() -> X25519SharedSecret {
        x25519_diffie_hellman(
            &X25519SecretKey::new([0x33; 32]),
            &X25519PublicKey([0x44; 32]),
        )
    }
    fn session_key(link_id: &LinkId) -> LinkKey {
        LinkKey::derive(link_id, &shared())
    }
    fn body(bytes: &[u8]) -> crate::engine::SendChannelBody {
        let mut body = crate::engine::SendChannelBody::new();
        body.extend_from_slice(bytes).unwrap();
        body
    }

    /// An engine that answers the link, holding the identity it signs acks with.
    fn responder() -> (EngineState<Cap>, LinkId, Ed25519PublicKey) {
        let link_id = LinkId::new(LINK);
        let mut state = EngineState::<Cap>::default();
        let identity = state.hold_identity(fixed_secret_key()).unwrap();
        let signing = *InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key())
            .signing_public_key()
            .as_ed25519();
        state
            .links
            .track_responding(RespondingLink {
                link_id,
                key: session_key(&link_id),
                requested_at: InstantMillis(0),
                timeout_at: InstantMillis(60_000),
                mtu: BROADCAST_MTU,
                initiator_signing: Ed25519PublicKey([0x99; 32]),
                destination: DestinationHash::new([0x77; 16]),
                identity,
                proof_strategy: ProofStrategy::ProveAll,
            })
            .unwrap();
        state
            .links
            .activate_responding(&link_id, 250, InterfaceId::new(LANE), InstantMillis(1_000))
            .unwrap();
        (state, link_id, signing)
    }

    /// An engine that opened the link, expecting the peer to sign acks with `peer`.
    fn initiator(peer: Ed25519PublicKey) -> (EngineState<Cap>, LinkId) {
        let link_id = LinkId::new(LINK);
        let mut state = EngineState::<Cap>::default();
        state
            .links
            .track_initiated(InitiatedLink {
                link_id,
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                link_signing: Ed25519SecretKey::new([0x11; 32]),
                requested_at: InstantMillis(0),
                timeout_at: InstantMillis(60_000),
                command_id: CommandId(1),
            })
            .unwrap();
        state
            .links
            .activate_initiated(
                &link_id,
                session_key(&link_id),
                250,
                BROADCAST_MTU,
                InterfaceId::new(LANE),
                InstantMillis(1_000),
                peer,
            )
            .unwrap();
        (state, link_id)
    }

    fn send_channel(
        engine: &mut EngineState<Cap>,
        link_id: LinkId,
        id: CommandId,
        message_type: MessageType,
        bytes: &[u8],
        now: u64,
    ) -> (Option<Vec<u8>>, Vec<(CommandId, Settlement)>) {
        let mut frame = None;
        let mut settled = Vec::new();
        engine.ingest_command_into(
            IssuedCommand {
                id,
                command: EngineCommand::SendChannel(SendChannel {
                    link_id,
                    message_type,
                    body: body(bytes),
                }),
            },
            &transporting_view(),
            InstantMillis(now),
            &mut |slot: &mut [u8]| slot.fill(0xAB),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    frame = filled_frame(fill)
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled.push((id, settlement))
                }
                _ => {}
            },
        );
        (frame, settled)
    }

    fn feed_packet(
        engine: &mut EngineState<Cap>,
        frame: &[u8],
        now: u64,
        on_message: &mut dyn FnMut(MessageType, &[u8]),
        on_send: &mut dyn FnMut(&[u8]),
        on_settled: &mut dyn FnMut(CommandId, Settlement),
    ) {
        let mut raw = frame.to_vec();
        engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(now),
                source_interface: InterfaceId::new(LANE),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
            InstantMillis(now),
            &mut |slot: &mut [u8]| slot.fill(0),
            &mut |_| false,
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::ChannelMessageReceived {
                    message_type,
                    data,
                    ..
                }) => on_message(message_type, data),
                EngineReaction::Directive(Directive::Send { bytes, .. }) => on_send(bytes),
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    on_settled(id, settlement)
                }
                _ => {}
            },
        );
    }

    #[test]
    fn a_channel_send_round_trips_to_delivered_through_the_peers_ack() {
        let (mut responder, link_id, responder_signing) = responder();
        let (mut initiator, _) = initiator(responder_signing);

        // The initiator sends; the message goes out, nothing settles yet.
        let (frame, settled) = send_channel(
            &mut initiator,
            link_id,
            CommandId(42),
            MessageType(7),
            b"hello channel",
            2_000,
        );
        let frame = frame.expect("the send emits a channel packet");
        assert!(
            settled.is_empty(),
            "a channel send settles on the ack, not at emission"
        );
        let index = initiator.channels.index_of(&link_id).unwrap();
        assert_eq!(initiator.channels.outstanding_count(index), 1);

        // The responder receives the message and answers the ack.
        let mut received = Vec::new();
        let mut ack = None;
        feed_packet(
            &mut responder,
            &frame,
            2_100,
            &mut |mt, data| received.push((mt, data.to_vec())),
            &mut |bytes| ack = Some(bytes.to_vec()),
            &mut |_, _| {},
        );
        assert_eq!(
            received,
            std::vec![(MessageType(7), b"hello channel".to_vec())]
        );
        let ack = ack.expect("the responder acks the channel packet");

        // The ack settles the initiator's send Delivered and frees the window.
        let mut settled = Vec::new();
        feed_packet(
            &mut initiator,
            &ack,
            2_200,
            &mut |_, _| {},
            &mut |_| {},
            &mut |id, settlement| settled.push((id, settlement)),
        );
        assert!(
            matches!(
                settled.as_slice(),
                [(
                    CommandId(42),
                    Settlement::SendChannel(Ok(Delivered { rtt_ms: 200 }))
                )]
            ),
            "got {settled:?}",
        );
        assert_eq!(
            initiator.channels.outstanding_count(index),
            0,
            "the ack retired the outstanding send",
        );
    }

    #[test]
    fn the_send_window_holds_at_its_limit_until_an_ack_frees_a_slot() {
        let (_, _, responder_signing) = responder();
        let (mut initiator, link_id) = initiator(responder_signing);

        let mut window_full = Vec::new();
        for n in 0..3u64 {
            let (_, settled) = send_channel(
                &mut initiator,
                link_id,
                CommandId(n),
                MessageType(0),
                b"x",
                2_000 + n,
            );
            window_full.extend(settled);
        }
        assert!(
            matches!(
                window_full.as_slice(),
                [(
                    CommandId(2),
                    Settlement::SendChannel(Err(SendChannelFailure::WindowFull))
                )]
            ),
            "the third send overflows the window of two; got {window_full:?}",
        );
        let index = initiator.channels.index_of(&link_id).unwrap();
        assert_eq!(
            initiator.channels.outstanding_count(index),
            CHANNEL_TX_WINDOW,
            "the window holds exactly its limit outstanding",
        );
    }
}
