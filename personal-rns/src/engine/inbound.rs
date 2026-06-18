use crate::crypto::X25519SecretKey;
use crate::engine::reaction::LinkClosedReason;
use crate::engine::{
    write_path_request_wire_packet, AnnounceIngest, Directive, EngineReaction, EngineState,
    IngestPacketOutcome, InstantMillis, Journaled, LaneWake, LinkEstablished, PathFound,
    PathResponseWriteOutcome, ProofIngest, Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{InboundPacket, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::AnnounceEntropy;
use crate::routing::delivery::Delivery;
use crate::routing::links::channel::receive::receive as channel_receive;
use crate::routing::links::establish::link_mtu_ceiling;
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_ECHO};
use crate::routing::proof::{
    ProofObligation, ProofRequest, IMPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN,
};
use crate::routing::{RemovedRoute, RouteRemovalCause};
use crate::storage::StorageLayout;
use crate::wire::{BROADCAST_MTU, HEADER_MAX_LEN};

pub(crate) fn journal_removal(removed: RemovedRoute) -> Journaled<'static> {
    match removed.cause {
        RouteRemovalCause::Expired => Journaled::RouteExpired {
            destination: removed.destination,
        },
        RouteRemovalCause::Evicted => Journaled::RouteEvicted {
            destination: removed.destination,
        },
        RouteRemovalCause::InterfaceGone => Journaled::RouteInterfaceGone {
            destination: removed.destination,
        },
    }
}

