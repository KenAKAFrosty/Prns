//! The transmit side of a channel: RNS 1.3.1 `Channel.send`'s sequencing and
//! windowed reliability, ported to the engine's command/receipt grammar. A send
//! stamps the channel's next sequence onto an envelope, seals it under the link
//! key as a `CHANNEL`-context packet, and tracks it in the channel's outstanding
//! ring; the peer's proof (an explicit link proof addressed to the link) settles
//! it Delivered. The window bounds how many sends may be in flight unproven —
//! opened by one on each ack and closed by one on each loss, ratcheting toward
//! an RTT-tiered ceiling (see [`ChannelWindow`]).

use crate::crypto::{ed25519_verify, Ed25519Signature};
use crate::engine::commands::{
    CommandId, CommandOutcome, Delivered, SendChannel, SendChannelError, SendChannelFailure,
    MAX_SEND_CHANNEL_BODY_LEN,
};
use crate::engine::reaction::LinkClosedReason;
use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, Journaled, Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::links::channel::columns::{ChannelColumns, OutstandingSend, TxOutcome};
use crate::routing::links::channel::{
    write_envelope, ChannelRtt, ChannelSequence, ChannelWindow, ENVELOPE_HEADER_LEN,
};
use crate::routing::links::data::{write_link_packet, LinkDataError};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::routing::proof::EXPLICIT_PROOF_PAYLOAD_LEN;
use crate::storage::StorageLayout;
use crate::units::Rtt;
use crate::wire::{DestinationHash, DestinationType, WireContext, BROADCAST_MTU, HEADER_MIN_LEN};

/// RNS 1.3.1 `Channel.WINDOW`: the number of unproven messages a fresh channel
/// keeps in flight before it has adapted. A channel's live allowance is its
/// [`ChannelWindow`], which opens from here toward an RTT-tiered ceiling on acks
/// and closes back toward its floor on losses.
pub const CHANNEL_TX_WINDOW: usize = ChannelWindow::INITIAL as usize;

/// The scratch an outbound envelope needs before sealing: the 6-byte header plus
/// the largest body a channel message carries at the broadcast MTU.
const CHANNEL_PLAINTEXT_CAP: usize = ENVELOPE_HEADER_LEN + MAX_SEND_CHANNEL_BODY_LEN;

/// RNS 1.3.1 `Channel._max_tries`: how many times a send is retransmitted before
/// the link is torn down for being unresponsive.
pub const CHANNEL_MAX_TRIES: u8 = 5;

