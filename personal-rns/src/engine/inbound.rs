use crate::engine::{
    AnnounceIngest, CachedPathResponseOutcome, Directive, EngineReaction, EngineState,
    IngestPacketOutcome, InstantMillis, Journaled, PathFound, PathResponseWriteOutcome,
    ProofIngest, Settlement, WakeSchedules,
};
use crate::interfaces::{InboundPacket, InterfaceConfig, InterfaceId};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::AnnounceEntropy;
use crate::routing::proof::IMPLICIT_PROOF_WIRE_LEN;
use crate::routing::storage::EngineStorage;
use crate::routing::{RemovedRoute, RouteRemovalCause};
use crate::wire::MTU;

pub(crate) fn journal_removal(removed: RemovedRoute) -> Journaled<'static> {
    match removed.cause {
        RouteRemovalCause::Expired => Journaled::RouteExpired {
            destination: removed.destination,
        },
        RouteRemovalCause::Evicted => Journaled::RouteEvicted {
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
    /// announce can schedule a rebroadcast, settle waiting path requests, and move the route
    /// expiry (as can an ignored one whose insert attempt culled or evicted before dropping),
    /// an arriving proof retires a send-timeout; everything else is `Unchanged`.
    pub fn ingest_packet_into<F>(
        &mut self,
        packet: InboundPacket<'_>,
        jitter: JitterSeed,
        view: &[InterfaceConfig],
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let source = packet.source_interface;
        let mut delta = WakeSchedules::UNCHANGED;
        let mut routes_removed = false;
        let outcome = self.ingest_packet_with(packet, jitter, view, &mut |removed| {
            routes_removed = true;
            sink(EngineReaction::Journaled(journal_removal(removed)));
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
                delta.rebroadcast_announces = self.rebroadcast_lane();
                delta.path_request_timeout = self.path_timeout_lane();
                delta.expired_routes = self.route_expiry_lane(view);
            }
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored) => {
                if routes_removed {
                    delta.expired_routes = self.route_expiry_lane(view);
                }
            }
            IngestPacketOutcome::Delivery {
                delivery,
                maybe_owed_proof,
            } => {
                sink(EngineReaction::Journaled(Journaled::Delivered(delivery)));
                if let Some(owed) = maybe_owed_proof {
                    if directed_eligible(view, source, Egress::Transmit) {
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
                delta.send_single_timeout = self.send_timeout_lane();
            }
            IngestPacketOutcome::Proof(ProofIngest::Ignored) => {}
            IngestPacketOutcome::Forward(forward) => {
                if directed_eligible(view, forward.fire_on, Egress::Transport) {
                    let mut buf = [0u8; MTU];
                    if let Ok(written) = forward.to_wire(&mut buf) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: forward.fire_on,
                            bytes: &buf[..written],
                        }));
                    }
                }
            }
            IngestPacketOutcome::AnswerPathRequest { destination } => {
                if directed_eligible(view, source, Egress::Transmit) {
                    let mut entropy_bytes = [0u8; AnnounceEntropy::LEN];
                    fill_entropy(&mut entropy_bytes);
                    let entropy = AnnounceEntropy::new(entropy_bytes);
                    let mut response = [0u8; MTU];
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
            IngestPacketOutcome::AnswerPathRequestFromCache { destination } => {
                if directed_eligible(view, source, Egress::Transport) {
                    let mut response = [0u8; MTU];
                    if let CachedPathResponseOutcome::Written { wire_len } =
                        self.write_cached_path_response(&destination, &mut response)
                    {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &response[..wire_len],
                        }));
                    }
                }
            }
            IngestPacketOutcome::Ignored => {}
        }
        delta
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

fn directed_eligible(view: &[InterfaceConfig], target: InterfaceId, egress: Egress) -> bool {
    view.iter()
        .find(|config| config.id == target)
        .is_some_and(|config| match egress {
            Egress::Transmit => config.capabilities.allows_transmit(),
            Egress::Transport => config.capabilities.allows_transport(),
        })
}