impl<S: StorageLayout> EngineState<S> {
    /// Ingest one packet and stream everything it produces to `sink`: the `Journaled`
    /// facts (announce heard, delivery, the settlements a learned route closes) and the
    /// `Directive`s it owes — a proof back on the arrival lane, a packet forwarded onward,
    /// a path response. This is the sink-shaped inbound edge: it folds in the follow-up the
    /// legacy runtime ran after `ingest_packet`, so the reactor's inbound arm just forwards
    /// the stream. `fill_entropy` is pulled only when a path response is actually minted.
    /// Returns a [`WakeSchedules`] delta for the scheduled lanes this packet moved — a learned
    /// announce can schedule a rebroadcast, settle waiting path requests, and bound the
    /// route-expiry lane by the one route it touched (`AtMost`: never a whole-table scan on
    /// this path; removals only push the true deadline later, so a cached one stays early,
    /// never late), an arriving proof retires a send-timeout; everything else is `Unchanged`.
    #[allow(clippy::too_many_arguments)]
    pub fn ingest_packet_into<F>(
        &mut self,
        packet: InboundPacket<'_>,
        jitter: JitterSeed,
        view: &[InterfaceConfig],
        now: InstantMillis,
        fill_entropy: &mut F,
        should_prove: &mut impl FnMut(&ProofRequest) -> bool,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let source = packet.source_interface;
        let mut wake_schedule_changes = WakeSchedules::UNCHANGED;
        let outcome = self.ingest_packet_with(packet, jitter, view, &mut |removed| {
            sink(EngineReaction::Journaled(journal_removal(removed)))
        });
        match outcome {
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(accepted)) => {
                sink(EngineReaction::Journaled(Journaled::AnnounceHeard {
                    destination: accepted.destination,
                    hops: accepted.hops,
                    source_interface: source,
                }));
                // A learned route closes every path request that was waiting on it.
                while let Some(settled) = self.pop_settled_path_request(&accepted.destination) {
                    sink(EngineReaction::Journaled(Journaled::CommandSettled {
                        id: settled.command_id,
                        settlement: Settlement::RequestPath(Ok(PathFound {
                            hops: crate::units::HopCount(accepted.hops),
                        })),
                    }));
                }
                wake_schedule_changes.scheduled_announces = self.scheduled_announces_wake();
                wake_schedule_changes.path_request_timeout = self.path_request_timeout_wake();
                wake_schedule_changes.expired_routes = self
                    .routing_table
                    .existing_route_for(&accepted.destination, view)
                    .map_or(LaneWake::Unchanged, |route| LaneWake::AtMost(route.expires));
            }
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored) => {}
            IngestPacketOutcome::Delivery { delivery, proof } => {
                sink(EngineReaction::Journaled(Journaled::Delivered(delivery)));
                let owed = match proof {
                    ProofObligation::None
                    | ProofObligation::OwedOverLink(_)
                    | ProofObligation::OwedIfAppOverLink(_) => None,
                    ProofObligation::Owed(owed) => Some(owed),
                    ProofObligation::OwedIfApp(owed) => match delivery {
                        Delivery::Single(single) => should_prove(&ProofRequest {
                            destination: single.destination,
                            plaintext: single.plaintext,
                        })
                        .then_some(owed),
                        Delivery::Plain(_) | Delivery::Group(_) | Delivery::Link(_) => None,
                    },
                };
                if let Some(owed) = owed {
                    if is_egress_eligible(view, source, Egress::Transmit) {
                        let mut proof = [0u8; IMPLICIT_PROOF_WIRE_LEN];
                        if let Ok(written) = self.write_proof(&owed, &mut proof) {
                            sink(EngineReaction::Directive(Directive::Send {
                                target: source,
                                bytes: &proof[..written],
                            }));
                        }
                    }
                }
                let owed_over_link = match proof {
                    ProofObligation::None
                    | ProofObligation::Owed(_)
                    | ProofObligation::OwedIfApp(_) => None,
                    ProofObligation::OwedOverLink(owed) => Some(owed),
                    ProofObligation::OwedIfAppOverLink(owed) => match delivery {
                        Delivery::Link(link) => should_prove(&ProofRequest {
                            destination: owed.destination,
                            plaintext: link.plaintext,
                        })
                        .then_some(owed),
                        Delivery::Plain(_) | Delivery::Single(_) | Delivery::Group(_) => None,
                    },
                };
                if let Some(owed) = owed_over_link {
                    if is_egress_eligible(view, source, Egress::Transmit) {
                        let mut proof = [0u8; LINK_PROOF_WIRE_LEN];
                        if let Ok(written) = self.write_link_proof(&owed, &mut proof) {
                            sink(EngineReaction::Directive(Directive::Send {
                                target: source,
                                bytes: &proof[..written],
                            }));
                        }
                    }
                }
            }
            IngestPacketOutcome::Proof(ProofIngest::SendSingleDelivered { id, delivered }) => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendSingle(Ok(delivered)),
                }));
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::SendLinkDelivered { id, delivered }) => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendLink(Ok(delivered)),
                }));
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::SendChannelDelivered { id, delivered }) => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendChannel(Ok(delivered)),
                }));
                wake_schedule_changes.channel_timeouts = self.channel_timeouts_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::Ignored) => {}
            IngestPacketOutcome::TransportedLinkRequest {
                header,
                body,
                fire_on,
            } => {
                if is_egress_eligible(view, fire_on, Egress::Transport) {
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(header_len) = header.write(&mut buf) {
                        let wire_len = header_len + body.len;
                        buf[header_len..wire_len].copy_from_slice(body.as_bytes());
                        sink(EngineReaction::Directive(Directive::Send {
                            target: fire_on,
                            bytes: &buf[..wire_len],
                        }));
                    }
                }
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            IngestPacketOutcome::Forward(forward) => {
                if is_egress_eligible(view, forward.fire_on, Egress::Transport) {
                    let size_hint = HEADER_MAX_LEN + forward.payload.len();
                    let mut fill = |slot: &mut [u8]| forward.to_wire(slot).ok();
                    sink(EngineReaction::Directive(Directive::EmitFrame {
                        target: forward.fire_on,
                        size_hint,
                        fill: &mut fill,
                    }));
                }
            }
            IngestPacketOutcome::AnswerPathRequest { destination } => {
                if is_egress_eligible(view, source, Egress::Transmit) {
                    let mut entropy_bytes = [0u8; AnnounceEntropy::LEN];
                    fill_entropy(&mut entropy_bytes);
                    let entropy = AnnounceEntropy::new(entropy_bytes);
                    let mut response = [0u8; BROADCAST_MTU];
                    if let PathResponseWriteOutcome::Written { wire_len } =
                        self.write_path_response_announce(&destination, now, entropy, &mut response)
                    {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &response[..wire_len],
                        }));
                    }
                }
            }
            IngestPacketOutcome::ScheduledPathResponse { .. } => {
                wake_schedule_changes.scheduled_announces = self.scheduled_announces_wake();
            }
            IngestPacketOutcome::ForwardPathRequestForDiscovery { destination, id } => {
                if let Some(via) = self.transport_id {
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(wire_len) =
                        write_path_request_wire_packet(destination, Some(via), &id, &mut buf)
                    {
                        for config in view {
                            if config.id != source && config.capabilities.allows_transport() {
                                sink(EngineReaction::Directive(Directive::Send {
                                    target: config.id,
                                    bytes: &buf[..wire_len],
                                }));
                            }
                        }
                    }
                }
                wake_schedule_changes.path_request_timeout = self.path_request_timeout_wake();
            }
            IngestPacketOutcome::RelayPathRequestToLocalClients { destination, id } => {
                if let Some(via) = self.transport_id {
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(wire_len) =
                        write_path_request_wire_packet(destination, Some(via), &id, &mut buf)
                    {
                        for config in view {
                            if config.id != source
                                && config.id.kind() == Some(InterfaceKind::LocalClient)
                                && config.capabilities.allows_transport()
                            {
                                sink(EngineReaction::Directive(Directive::Send {
                                    target: config.id,
                                    bytes: &buf[..wire_len],
                                }));
                            }
                        }
                    }
                }
                wake_schedule_changes.path_request_timeout = self.path_request_timeout_wake();
            }
            IngestPacketOutcome::OwesLinkRtt {
                link_id,
                responder_encryption,
                responder_signing,
                command_id,
                rtt,
                mtu,
            } => {
                if is_egress_eligible(view, source, Egress::Transmit) {
                    let mut iv = [0u8; ENCRYPTION_IV_LEN];
                    fill_entropy(&mut iv);
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(written) = self.write_owed_link_rtt(
                        &link_id,
                        &responder_encryption,
                        rtt,
                        mtu.min(link_mtu_ceiling(view, source)),
                        source,
                        now,
                        responder_signing,
                        &iv,
                        &mut buf,
                    ) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &buf[..written],
                        }));
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id: command_id,
                            settlement: Settlement::EstablishLink(Ok(LinkEstablished {
                                link_id,
                                rtt_ms: rtt.millis(),
                            })),
                        }));
                    }
                    wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
                }
            }
            IngestPacketOutcome::RequestReceived {
                link_id,
                request_id,
                path_hash,
                requested_at,
                data,
            } => {
                sink(EngineReaction::Journaled(Journaled::RequestReceived {
                    link_id,
                    request_id,
                    path_hash,
                    requested_at,
                    data,
                }));
            }
            IngestPacketOutcome::ResponseSettled {
                id,
                delivered,
                link_id,
                request_id,
                data,
            } => {
                sink(EngineReaction::Journaled(Journaled::ResponseReceived {
                    command_id: id,
                    link_id,
                    request_id,
                    data,
                }));
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendRequest(Ok(delivered)),
                }));
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::ChannelDataReceived {
                link_id,
                message_type,
                sequence,
                payload,
                packet_hash,
            } => {
                let outcome = channel_receive(
                    &mut self.channels,
                    &link_id,
                    sequence,
                    message_type,
                    payload,
                    |message_type, data| {
                        sink(EngineReaction::Journaled(
                            Journaled::ChannelMessageReceived {
                                link_id,
                                message_type,
                                data,
                            },
                        ));
                    },
                );
                if outcome.owes_proof() && is_egress_eligible(view, source, Egress::Transmit) {
                    let mut proof = [0u8; LINK_PROOF_WIRE_LEN];
                    if let Ok(written) = self.write_channel_ack(&link_id, &packet_hash, &mut proof)
                    {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &proof[..written],
                        }));
                    }
                }
            }
            IngestPacketOutcome::OwesResourceParts {
                link_id,
                hash,
                requested,
                exhausted_at,
            } => {
                self.serve_resource_request(
                    &link_id,
                    &hash,
                    requested,
                    exhausted_at,
                    source,
                    now,
                    fill_entropy,
                    sink,
                );
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::OwesResourcePull { link_id, hash } => {
                self.emit_resource_pull(&link_id, &hash, now, fill_entropy, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::OwesResourceAssembly { link_id, hash } => {
                self.conclude_resource(&link_id, &hash, now, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::ResourceConcludedFailed { link_id, hash } => {
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id,
                    hash,
                }));
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceRejectedByPeer { id } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendResource(Err(
                        crate::engine::SendResourceFailure::RejectedByPeer,
                    )),
                }));
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceDelivered { id } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendResource(Ok(())),
                }));
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::PeerIdentified { link_id, identity } => {
                sink(EngineReaction::Journaled(Journaled::PeerIdentified {
                    link_id,
                    identity,
                }));
            }
            IngestPacketOutcome::LinkActivated { link_id, rtt_ms } => {
                sink(EngineReaction::Journaled(Journaled::LinkEstablished(
                    LinkEstablished { link_id, rtt_ms },
                )));
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            IngestPacketOutcome::OwesLinkProof {
                request,
                identity,
                proof_strategy,
                received_hops,
                arrived_at,
            } => {
                if is_egress_eligible(view, source, Egress::Transmit) {
                    let mut secret_bytes = [0u8; 32];
                    fill_entropy(&mut secret_bytes);
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(written) = self.write_owed_link_proof(
                        &request,
                        &identity,
                        proof_strategy,
                        received_hops,
                        arrived_at,
                        X25519SecretKey::new(secret_bytes),
                        link_mtu_ceiling(view, source),
                        &mut buf,
                    ) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &buf[..written],
                        }));
                    }
                    wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
                }
            }
            IngestPacketOutcome::OwesKeepaliveEcho { link_id } => {
                if is_egress_eligible(view, source, Egress::Transmit) {
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(written) = write_keepalive(&link_id, KEEPALIVE_ECHO, &mut buf) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &buf[..written],
                        }));
                    }
                }
            }
            IngestPacketOutcome::LinkClosedByPeer { link_id } => {
                sink(EngineReaction::Journaled(Journaled::LinkClosed {
                    link_id,
                    reason: LinkClosedReason::PeerClosed,
                }));
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            IngestPacketOutcome::OwesLinkClose { link_id, reason } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                if let Ok(dispatch) = self.write_owed_link_close(&link_id, &iv, &mut buf) {
                    let target = dispatch.fire_on.unwrap_or(source);
                    if is_egress_eligible(view, target, Egress::Transmit) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target,
                            bytes: &buf[..dispatch.wire_len],
                        }));
                    }
                    sink(EngineReaction::Journaled(Journaled::LinkClosed {
                        link_id,
                        reason,
                    }));
                }
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            IngestPacketOutcome::LinkInterfaceMismatch {
                link_id,
                attached_interface,
                arrived_on,
            } => {
                sink(EngineReaction::Journaled(
                    Journaled::LinkInterfaceMismatch {
                        link_id,
                        attached_interface,
                        arrived_on,
                    },
                ));
            }
            IngestPacketOutcome::Ignored => {}
        }
        wake_schedule_changes
    }
}