/// How long a send on its `tries`-th attempt waits for its proof before the
/// watchdog retransmits — an integer reformulation of RNS Channel's
/// `_get_packet_timeout_time` (local pacing, no parity cost): a base of
/// `max(rtt × 2.5, 25 ms)` widened with each retry.
pub fn channel_retry_timeout_ms(rtt: Rtt, tries: u8) -> u64 {
    let base = rtt.millis().saturating_mul(5).saturating_div(2).max(25);
    base.saturating_mul(u64::from(tries) + 1)
}

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
                    self.channels.outstanding_count(index)
                        >= self.channels.window(index).in_flight_count_limit()
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
        let rtt = match self.links.phase_for(&send.link_id) {
            Some(LinkPhase::Active { rtt, .. }) => *rtt,
            _ => return Err(SendChannelWriteError::LinkVanished),
        };
        let index = self
            .channels
            .ensure(&send.link_id)
            .map_err(|_| SendChannelWriteError::Untrackable)?;
        if self.channels.next_tx_sequence(index) == ChannelSequence(0)
            && self.channels.outstanding_count(index) == 0
        {
            self.channels
                .set_window(index, ChannelWindow::for_rtt(ChannelRtt(rtt)));
        }
        if self.channels.outstanding_count(index)
            >= self.channels.window(index).in_flight_count_limit()
        {
            return Err(SendChannelWriteError::WindowFull);
        }
        let sequence = self.channels.next_tx_sequence(index);

        let mut envelope = [0u8; CHANNEL_PLAINTEXT_CAP];
        let plaintext_len = write_envelope(send.message_type, sequence, &send.body, &mut envelope)
            .map_err(|_| SendChannelWriteError::Frame(LinkDataError::PayloadTooLong))?;

        let Some(LinkPhase::Active { key, mtu, .. }) = self.links.phase_for(&send.link_id) else {
            return Err(SendChannelWriteError::LinkVanished);
        };
        let timeout_at = InstantMillis(now.0.saturating_add(channel_retry_timeout_ms(rtt, 0)));
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
        let outcome = self.channels.push_outstanding(
            index,
            OutstandingSend {
                packet_hash,
                command_id: id,
                sequence,
                message_type: send.message_type,
                body: &send.body,
                iv: *iv,
                sent_at: now,
                timeout_at,
            },
        );
        match outcome {
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

        let Some(LinkPhase::Active {
            peer_signing, rtt, ..
        }) = self.links.phase_for(link_id)
        else {
            return None;
        };
        let peer_signing = *peer_signing;
        let rtt = *rtt;
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
        let mut window = self.channels.window(index);
        window.grow_on_ack(ChannelRtt(rtt));
        self.channels.set_window(index, window);
        Some((
            command_id,
            Delivered {
                rtt: Rtt::measured_between(sent_at, arrived_at),
            },
        ))
    }

    /// RNS 1.3.1 `Channel._packet_timeout`: retransmit every channel send whose
    /// proof is overdue, byte-identically (same sequence/IV → same packet hash, so
    /// the original outstanding entry still settles it), each retry widening its
    /// next deadline. A send that exhausts [`CHANNEL_MAX_TRIES`] tears the link
    /// down — every still-outstanding send on it settles Timeout, the peer gets a
    /// LINKCLOSE, and the channel state is dropped.
    pub fn fire_due_channel_timeouts<F>(
        &mut self,
        now: InstantMillis,
        view: &[InterfaceConfig],
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        while let Some((index, sub)) = self.next_due_channel(now) {
            let link_id = self.channels.link_at(index);
            let tries = self.channels.outstanding_tries(index, sub);

            let active = match self.links.phase_for(&link_id) {
                Some(LinkPhase::Active {
                    rtt,
                    attached_interface,
                    ..
                }) => Some((*rtt, *attached_interface)),
                _ => None,
            };
            let Some((rtt, fire_on)) = active else {
                let id = self.channels.outstanding_command_id(index, sub);
                self.channels.retire_outstanding(index, sub);
                settle_channel_timeout(id, sink);
                continue;
            };
            if tries >= CHANNEL_MAX_TRIES {
                self.teardown_channel_link(&link_id, view, fill_entropy, sink);
                continue;
            }

            let sequence = self.channels.outstanding_sequence(index, sub);
            let message_type = self.channels.outstanding_message_type(index, sub);
            let iv = self.channels.outstanding_iv(index, sub);
            let body_src = self.channels.outstanding_body(index, sub);
            let body_len = body_src.len();
            let mut body = [0u8; MAX_SEND_CHANNEL_BODY_LEN];
            body[..body_len].copy_from_slice(body_src);

            let mut envelope = [0u8; CHANNEL_PLAINTEXT_CAP];
            let mut frame = [0u8; BROADCAST_MTU];
            let resealed = match self.links.phase_for(&link_id) {
                Some(LinkPhase::Active { key, mtu, .. }) => {
                    write_envelope(message_type, sequence, &body[..body_len], &mut envelope)
                        .ok()
                        .and_then(|env_len| {
                            write_link_packet(
                                &link_id,
                                key,
                                *mtu,
                                WireContext::Channel,
                                &envelope[..env_len],
                                &iv,
                                &mut frame,
                            )
                            .ok()
                        })
                }
                _ => None,
            };
            if let Some(wire_len) = resealed {
                if transmit_eligible(view, fire_on) {
                    sink(EngineReaction::Directive(Directive::Send {
                        target: fire_on,
                        bytes: &frame[..wire_len],
                    }));
                }
            }
            let new_tries = tries + 1;
            self.channels.set_outstanding_tries(index, sub, new_tries);
            self.channels.set_outstanding_timeout_at(
                index,
                sub,
                InstantMillis(
                    now.0
                        .saturating_add(channel_retry_timeout_ms(rtt, new_tries)),
                ),
            );
            let mut window = self.channels.window(index);
            window.shrink_on_loss();
            self.channels.set_window(index, window);
        }

        let mut wake = WakeSchedules::UNCHANGED;
        wake.channel_timeouts = self.channel_timeouts_wake();
        wake.link_deadlines = self.link_deadlines_wake();
        wake
    }

    /// The first outstanding channel send whose retry deadline has passed.
    fn next_due_channel(&self, now: InstantMillis) -> Option<(usize, usize)> {
        for index in 0..self.channels.len() {
            for sub in 0..self.channels.outstanding_count(index) {
                if self.channels.outstanding_timeout_at(index, sub).0 <= now.0 {
                    return Some((index, sub));
                }
            }
        }
        None
    }

    /// Tear down a link whose channel ran out of retries: settle every send still
    /// outstanding on it Timeout, send the peer the sealed LINKCLOSE, journal the
    /// closure, and drop the channel state.
    fn teardown_channel_link<F>(
        &mut self,
        link_id: &LinkId,
        view: &[InterfaceConfig],
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        if let Some(index) = self.channels.index_of(link_id) {
            while self.channels.outstanding_count(index) > 0 {
                let id = self.channels.outstanding_command_id(index, 0);
                self.channels.retire_outstanding(index, 0);
                settle_channel_timeout(id, sink);
            }
        }
        let mut iv = [0u8; ENCRYPTION_IV_LEN];
        fill_entropy(&mut iv);
        let mut buf = [0u8; BROADCAST_MTU];
        if let Ok(dispatch) = self.write_owed_link_close(link_id, &iv, &mut buf) {
            if let Some(target) = dispatch.fire_on {
                if transmit_eligible(view, target) {
                    sink(EngineReaction::Directive(Directive::Send {
                        target,
                        bytes: &buf[..dispatch.wire_len],
                    }));
                }
            }
            sink(EngineReaction::Journaled(Journaled::LinkClosed {
                link_id: *link_id,
                reason: LinkClosedReason::Timeout,
            }));
        }
    }
}

