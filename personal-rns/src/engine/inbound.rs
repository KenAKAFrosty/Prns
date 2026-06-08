use crate::engine::proof::IMPLICIT_PROOF_WIRE_LEN;
use crate::engine::{
    AnnounceIngest, CachedPathResponseOutcome, Directive, EngineReaction, EngineState,
    IngestPacketOutcome, InstantMillis, Journaled, PathFound, PathResponseWriteOutcome,
    ProofIngest, Settlement,
};
use crate::interfaces::{ConnectionState, InboundPacket, InterfaceDescriptor, InterfaceId};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::SelfAnnounceEntropy;
use crate::routing::storage::EngineStorage;
use crate::wire::MTU;

impl<S: EngineStorage> EngineState<S> {
    /// Ingest one packet and stream everything it produces to `sink`: the `Journaled`
    /// facts (announce heard, delivery, the settlements a learned route closes) and the
    /// `Directive`s it owes — a proof back on the arrival lane, a packet forwarded onward,
    /// a path response. This is the sink-shaped inbound edge: it folds in the follow-up the
    /// legacy runtime ran after `ingest_packet`, so the reactor's inbound arm just forwards
    /// the stream. `fill_entropy` is pulled only when a path response is actually minted.
    pub fn ingest_packet_into<F>(
        &mut self,
        packet: InboundPacket<'_>,
        jitter: JitterSeed,
        view: &[InterfaceDescriptor],
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let source = packet.source_interface;
        match self.ingest_packet(packet, jitter, view) {
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
            }
            IngestPacketOutcome::Announce(
                AnnounceIngest::HeldForRetry | AnnounceIngest::Ignored,
            ) => {}
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
                    let mut entropy_bytes = [0u8; SelfAnnounceEntropy::LEN];
                    fill_entropy(&mut entropy_bytes);
                    let entropy = SelfAnnounceEntropy::new(entropy_bytes);
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

fn directed_eligible(view: &[InterfaceDescriptor], target: InterfaceId, egress: Egress) -> bool {
    view.iter()
        .find(|descriptor| descriptor.id == target)
        .is_some_and(|descriptor| {
            matches!(
                descriptor.state,
                ConnectionState::Connected | ConnectionState::Degraded
            ) && match egress {
                Egress::Transmit => descriptor.capabilities.allows_transmit(),
                Egress::Transport => descriptor.capabilities.allows_transport(),
            }
        })
}