/// Which capability a directed emit needs from its target interface — the same gate the
/// legacy runtime's `fan_to_handles` applied to a single listed target.
#[derive(Clone, Copy)]
pub(crate) enum Egress {
    /// Self-originated traffic (a proof, our own path response).
    Transmit,
    /// Relayed traffic (a forward, a cached path response).
    Transport,
}

pub(crate) fn is_egress_eligible(
    interfaces: &[InterfaceConfig],
    target: InterfaceId,
    egress_kind: Egress,
) -> bool {
    interfaces
        .iter()
        .find(|config| config.id == target)
        .is_some_and(|config| match egress_kind {
            Egress::Transmit => config.capabilities.allows_transmit(),
            Egress::Transport => config.capabilities.allows_transport(),
        })
}

#[cfg(test)]
mod channel_tests {
    use super::*;
    use crate::crypto::{
        ed25519_public_key, ed25519_verify, x25519_diffie_hellman, Ed25519PublicKey,
        Ed25519SecretKey, Ed25519Signature, X25519PublicKey, X25519SecretKey,
    };
    use crate::engine::commands::CommandId;
    use crate::engine::test_support::{transporting_view, Cap, TEST_ENTROPY};
    use crate::engine::{Directive, EngineReaction};
    use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
    use crate::routing::links::channel::{write_envelope, ChannelSequence, MessageType};
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::table::InitiatedLink;
    use crate::routing::links::{LinkId, LinkKey};
    use crate::routing::proof::LINK_PROOF_WIRE_LEN;
    use crate::wire::{
        DestinationHash, DestinationType, PacketType, WireContext, BROADCAST_MTU, HEADER_MIN_LEN,
    };
    use std::vec::Vec;