fn settle_channel_timeout(id: CommandId, sink: &mut impl FnMut(EngineReaction<'_>)) {
    sink(EngineReaction::Journaled(Journaled::CommandSettled {
        id,
        settlement: Settlement::SendChannel(Err(SendChannelFailure::Timeout)),
    }));
}

/// Whether `target` is in the view and may transmit — the self-originated egress
/// gate (RNS would not push onto a receive-only or downed interface).
fn transmit_eligible(view: &[InterfaceConfig], target: InterfaceId) -> bool {
    view.iter()
        .find(|config| config.id == target)
        .is_some_and(|config| config.capabilities.allows_transmit())
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

    const LANE: [u8; 8] = [0xEE; 8];
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
            .activate_responding(
                &link_id,
                Rtt(250),
                InterfaceId::new(LANE),
                InstantMillis(1_000),
            )
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
                Rtt(250),
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
                    Settlement::SendChannel(Ok(Delivered { rtt: Rtt(200) }))
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

    #[test]
    fn the_send_window_opens_by_one_as_each_ack_arrives() {
        let (mut responder, link_id, responder_signing) = responder();
        let (mut initiator, _) = initiator(responder_signing);

        let (frame, _) = send_channel(
            &mut initiator,
            link_id,
            CommandId(1),
            MessageType(0),
            b"a",
            2_000,
        );
        let frame = frame.expect("the send goes out");
        let index = initiator.channels.index_of(&link_id).unwrap();
        assert_eq!(
            initiator.channels.window(index).in_flight_count_limit(),
            CHANNEL_TX_WINDOW,
            "an unacked channel sits at the initial window",
        );

        let mut ack = None;
        feed_packet(
            &mut responder,
            &frame,
            2_100,
            &mut |_, _| {},
            &mut |bytes| ack = Some(bytes.to_vec()),
            &mut |_, _| {},
        );
        let ack = ack.expect("the responder acks");
        feed_packet(
            &mut initiator,
            &ack,
            2_200,
            &mut |_, _| {},
            &mut |_| {},
            &mut |_, _| {},
        );

        assert_eq!(
            initiator.channels.window(index).in_flight_count_limit(),
            CHANNEL_TX_WINDOW + 1,
            "the ack opened the window by one",
        );
    }

    struct Fired {
        sends: Vec<Vec<u8>>,
        timed_out: Vec<(CommandId, Settlement)>,
        closed: Vec<LinkId>,
    }

    fn fire(engine: &mut EngineState<Cap>, now: u64) -> Fired {
        let mut fired = Fired {
            sends: Vec::new(),
            timed_out: Vec::new(),
            closed: Vec::new(),
        };
        engine.fire_due_channel_timeouts(
            InstantMillis(now),
            &transporting_view(),
            &mut |slot: &mut [u8]| slot.fill(0),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    fired.sends.push(bytes.to_vec())
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    fired.timed_out.push((id, settlement))
                }
                EngineReaction::Journaled(Journaled::LinkClosed { link_id, .. }) => {
                    fired.closed.push(link_id)
                }
                _ => {}
            },
        );
        fired
    }

    #[test]
    fn an_unacked_send_retransmits_byte_identically_then_tears_the_link_down() {
        let (_, _, responder_signing) = responder();
        let (mut initiator, link_id) = initiator(responder_signing);

        let (original, settled) = send_channel(
            &mut initiator,
            link_id,
            CommandId(7),
            MessageType(1),
            b"retry me",
            2_000,
        );
        let original = original.expect("the first send goes out");
        assert!(settled.is_empty());

        // Five retries: each watchdog firing past the (growing) deadline retransmits
        // a byte-identical packet and bumps the try count.
        let index = initiator.channels.index_of(&link_id).unwrap();
        for tries in 1..=CHANNEL_MAX_TRIES {
            let fired = fire(&mut initiator, 2_000 + u64::from(tries) * 1_000_000);
            assert_eq!(
                fired.sends,
                std::vec![original.clone()],
                "retry {tries} resends the same packet"
            );
            assert!(fired.closed.is_empty() && fired.timed_out.is_empty());
            assert_eq!(initiator.channels.outstanding_tries(index, 0), tries);
        }

        // The sixth firing finds the budget spent: the link tears down, the send
        // settles Timeout, and the channel state is gone.
        let fired = fire(&mut initiator, 2_000 + 9_000_000);
        assert_eq!(fired.closed, std::vec![link_id], "the link is torn down");
        assert!(
            matches!(
                fired.timed_out.as_slice(),
                [(
                    CommandId(7),
                    Settlement::SendChannel(Err(SendChannelFailure::Timeout))
                )]
            ),
            "got {:?}",
            fired.timed_out,
        );
        assert_eq!(
            initiator.channels.index_of(&link_id),
            None,
            "the channel is dropped with its link"
        );
    }

    #[test]
    fn a_retransmission_still_settles_when_its_ack_arrives() {
        let (mut responder, link_id, responder_signing) = responder();
        let (mut initiator, _) = initiator(responder_signing);

        let _ = send_channel(
            &mut initiator,
            link_id,
            CommandId(9),
            MessageType(3),
            b"once more",
            2_000,
        );
        // The first attempt is "lost"; the watchdog retransmits it.
        let fired = fire(&mut initiator, 9_000_000);
        let resent = fired
            .sends
            .into_iter()
            .next()
            .expect("the watchdog retransmits");

        // The peer receives the retransmission and acks it; the ack settles the
        // send Delivered against the unchanged outstanding hash.
        let mut ack = None;
        feed_packet(
            &mut responder,
            &resent,
            9_000_100,
            &mut |_, _| {},
            &mut |bytes| ack = Some(bytes.to_vec()),
            &mut |_, _| {},
        );
        let ack = ack.expect("the responder acks the retransmission");

        let mut settled = Vec::new();
        feed_packet(
            &mut initiator,
            &ack,
            9_000_200,
            &mut |_, _| {},
            &mut |_| {},
            &mut |id, settlement| settled.push((id, settlement)),
        );
        assert!(
            matches!(
                settled.as_slice(),
                [(CommandId(9), Settlement::SendChannel(Ok(Delivered { .. })))]
            ),
            "the retransmission's ack settles the original send; got {settled:?}",
        );
    }
}
