use crate::crypto::X25519SecretKey;
use crate::engine::reaction::LinkClosedReason;
use crate::engine::{
    AnnounceIngest, Directive, EngineReaction, EngineState, IngestPacketOutcome, InstantMillis,
    Journaled, LaneWake, LinkEstablished, PathFound, PathResponseWriteOutcome, ProofIngest,
    Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{InboundPacket, InterfaceConfig, InterfaceId};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::AnnounceEntropy;
use crate::routing::delivery::Delivery;
use crate::routing::links::establish::link_mtu_ceiling;
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_ECHO};
use crate::routing::proof::{
    ProofObligation, ProofRequest, IMPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN,
};
use crate::routing::storage::EngineStorage;
use crate::routing::{RemovedRoute, RouteRemovalCause};
use crate::wire::BROADCAST_MTU;

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

impl<S: EngineStorage> EngineState<S> {
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
                            hops: accepted.hops,
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
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(written) = forward.to_wire(&mut buf) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: forward.fire_on,
                            bytes: &buf[..written],
                        }));
                    }
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
            IngestPacketOutcome::OwesLinkRtt {
                link_id,
                responder_encryption,
                responder_signing,
                command_id,
                rtt_ms,
                mtu,
            } => {
                if is_egress_eligible(view, source, Egress::Transmit) {
                    let mut iv = [0u8; ENCRYPTION_IV_LEN];
                    fill_entropy(&mut iv);
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(written) = self.write_owed_link_rtt(
                        &link_id,
                        &responder_encryption,
                        rtt_ms,
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
                                rtt_ms,
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
                    fill_entropy,
                    sink,
                );
            }
            IngestPacketOutcome::OwesResourcePull { link_id, hash } => {
                self.emit_resource_pull(&link_id, &hash, fill_entropy, sink);
            }
            IngestPacketOutcome::OwesResourceAssembly { link_id, hash } => {
                self.conclude_resource(&link_id, &hash, sink);
            }
            IngestPacketOutcome::ResourceConcludedFailed { link_id, hash } => {
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id,
                    hash,
                }));
            }
            IngestPacketOutcome::ResourceDelivered { id } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendResource(Ok(())),
                }));
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