    const LANE: [u8; 8] = [0xEE; 8];

    fn shared() -> crate::crypto::X25519SharedSecret {
        x25519_diffie_hellman(
            &X25519SecretKey::new([0x33; 32]),
            &X25519PublicKey([0x44; 32]),
        )
    }

    /// An engine holding one active link we initiated, with a known link signing
    /// key, plus that link's session key for sealing packets it will open.
    fn active_initiator() -> (EngineState<Cap>, LinkId, LinkKey, Ed25519PublicKey) {
        let link_id = LinkId::new([0x5C; 16]);
        let link_signing = Ed25519SecretKey::new([0x42; 32]);
        let link_signing_public = ed25519_public_key(&link_signing);
        let mut state = EngineState::<Cap>::default();
        state
            .links
            .track_initiated(InitiatedLink {
                link_id,
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                link_signing,
                requested_at: InstantMillis(0),
                timeout_at: InstantMillis(5_000),
                command_id: CommandId(1),
            })
            .unwrap();
        state
            .links
            .activate_initiated(
                &link_id,
                LinkKey::derive(&link_id, &shared()),
                crate::units::Rtt(250),
                BROADCAST_MTU,
                InterfaceId::new(LANE),
                InstantMillis(1_000),
                Ed25519PublicKey([0x99; 32]),
            )
            .unwrap();
        (
            state,
            link_id,
            LinkKey::derive(&link_id, &shared()),
            link_signing_public,
        )
    }

