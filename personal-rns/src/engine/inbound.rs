use crate::engine::{
    AnnounceIngest, Directive, EngineReaction, EngineState, IngestPacketOutcome, InstantMillis,
    Journaled, LaneWake, PathFound, PathResponseWriteOutcome, ProofIngest, Settlement,
    WakeSchedules,
};
use crate::interfaces::{InboundPacket, InterfaceConfig, InterfaceId};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::AnnounceEntropy;
use crate::routing::delivery::Delivery;
use crate::routing::proof::{ProofObligation, ProofRequest, IMPLICIT_PROOF_WIRE_LEN};
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
                    ProofObligation::None => None,
                    ProofObligation::Owed(owed) => Some(owed),
                    ProofObligation::OwedIfApp(owed) => match delivery {
                        Delivery::Single(single) => should_prove(&ProofRequest {
                            destination: single.destination,
                            plaintext: single.plaintext,
                        })
                        .then_some(owed),
                        Delivery::Plain(_) | Delivery::Group(_) => None,
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
            }
            IngestPacketOutcome::Proof(ProofIngest::SendSingleDelivered { id, delivered }) => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendSingle(Ok(delivered)),
                }));
                wake_schedule_changes.send_single_timeout =
                    self.send_single_receipts_timeout_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::Ignored) => {}
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
            IngestPacketOutcome::Ignored => {}
        }
        wake_schedule_changes
    }
}

/// Which capability a directed emit needs from its target interface — the same gate the
/// legacy runtime's `fan_to_handles` applied to a single listed target.
#[derive(Clone, Copy)]
enum Egress {
    /// Self-originated traffic (a proof, our own path response).
    Transmit,
    /// Relayed traffic (a forward, a cached path response).
    Transport,
}

fn is_egress_eligible(view: &[InterfaceConfig], target: InterfaceId, egress: Egress) -> bool {
    view.iter()
        .find(|config| config.id == target)
        .is_some_and(|config| match egress {
            Egress::Transmit => config.capabilities.allows_transmit(),
            Egress::Transport => config.capabilities.allows_transport(),
        })
}
