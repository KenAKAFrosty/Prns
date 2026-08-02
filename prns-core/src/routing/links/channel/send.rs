//! RNS 1.4.0 `Channel.send`'s sequencing and windowed reliability, ported to the engine's command/receipt grammar.

use crate::crypto::{ed25519_verify, Ed25519Signature};
use crate::engine::LinkClosedReason;
use crate::engine::{
    CommandId, CommandOutcome, DeliveryEvidence, DeliveryProof, PacketReceiptDelivered,
    SendToChannel, SendToChannelFailure, SendToChannelRejection, MAX_SEND_TO_CHANNEL_BODY_LEN,
};
use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, Journaled, Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{AttachedInterfaces, Egress, InterfaceId};
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::links::channel::table::{ChannelTable, OutstandingSend, TxOutcome};
use crate::routing::links::channel::{
    write_envelope, ChannelRtt, ChannelSequence, ChannelWindow, CHANNEL_ENVELOPE_HEADER_LEN,
};
use crate::routing::links::data::{write_link_packet, LinkDataError};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::routing::proof::EXPLICIT_PROOF_PAYLOAD_LEN;
use crate::storage::StorageLayout;
use crate::units::RttMillis;
#[cfg(test)]
use crate::wire::DestinationHash;
use crate::wire::{DestinationType, WireContext, BROADCAST_MTU, HEADER_MIN_LEN};

/// RNS 1.4.0 `Channel.WINDOW`
pub const CHANNEL_TX_WINDOW: usize = ChannelWindow::INITIAL as usize;

const CHANNEL_PLAINTEXT_CAP: usize = CHANNEL_ENVELOPE_HEADER_LEN + MAX_SEND_TO_CHANNEL_BODY_LEN;

/// RNS 1.4.0 `Channel._max_tries`
pub const CHANNEL_MAX_TRIES: u8 = 5;