    fn channel_frame(
        key: &LinkKey,
        link_id: &LinkId,
        message_type: MessageType,
        sequence: ChannelSequence,
        body: &[u8],
    ) -> Vec<u8> {
        let mut envelope = [0u8; BROADCAST_MTU];
        let env_len = write_envelope(message_type, sequence, body, &mut envelope).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let len = write_link_packet(
            link_id,
            key,
            BROADCAST_MTU,
            WireContext::Channel,
            &envelope[..env_len],
            &[0u8; 16],
            &mut frame,
        )
        .unwrap();
        frame[..len].to_vec()
    }

    /// The messages a fed channel packet journals (in order) paired with the ack
    /// directive it emits, if any.
    type FeedOutcome = (Vec<(MessageType, Vec<u8>)>, Option<Vec<u8>>);

    /// Feed one already-framed channel packet and collect the messages it
    /// journals (in order) and the ack directive it emits, if any.
    fn feed(state: &mut EngineState<Cap>, frame: &[u8], now: u64) -> FeedOutcome {
        let mut raw = frame.to_vec();
        let mut messages = Vec::new();
        let mut ack = None;
        state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(now),
                source_interface: InterfaceId::new(LANE),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
            InstantMillis(now),
            &mut |bytes: &mut [u8]| bytes.fill(0),
            &mut |_| false,
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::ChannelMessageReceived {
                    message_type,
                    data,
                    ..
                }) => messages.push((message_type, data.to_vec())),
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    ack = Some(bytes.to_vec())
                }
                _ => {}
            },
        );
        (messages, ack)
    }

    fn assert_valid_ack(
        ack: &[u8],
        ciphertext: &[u8],
        link_id: &LinkId,
        signer: &Ed25519PublicKey,
    ) {
        assert_eq!(
            ack.len(),
            LINK_PROOF_WIRE_LEN,
            "the ack is one explicit proof"
        );
        let expected = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &DestinationHash::new(*link_id.as_bytes()),
            WireContext::Channel,
            ciphertext,
        );
        assert_eq!(
            &ack[HEADER_MIN_LEN..HEADER_MIN_LEN + PACKET_HASH_LEN],
            expected.as_bytes(),
            "the ack names the packet it proves",
        );
        let signature = Ed25519Signature(
            ack[HEADER_MIN_LEN + PACKET_HASH_LEN..LINK_PROOF_WIRE_LEN]
                .try_into()
                .unwrap(),
        );
        ed25519_verify(signer, expected.as_bytes(), &signature)
            .expect("the ack verifies against the initiator's link signing key");
    }

    #[test]
    fn an_in_order_channel_message_is_journaled_and_unconditionally_acked() {
        let (mut state, link_id, key, signer) = active_initiator();
        let frame = channel_frame(
            &key,
            &link_id,
            MessageType(7),
            ChannelSequence(0),
            b"hello channel",
        );
        let ciphertext = frame[HEADER_MIN_LEN..].to_vec();

        let (messages, ack) = feed(&mut state, &frame, 2_000);
        assert_eq!(
            messages,
            std::vec![(MessageType(7), b"hello channel".to_vec())],
            "the message is delivered to the journal in order",
        );
        assert_valid_ack(
            &ack.expect("a channel arrival owes an ack even when should_prove says no"),
            &ciphertext,
            &link_id,
            &signer,
        );
    }

    #[test]
    fn a_gap_then_its_fill_journals_the_whole_run_in_order() {
        let (mut state, link_id, key, _signer) = active_initiator();

        let ahead = channel_frame(&key, &link_id, MessageType(1), ChannelSequence(1), b"one");
        let (messages, ack) = feed(&mut state, &ahead, 2_000);
        assert!(
            messages.is_empty(),
            "the out-of-order arrival waits for the gap"
        );
        assert!(
            ack.is_some(),
            "but it is still acked so the sender stops resending"
        );

        let gap = channel_frame(&key, &link_id, MessageType(0), ChannelSequence(0), b"zero");
        let (messages, ack) = feed(&mut state, &gap, 2_100);
        assert_eq!(
            messages,
            std::vec![
                (MessageType(0), b"zero".to_vec()),
                (MessageType(1), b"one".to_vec()),
            ],
            "filling the gap drains the buffered run in one arrival, in sequence order",
        );
        assert!(ack.is_some(), "the gap-filling arrival is acked too");
    }
}