/// RNS 1.4.0 `Channel._get_packet_timeout_time` in integer millis: `1.5^(tries − 1) × max(rtt × 2.5, 25 ms) × (in_flight_count + 1.5)`.
/// In-flight messages queue serially on the wire, so the deadline scales with the ring; the fractions cancel exactly as `base × (2·ring + 3) × 3^(tries−1) / 2^tries`.
pub fn channel_retry_timeout_ms(rtt: RttMillis, tries: u8, in_flight_count: usize) -> u64 {
    let tries = tries.min(CHANNEL_MAX_TRIES);
    let base = rtt.millis().saturating_mul(5).saturating_div(2).max(25);
    let ring_scaled =
        base.saturating_mul((in_flight_count as u64).saturating_mul(2).saturating_add(3));
    match tries {
        0 => ring_scaled / 3,
        tries => ring_scaled.saturating_mul(3u64.pow(u32::from(tries) - 1)) / (1u64 << tries),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendToChannelDispatch {
    pub wire_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendToChannelWriteError {
    LinkVanished,
    Untrackable,
    WindowFull,
    Frame(LinkDataError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DueSendVerdict {
    OrphanedByLink,
    Exhausted,
    Retry {
        rtt: RttMillis,
        fire_on: InterfaceId,
    },
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_send_to_channel(&self, id: CommandId, send: SendToChannel) -> CommandOutcome {
        match self.links.phase_for(&send.link_id) {
            None => CommandOutcome::SendToChannelRejected {
                id,
                failure: SendToChannelFailure::Rejected(SendToChannelRejection::NoSuchLink),
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::SendToChannelRejected {
                    id,
                    failure: SendToChannelFailure::Rejected(SendToChannelRejection::LinkNotActive),
                }
            }
            Some(LinkPhase::Active { .. }) => {
                let window_full = self.channels.index_of(&send.link_id).is_some_and(|index| {
                    self.channels.outstanding_count(index)
                        >= self.channels.window(index).in_flight_count_limit()
                });
                if window_full {
                    CommandOutcome::SendToChannelRejected {
                        id,
                        failure: SendToChannelFailure::WindowFull,
                    }
                } else {
                    CommandOutcome::OwesSendToChannel { id, send }
                }
            }
        }
    }

    /// The receiver delivers in sequence order, so a sequence number burned on a failed write would be a hole the peer stalls on forever.
    /// So the number is consumed only once the send is `Tracked`, after every fallible step.
    pub fn write_commanded_send_to_channel(
        &mut self,
        id: CommandId,
        send: &SendToChannel,
        now: InstantMillis,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<SendToChannelDispatch, SendToChannelWriteError> {
        let rtt = match self.links.phase_for(&send.link_id) {
            Some(LinkPhase::Active { rtt, .. }) => *rtt,
            _ => return Err(SendToChannelWriteError::LinkVanished),
        };
        let index = self
            .channels
            .ensure(&send.link_id)
            .map_err(|_| SendToChannelWriteError::Untrackable)?;
        if self.channels.next_tx_sequence(index) == ChannelSequence(0)
            && self.channels.outstanding_count(index) == 0
        {
            self.channels
                .set_window(index, ChannelWindow::for_rtt(ChannelRtt(rtt)));
        }
        if self.channels.outstanding_count(index)
            >= self.channels.window(index).in_flight_count_limit()
        {
            return Err(SendToChannelWriteError::WindowFull);
        }
        let sequence = self.channels.next_tx_sequence(index);

        let mut envelope = [0u8; CHANNEL_PLAINTEXT_CAP];
        let plaintext_len = write_envelope(send.message_type, sequence, &send.body, &mut envelope)
            .map_err(|_| SendToChannelWriteError::Frame(LinkDataError::PayloadTooLong))?;

        let Some(LinkPhase::Active { key, mtu, .. }) = self.links.phase_for(&send.link_id) else {
            return Err(SendToChannelWriteError::LinkVanished);
        };
        let in_flight_count = self.channels.outstanding_count(index) + 1;
        let timeout_at = InstantMillis(now.0.saturating_add(channel_retry_timeout_ms(
            rtt,
            0,
            in_flight_count,
        )));
        let wire_bytes = write_link_packet(
            &send.link_id,
            key,
            *mtu,
            WireContext::Channel,
            &envelope[..plaintext_len],
            iv,
            buf,
        )
        .map_err(SendToChannelWriteError::Frame)?;

        let packet_hash = PacketHash::of_data_fields(
            DestinationType::Link,
            &send.link_id.to_address(),
            WireContext::Channel,
            &buf[HEADER_MIN_LEN..wire_bytes],
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
                self.stretch_channel_deadlines(index, rtt);
                Ok(SendToChannelDispatch { wire_bytes })
            }
            TxOutcome::Full => Err(SendToChannelWriteError::WindowFull),
        }
    }

    /// RNS 1.4.0 `PacketReceipt.validate_proof`: the outstanding-hash match gates the `ed25519` verify. The reference uses the same order, so proofs naming nothing we sent cost no signature check.
    pub fn settle_channel_ack(
        &mut self,
        link_id: &LinkId,
        payload: &[u8],
        proof: DeliveryProof,
        arrived_at: InstantMillis,
    ) -> Option<(CommandId, PacketReceiptDelivered)> {
        if payload.len() != EXPLICIT_PROOF_PAYLOAD_LEN {
            return None;
        }
        let index = self.channels.index_of(link_id)?;
        let (named_hash, signature) = payload.split_at(PACKET_HASH_LEN);
        let (Ok(named_hash), Ok(signature)) = (named_hash.try_into(), signature.try_into()) else {
            return None;
        };
        let named_hash = PacketHash::new(named_hash);
        let outstanding_index = self
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

        let command_id = self
            .channels
            .outstanding_command_id(index, outstanding_index);
        let sent_at = self.channels.outstanding_sent_at(index, outstanding_index);
        self.channels.retire_outstanding(index, outstanding_index);
        let mut window = self.channels.window(index);
        window.grow_on_ack(ChannelRtt(rtt));
        self.channels.set_window(index, window);
        Some((
            command_id,
            PacketReceiptDelivered {
                rtt: RttMillis::measured_between(sent_at, arrived_at),
                evidence: DeliveryEvidence::Proof(proof),
            },
        ))
    }

    /// RNS 1.4.0 `Channel._packet_timeout`. Retransmits are byte-identical (same sequence and IV, so the same packet hash, so the original outstanding entry still settles).
    /// A send that exhausts [`CHANNEL_MAX_TRIES`] tears the link down.
    pub fn fire_due_channel_timeouts<F>(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        while let Some((index, outstanding_index)) = self.next_due_channel(now) {
            let link_id = self.channels.link_at(index);
            let tries = self.channels.outstanding_tries(index, outstanding_index);
            match self.classify_due_send(&link_id, tries) {
                DueSendVerdict::OrphanedByLink => {
                    let id = self
                        .channels
                        .outstanding_command_id(index, outstanding_index);
                    self.channels.retire_outstanding(index, outstanding_index);
                    settle_channel_timeout(id, sink);
                }
                DueSendVerdict::Exhausted => {
                    self.teardown_channel_link(&link_id, interfaces, fill_entropy, sink);
                }
                DueSendVerdict::Retry { rtt, fire_on } => {
                    self.retry_outstanding_send(
                        index,
                        outstanding_index,
                        &link_id,
                        rtt,
                        fire_on,
                        interfaces,
                        sink,
                    );
                }
            }
        }

        let mut wake = WakeSchedules::UNCHANGED;
        wake.channel_timeouts = self.channel_timeouts_wake();
        wake.link_deadlines = self.link_deadlines_wake();
        wake
    }

    fn next_due_channel(&self, now: InstantMillis) -> Option<(usize, usize)> {
        let index = self.channels.first_due_channel(now)?;
        for outstanding_index in 0..self.channels.outstanding_count(index) {
            if self
                .channels
                .outstanding_timeout_at(index, outstanding_index)
                .0
                <= now.0
            {
                return Some((index, outstanding_index));
            }
        }
        None
    }

    fn classify_due_send(&self, link_id: &LinkId, tries: u8) -> DueSendVerdict {
        match self.links.phase_for(link_id) {
            Some(LinkPhase::Active {
                rtt,
                attached_interface,
                ..
            }) => {
                if tries >= CHANNEL_MAX_TRIES {
                    DueSendVerdict::Exhausted
                } else {
                    DueSendVerdict::Retry {
                        rtt: *rtt,
                        fire_on: *attached_interface,
                    }
                }
            }
            _ => DueSendVerdict::OrphanedByLink,
        }
    }

    /// RNS 1.4.0 `PacketReceipt.sent_at` never resets on resend, so each deadline is `sent_at + timeout(tries)`. The retry ladder is the exponential curve itself, not a running sum of gaps.
    #[allow(clippy::too_many_arguments)]
    fn retry_outstanding_send(
        &mut self,
        index: usize,
        outstanding_index: usize,
        link_id: &LinkId,
        rtt: RttMillis,
        fire_on: InterfaceId,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let tries = self.channels.outstanding_tries(index, outstanding_index);
        let sequence = self.channels.outstanding_sequence(index, outstanding_index);
        let message_type = self
            .channels
            .outstanding_message_type(index, outstanding_index);
        let iv = self.channels.outstanding_iv(index, outstanding_index);
        let body_src = self.channels.outstanding_body(index, outstanding_index);
        let body_len = body_src.len();
        let mut body = [0u8; MAX_SEND_TO_CHANNEL_BODY_LEN];
        body[..body_len].copy_from_slice(body_src);

        let mut envelope = [0u8; CHANNEL_PLAINTEXT_CAP];
        let mut frame = [0u8; BROADCAST_MTU];
        let resealed = match self.links.phase_for(link_id) {
            Some(LinkPhase::Active { key, mtu, .. }) => {
                write_envelope(message_type, sequence, &body[..body_len], &mut envelope)
                    .ok()
                    .and_then(|env_len| {
                        write_link_packet(
                            link_id,
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
        if let Some(wire_bytes) = resealed {
            if interfaces.is_egress_eligible(fire_on, Egress::Transmit) {
                sink(EngineReaction::Directive(Directive::Send {
                    target: fire_on,
                    bytes: &frame[..wire_bytes],
                }));
            }
        }

        let new_tries = tries + 1;
        self.channels
            .set_outstanding_tries(index, outstanding_index, new_tries);
        let sent_at = self.channels.outstanding_sent_at(index, outstanding_index);
        let in_flight_count = self.channels.outstanding_count(index);
        self.channels.set_outstanding_timeout_at(
            index,
            outstanding_index,
            InstantMillis(sent_at.0.saturating_add(channel_retry_timeout_ms(
                rtt,
                new_tries,
                in_flight_count,
            ))),
        );
        self.stretch_channel_deadlines(index, rtt);
        let mut window = self.channels.window(index);
        window.shrink_on_loss();
        self.channels.set_window(index, window);
    }

    /// RNS 1.4.0 `Channel._update_packet_timeouts`: after every send and resend, each in-flight deadline is recomputed at the current ring size and only ever extended, never shortened.
    fn stretch_channel_deadlines(&mut self, index: usize, rtt: RttMillis) {
        let in_flight_count = self.channels.outstanding_count(index);
        for outstanding_index in 0..in_flight_count {
            let tries = self.channels.outstanding_tries(index, outstanding_index);
            let sent_at = self.channels.outstanding_sent_at(index, outstanding_index);
            let stretched = InstantMillis(sent_at.0.saturating_add(channel_retry_timeout_ms(
                rtt,
                tries,
                in_flight_count,
            )));
            if stretched.0
                > self
                    .channels
                    .outstanding_timeout_at(index, outstanding_index)
                    .0
            {
                self.channels
                    .set_outstanding_timeout_at(index, outstanding_index, stretched);
            }
        }
    }

    fn teardown_channel_link<F>(
        &mut self,
        link_id: &LinkId,
        interfaces: AttachedInterfaces<'_>,
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
                if interfaces.is_egress_eligible(target, Egress::Transmit) {
                    sink(EngineReaction::Directive(Directive::Send {
                        target,
                        bytes: &buf[..dispatch.wire_bytes],
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
        settlement: Settlement::SendToChannel(Err(SendToChannelFailure::Timeout)),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{
        x25519_diffie_hellman, Ed25519PublicKey, Ed25519SecretKey, X25519PublicKey,
        X25519SecretKey, X25519SharedSecret,
    };
    use crate::engine::test_support::{
        filled_frame, fixed_secret_key, transporting_interfaces, TestStorageLayout,
    };
    use crate::engine::IngestIo;
    use crate::engine::{Directive, EngineReaction, Journaled, PacketReceiptDelivered};
    use crate::engine::{IssuedCommand, PrnsCommand, SendToChannel, Settlement};
    use crate::identity::{in_memory::InMemoryNodeIdentity, IdentitySigner};
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::links::channel::MessageType;
    use crate::routing::links::data::link_data_frame_ceiling;
    use crate::routing::links::table::{InitiatedLink, RespondingLink};
    use crate::routing::links::table::{LinkActivation, LinkPhase};
    use crate::routing::links::{LinkId, LinkKey, LinkMode};
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::wire::BROADCAST_MTU;
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
    fn body(bytes: &[u8]) -> crate::engine::SendToChannelBody {
        let mut body = crate::engine::SendToChannelBody::new();
        body.extend_from_slice(bytes).unwrap();
        body
    }

    fn responder() -> (EngineState<TestStorageLayout>, LinkId, Ed25519PublicKey) {
        let link_id = LinkId::new(LINK);
        let mut state = EngineState::<TestStorageLayout>::default();
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
                RttMillis::new(250),
                InterfaceId::new(LANE),
                InstantMillis(1_000),
            )
            .unwrap();
        (state, link_id, signing)
    }

    fn initiator(peer: Ed25519PublicKey) -> (EngineState<TestStorageLayout>, LinkId) {
        let link_id = LinkId::new(LINK);
        let mut state = EngineState::<TestStorageLayout>::default();
        state
            .links
            .track_initiated(InitiatedLink {
                link_id,
                destination: DestinationHash::new([0x77; 16]),
                expected_hops: 1,
                mode: LinkMode::Aes256Cbc,
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
                &LinkActivation {
                    received_hops: 1,
                    rtt: RttMillis::new(250),
                    mtu: BROADCAST_MTU,
                    attached_interface: InterfaceId::new(LANE),
                    peer_signing: peer,
                },
                InstantMillis(1_000),
            )
            .unwrap();
        (state, link_id)
    }

    fn send_to_channel(
        engine: &mut EngineState<TestStorageLayout>,
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
                command: PrnsCommand::SendToChannel(SendToChannel {
                    link_id,
                    message_type,
                    body: body(bytes),
                }),
            },
            AttachedInterfaces::new(&transporting_interfaces()),
            InstantMillis(now),
            &mut |slot: &mut [u8]| slot.fill(0xAB),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame {
                    size_hint, fill, ..
                }) => {
                    assert_eq!(
                        size_hint,
                        link_data_frame_ceiling(CHANNEL_ENVELOPE_HEADER_LEN + bytes.len()),
                    );
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
        engine: &mut EngineState<TestStorageLayout>,
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
            IngestIo {
                interfaces: AttachedInterfaces::new(&transporting_interfaces()),
                now: InstantMillis(now),
                fill_entropy: &mut |slot: &mut [u8]| slot.fill(0),
                should_prove: &mut |_| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| match reaction {
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
            },
        );
    }

    #[test]
    fn a_channel_send_round_trips_to_delivered_through_the_peers_ack() {
        let (mut responder, link_id, responder_signing) = responder();
        let (mut initiator, _) = initiator(responder_signing);

        let (frame, settled) = send_to_channel(
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
        let ack_hash = PacketHash::of_wire_packet(&ack).unwrap();

        let mut settled = Vec::new();
        feed_packet(
            &mut initiator,
            &ack,
            2_200,
            &mut |_, _| {},
            &mut |_| {},
            &mut |id, settlement| settled.push((id, settlement)),
        );
        assert_eq!(
            settled,
            [(
                CommandId(42),
                Settlement::SendToChannel(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(200),
                    evidence: DeliveryEvidence::Proof(DeliveryProof::Explicit(ack_hash)),
                }))
            )],
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
            let (_, settled) = send_to_channel(
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
                    Settlement::SendToChannel(Err(SendToChannelFailure::WindowFull))
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

        let (frame, _) = send_to_channel(
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

    fn fire(engine: &mut EngineState<TestStorageLayout>, now: u64) -> Fired {
        let mut fired = Fired {
            sends: Vec::new(),
            timed_out: Vec::new(),
            closed: Vec::new(),
        };
        engine.fire_due_channel_timeouts(
            InstantMillis(now),
            AttachedInterfaces::new(&transporting_interfaces()),
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

        let (original, settled) = send_to_channel(
            &mut initiator,
            link_id,
            CommandId(7),
            MessageType(1),
            b"retry me",
            2_000,
        );
        let original = original.expect("the first send goes out");
        assert!(settled.is_empty());

        let index = initiator.channels.index_of(&link_id).unwrap();
        let rtt = RttMillis::new(250);
        for tries in 1..=CHANNEL_MAX_TRIES {
            let due_at = 2_000 + channel_retry_timeout_ms(rtt, tries - 1, 1);
            let fired = fire(&mut initiator, due_at);
            assert_eq!(
                fired.sends,
                std::vec![original.clone()],
                "retry {tries} resends the same packet"
            );
            assert!(fired.closed.is_empty() && fired.timed_out.is_empty());
            assert_eq!(initiator.channels.outstanding_tries(index, 0), tries);
            let Some(LinkPhase::Active { last_outbound, .. }) = initiator.links.phase_for(&link_id)
            else {
                panic!("the retrying channel link must remain active");
            };
            assert_eq!(
                *last_outbound,
                InstantMillis(2_000),
                "byte-identical channel retransmissions do not refresh outbound liveness",
            );
        }

        let fired = fire(
            &mut initiator,
            2_000 + channel_retry_timeout_ms(rtt, CHANNEL_MAX_TRIES, 1),
        );
        assert_eq!(fired.closed, std::vec![link_id], "the link is torn down");
        assert!(
            matches!(
                fired.timed_out.as_slice(),
                [(
                    CommandId(7),
                    Settlement::SendToChannel(Err(SendToChannelFailure::Timeout))
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

        let _ = send_to_channel(
            &mut initiator,
            link_id,
            CommandId(9),
            MessageType(3),
            b"once more",
            2_000,
        );
        let fired = fire(
            &mut initiator,
            2_000 + channel_retry_timeout_ms(RttMillis::new(250), 0, 1),
        );
        let resent = fired
            .sends
            .into_iter()
            .next()
            .expect("the watchdog retransmits");

        let mut ack = None;
        feed_packet(
            &mut responder,
            &resent,
            3_100,
            &mut |_, _| {},
            &mut |bytes| ack = Some(bytes.to_vec()),
            &mut |_, _| {},
        );
        let ack = ack.expect("the responder acks the retransmission");

        let mut settled = Vec::new();
        feed_packet(
            &mut initiator,
            &ack,
            3_200,
            &mut |_, _| {},
            &mut |_| {},
            &mut |id, settlement| settled.push((id, settlement)),
        );
        assert!(
            matches!(
                settled.as_slice(),
                [(
                    CommandId(9),
                    Settlement::SendToChannel(Ok(PacketReceiptDelivered { .. }))
                )]
            ),
            "the retransmission's ack settles the original send; got {settled:?}",
        );
    }

    #[test]
    fn the_retry_timeout_matches_the_reference_formula() {
        let rtt = RttMillis::new(1_000);
        assert_eq!(channel_retry_timeout_ms(rtt, 0, 1), 4_166);
        assert_eq!(channel_retry_timeout_ms(rtt, 1, 1), 6_250);
        assert_eq!(channel_retry_timeout_ms(rtt, 2, 3), 16_875);
        assert_eq!(channel_retry_timeout_ms(rtt, 5, 12), 170_859);
        assert_eq!(
            channel_retry_timeout_ms(RttMillis::new(0), 0, 1),
            41,
            "a zero rtt still pays the 25 ms floor",
        );
    }

    #[test]
    fn a_filling_ring_stretches_the_first_deadline_like_the_reference() {
        let (_, _, responder_signing) = responder();
        let (mut initiator, link_id) = initiator(responder_signing);
        let rtt = RttMillis::new(250);

        send_to_channel(
            &mut initiator,
            link_id,
            CommandId(1),
            MessageType(0),
            b"a",
            2_000,
        );
        let index = initiator.channels.index_of(&link_id).unwrap();
        let solo_deadline = initiator.channels.outstanding_timeout_at(index, 0);
        assert_eq!(solo_deadline.0, 2_000 + channel_retry_timeout_ms(rtt, 0, 1));

        send_to_channel(
            &mut initiator,
            link_id,
            CommandId(2),
            MessageType(0),
            b"b",
            2_100,
        );
        let stretched = initiator.channels.outstanding_timeout_at(index, 0);
        assert_eq!(
            stretched.0,
            2_000 + channel_retry_timeout_ms(rtt, 0, 2),
            "the second send re-times the first at the fuller ring",
        );
        assert!(stretched.0 > solo_deadline.0);
    }
}
