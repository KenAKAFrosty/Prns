use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled, PrnsCommand,
    Respond, RespondData, SendRequest, SendRequestData, SendSinglePacketEntropy,
    SendSinglePacketFailure, SendSinglePacketPrepared, SendSinglePacketWriteError, Settlement,
    WakeSchedules,
};
use crate::manifold::Host;
use crate::routing::links::request::{write_request_plaintext, RequestId, REQUEST_WIRE_OVERHEAD};
use crate::routing::links::resources::{
    ResourceBody, ResourceCorrelation, ResourceMetadata, ResourceSegment, ResourceSend,
};
use crate::runtime::node_introspection::NodeIntrospectionRequest;
#[cfg(feature = "runtime-metrics")]
use crate::runtime::RuntimeMetricsSnapshot;
use crate::runtime::{
    apply_destination_identity_retention_command, apply_identity_blackhole_command,
    ClearAnnounceQueuesOutcome,
};
use crate::storage::StorageLayout;
use prns_runtime::runtime::persistence_snapshots;

use super::crypto_pool::{CryptoJob, CryptoPool};
use super::egress::{clear_announce_queues, route_reaction, WireScratch};
use super::host_protocol::{HostCommand, HostResourcePayload, RequestAnyHostCommand};
use super::interface_topology::InterfaceTopology;
use super::journal_delivery::JournalDispatch;

pub(super) enum CommandEffect {
    Delta(WakeSchedules),
    RecomputeWakeSchedules,
    InterfaceAttached {
        id: crate::interfaces::InterfaceId,
        frame_capacity: usize,
    },
}

impl CommandEffect {
    const UNCHANGED: Self = Self::Delta(WakeSchedules::UNCHANGED);
}

pub(super) struct CommandDispatch<'a, S, H, J>
where
    S: StorageLayout,
    H: Host,
    J: for<'b> FnMut(Journaled<'b>),
{
    pub(super) engine: &'a mut EngineState<S>,
    pub(super) host: &'a mut H,
    pub(super) topology: &'a mut InterfaceTopology,
    pub(super) wire_scratch: &'a mut WireScratch,
    pub(super) journal: &'a mut JournalDispatch<J>,
    pub(super) crypto_pool: Option<&'a CryptoPool>,
}

impl<S, H, J> CommandDispatch<'_, S, H, J>
where
    S: StorageLayout,
    H: Host,
    J: for<'a> FnMut(Journaled<'a>),
{
    pub(super) fn dispatch(self, command: HostCommand, now: InstantMillis) -> CommandEffect {
        let Self {
            engine,
            host,
            topology,
            wire_scratch,
            journal,
            crypto_pool,
        } = self;
        macro_rules! journaled_sink {
            () => {
                |journaled| journal.route(journaled)
            };
        }
        macro_rules! reaction_sink {
            () => {
                |reaction| {
                    route_reaction(
                        reaction,
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        now,
                        &mut journaled_sink!(),
                    )
                }
            };
        }
        macro_rules! defer_send_single_packet {
            ($pool:expr, $id:expr, $send:expr) => {{
                let mut entropy_bytes = [0u8; SendSinglePacketEntropy::LEN];
                host.fill_entropy(&mut entropy_bytes);
                match engine.prepare_send_single_packet_deferred(
                    $id,
                    $send,
                    now,
                    SendSinglePacketEntropy::new(entropy_bytes),
                ) {
                    SendSinglePacketPrepared::Owed(owed) => {
                        $pool.submit(CryptoJob::SealScalars(owed));
                    }
                    SendSinglePacketPrepared::Rejected { id, rejection } => {
                        route_reaction(
                            EngineReaction::Journaled(Journaled::CommandSettled {
                                id,
                                settlement: Settlement::SendSinglePacket(Err(
                                    SendSinglePacketFailure::Rejected(rejection),
                                )),
                            }),
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            now,
                            &mut journaled_sink!(),
                        );
                    }
                    SendSinglePacketPrepared::RouteVanished { id } => {
                        route_reaction(
                            EngineReaction::Journaled(Journaled::CommandSettled {
                                id,
                                settlement: Settlement::SendSinglePacket(Err(
                                    SendSinglePacketFailure::WriteFailed(
                                        SendSinglePacketWriteError::RouteVanished,
                                    ),
                                )),
                            }),
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            now,
                            &mut journaled_sink!(),
                        );
                    }
                }
                CommandEffect::UNCHANGED
            }};
        }

        match command {
            HostCommand::Engine(issued) => {
                let id = issued.id;
                match (crypto_pool, issued.command) {
                    (Some(pool), PrnsCommand::SendSinglePacket(send)) => {
                        defer_send_single_packet!(pool, id, send)
                    }
                    (_, command) => CommandEffect::Delta(engine.ingest_command_into(
                        IssuedCommand { id, command },
                        topology.interfaces.view(),
                        now,
                        &mut |entropy| host.fill_entropy(entropy),
                        &mut reaction_sink!(),
                    )),
                }
            }
            HostCommand::AwaitedEngine { issued, completion } => {
                let id = issued.id;
                journal.register_completion(id, completion);
                match (crypto_pool, issued.command) {
                    (Some(pool), PrnsCommand::SendSinglePacket(send)) => {
                        defer_send_single_packet!(pool, id, send)
                    }
                    (_, command) => CommandEffect::Delta(engine.ingest_command_into(
                        IssuedCommand { id, command },
                        topology.interfaces.view(),
                        now,
                        &mut |entropy| host.fill_entropy(entropy),
                        &mut reaction_sink!(),
                    )),
                }
            }
            HostCommand::SendResource(send) => CommandEffect::Delta(
                engine.ingest_send_resource_into(
                    &ResourceSend {
                        id: send.id,
                        link_id: send.link_id,
                        body: ResourceBody {
                            data: send.data.as_slice(),
                            compressed_candidate: send
                                .compressed_candidate
                                .as_ref()
                                .map(HostResourcePayload::as_slice),
                            metadata: send.metadata.as_engine(),
                        },
                        correlation: send.request_id.map_or(
                            ResourceCorrelation::Unsolicited,
                            ResourceCorrelation::Response,
                        ),
                    },
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut reaction_sink!(),
                ),
            ),
            HostCommand::SendResourceSegment(send) => {
                journal.register_completion(send.id, send.completion);
                CommandEffect::Delta(
                    engine.ingest_send_resource_segment_into(
                        &ResourceSend {
                            id: send.id,
                            link_id: send.link_id,
                            body: ResourceBody {
                                data: send.data.as_slice(),
                                compressed_candidate: send
                                    .compressed_candidate
                                    .as_ref()
                                    .map(HostResourcePayload::as_slice),
                                metadata: send.metadata.as_engine(),
                            },
                            correlation: send.request_id.map_or(
                                ResourceCorrelation::Unsolicited,
                                ResourceCorrelation::Response,
                            ),
                        },
                        ResourceSegment {
                            index: send.segment_index,
                            total_segments: send.total_segments,
                            total_data_bytes: send.total_data_bytes,
                        },
                        now,
                        &mut |entropy| host.fill_entropy(entropy),
                        &mut reaction_sink!(),
                    ),
                )
            }
            HostCommand::RespondAny(mut respond) => {
                if let Some(completion) = respond.completion.take() {
                    journal.register_completion(respond.id, completion);
                }
                let data = respond.packed.as_slice();
                let as_packet = engine
                    .response_fits_packet(&respond.link_id, data)
                    .then(|| RespondData::from_slice(data).ok())
                    .flatten();
                let delta = match as_packet {
                    Some(data) => engine.ingest_command_into(
                        IssuedCommand {
                            id: respond.id,
                            command: PrnsCommand::Respond(Respond {
                                link_id: respond.link_id,
                                request_id: respond.request_id,
                                payload: crate::engine::RespondPayload::Packed(data),
                            }),
                        },
                        topology.interfaces.view(),
                        now,
                        &mut |entropy| host.fill_entropy(entropy),
                        &mut reaction_sink!(),
                    ),
                    None => engine.ingest_send_resource_into(
                        &ResourceSend {
                            id: respond.id,
                            link_id: respond.link_id,
                            body: ResourceBody {
                                data,
                                compressed_candidate: respond
                                    .compressed_candidate
                                    .as_ref()
                                    .map(HostResourcePayload::as_slice),
                                metadata: ResourceMetadata::None,
                            },
                            correlation: ResourceCorrelation::Response(respond.request_id),
                        },
                        now,
                        &mut |entropy| host.fill_entropy(entropy),
                        &mut reaction_sink!(),
                    ),
                };
                CommandEffect::Delta(delta)
            }
            HostCommand::RequestAny(request) => {
                let RequestAnyHostCommand {
                    id,
                    link_id,
                    path_hash,
                    data,
                    response_timeout,
                    maximum_response_bytes,
                    completion,
                } = request;
                journal.register_request(id, completion);
                let payload = data.as_slice();
                let delta = if engine.request_fits_packet(&link_id, payload) {
                    match SendRequestData::from_slice(payload) {
                        Ok(send_data) => engine.ingest_command_into(
                            IssuedCommand {
                                id,
                                command: PrnsCommand::SendRequest(SendRequest {
                                    link_id,
                                    path_hash,
                                    data: send_data,
                                    response_timeout,
                                    maximum_response_bytes,
                                }),
                            },
                            topology.interfaces.view(),
                            now,
                            &mut |entropy| host.fill_entropy(entropy),
                            &mut reaction_sink!(),
                        ),
                        Err(_) => journal.fail_request(id),
                    }
                } else {
                    let mut packed = std::vec![0u8; REQUEST_WIRE_OVERHEAD + payload.len().max(1)];
                    match write_request_plaintext(now, &path_hash, payload, &mut packed) {
                        Ok(plain_len) => {
                            let packed_request = &packed[..plain_len];
                            let request_id = RequestId::of_request_data(packed_request);
                            engine.ingest_send_resource_into(
                                &ResourceSend {
                                    id,
                                    link_id,
                                    body: ResourceBody {
                                        data: packed_request,
                                        compressed_candidate: None,
                                        metadata: ResourceMetadata::None,
                                    },
                                    correlation: ResourceCorrelation::Request {
                                        id: request_id,
                                        response_timeout,
                                        maximum_response_bytes,
                                    },
                                },
                                now,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut reaction_sink!(),
                            )
                        }
                        Err(_) => journal.fail_request(id),
                    }
                };
                CommandEffect::Delta(delta)
            }
            HostCommand::ProvideDecompressed(provide) => {
                CommandEffect::Delta(engine.provide_decompressed(
                    provide.link_id,
                    provide.hash,
                    provide.plaintext.as_slice(),
                    now,
                    &mut reaction_sink!(),
                ))
            }
            HostCommand::AddInterface(add) => match topology.attach(engine, add, now) {
                Some((id, frame_capacity)) => {
                    CommandEffect::InterfaceAttached { id, frame_capacity }
                }
                None => CommandEffect::UNCHANGED,
            },
            HostCommand::RemoveInterface { id, departure } => {
                topology.detach(engine, id, departure, now);
                CommandEffect::RecomputeWakeSchedules
            }
            HostCommand::DropRoute { destination, reply } => {
                let effect = engine.drop_route(&destination, topology.view());
                if let Some(removed) = effect.removed_route() {
                    journal.route(Journaled::RouteRemoved {
                        destination: removed.destination,
                        cause: removed.cause,
                    });
                }
                let _ = reply.send(effect.outcome());
                CommandEffect::Delta(effect.wake_schedules())
            }
            HostCommand::DropRoutesVia { transport, reply } => {
                let effect = engine.drop_routes_via(transport, topology.view(), &mut |removed| {
                    journal.route(Journaled::RouteRemoved {
                        destination: removed.destination,
                        cause: removed.cause,
                    });
                });
                let _ = reply.send(effect.outcome());
                CommandEffect::Delta(effect.wake_schedules())
            }
            HostCommand::ClearAnnounceQueues { reply } => {
                let dropped = clear_announce_queues(&mut topology.pacers);
                let _ = reply.send(ClearAnnounceQueuesOutcome {
                    dropped_announces: u32::try_from(dropped).unwrap_or(u32::MAX),
                });
                CommandEffect::UNCHANGED
            }
            HostCommand::IdentityBlackhole(command) => {
                CommandEffect::Delta(apply_identity_blackhole_command(
                    engine,
                    command,
                    topology.view(),
                    &mut |removed| {
                        journal.route(Journaled::RouteRemoved {
                            destination: removed.destination,
                            cause: removed.cause,
                        });
                    },
                ))
            }
            HostCommand::DestinationIdentityRetention(command) => CommandEffect::Delta(
                apply_destination_identity_retention_command(engine, command, now),
            ),
            HostCommand::NodeIntrospection(request) => {
                match request {
                    NodeIntrospectionRequest::LinkCount { reply } => {
                        let _ = reply.send(engine.link_count());
                    }
                    NodeIntrospectionRequest::AnnounceRates { reply } => {
                        let mut snapshots = std::vec::Vec::new();
                        engine.visit_announce_rate_states(|state| {
                            snapshots.push(journal.announce_rate_snapshot(state));
                        });
                        let _ = reply.send(snapshots);
                    }
                    NodeIntrospectionRequest::Routes { reply } => {
                        let mut snapshots = std::vec::Vec::new();
                        engine.visit_route_snapshots(topology.view(), |snapshot| {
                            snapshots.push(snapshot);
                        });
                        let _ = reply.send(snapshots);
                    }
                    NodeIntrospectionRequest::Route { destination, reply } => {
                        let _ = reply.send(engine.route_snapshot(destination, topology.view()));
                    }
                    NodeIntrospectionRequest::DestinationIdentityHash { destination, reply } => {
                        let identity = engine
                            .destination_identity(&destination)
                            .map(|entry| entry.identity);
                        let _ = reply.send(identity);
                    }
                    NodeIntrospectionRequest::DestinationIdentity { query, reply } => {
                        use crate::node_introspection::{
                            DestinationIdentityQuery, DestinationIdentitySnapshot,
                        };

                        let entry = match query {
                            DestinationIdentityQuery::Destination(destination) => {
                                engine.destination_identity(&destination)
                            }
                            DestinationIdentityQuery::Identity(identity) => engine
                                .destination_identities()
                                .find(|entry| entry.identity == identity),
                        };
                        let snapshot = entry.map(|entry| DestinationIdentitySnapshot {
                            destination: entry.destination,
                            identity: entry.identity,
                            public: crate::identity::PublicIdentityMaterial::from_bytes(
                                entry.public_keys.public_key_bytes(),
                            ),
                        });
                        let _ = reply.send(snapshot);
                    }
                    NodeIntrospectionRequest::DestinationIdentities { reply } => {
                        let snapshots = engine
                            .destination_identities()
                            .map(
                                |entry| crate::node_introspection::DestinationIdentitySnapshot {
                                    destination: entry.destination,
                                    identity: entry.identity,
                                    public: crate::identity::PublicIdentityMaterial::from_bytes(
                                        entry.public_keys.public_key_bytes(),
                                    ),
                                },
                            )
                            .collect();
                        let _ = reply.send(snapshots);
                    }
                    NodeIntrospectionRequest::EngineSnapshot { reply } => {
                        let mut routes = std::vec::Vec::new();
                        engine.visit_route_snapshots(topology.view(), |snapshot| {
                            routes.push(snapshot);
                        });
                        let destination_identities = engine
                            .destination_identities()
                            .map(
                                |entry| crate::node_introspection::DestinationIdentitySnapshot {
                                    destination: entry.destination,
                                    identity: entry.identity,
                                    public: crate::identity::PublicIdentityMaterial::from_bytes(
                                        entry.public_keys.public_key_bytes(),
                                    ),
                                },
                            )
                            .collect();
                        let _ = reply.send(crate::node_introspection::EngineInspectionSnapshot {
                            link_count: engine.link_count(),
                            routes,
                            destination_identities,
                        });
                    }
                }
                CommandEffect::UNCHANGED
            }
            HostCommand::SynthesizeTunnel { interface } => {
                let mut random_hash = [0u8; crate::routing::tunnel::RANDOM_HASH_LEN];
                host.fill_entropy(&mut random_hash);
                let mut buf = [0u8; 256];
                if let Ok(len) = engine.write_tunnel_synthesize(interface, &random_hash, &mut buf) {
                    route_reaction(
                        EngineReaction::Directive(Directive::Send {
                            target: interface,
                            bytes: &buf[..len],
                        }),
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        now,
                        &mut journaled_sink!(),
                    );
                }
                CommandEffect::UNCHANGED
            }
            HostCommand::RegisterStreamReader {
                link_id,
                stream_id,
                sink,
                ready,
            } => {
                journal.register_stream_reader(link_id, stream_id, sink);
                let _ = ready.send(());
                CommandEffect::UNCHANGED
            }
            HostCommand::RegisterResourceSink {
                link_id,
                sink,
                ready,
            } => {
                journal.register_resource_sink(link_id, sink);
                let _ = ready.send(());
                CommandEffect::UNCHANGED
            }
            HostCommand::SetResourceStrategy {
                destination,
                strategy,
                ready,
            } => {
                let applied = engine.set_default_resource_strategy(&destination, strategy);
                let _ = ready.send(applied);
                CommandEffect::UNCHANGED
            }
            HostCommand::RegisterRequestHandler {
                destination,
                path_hash,
                policy,
                ready,
            } => {
                let result = engine.register_request_handler_hash(&destination, path_hash, policy);
                let _ = ready.send(result);
                CommandEffect::UNCHANGED
            }
            HostCommand::UnregisterRequestHandler {
                destination,
                path_hash,
                ready,
            } => {
                let removed = engine.unregister_request_handler_hash(&destination, &path_hash);
                let _ = ready.send(removed);
                CommandEffect::UNCHANGED
            }
            HostCommand::NotePersistenceFlush {
                cause,
                target,
                observed,
            } => {
                journal.route(Journaled::PersistenceFlushed { cause, target });
                if let Some(observed) = observed {
                    let _ = observed.send(());
                }
                CommandEffect::UNCHANGED
            }
            HostCommand::NotePersistenceFlushFailure {
                cause,
                target,
                observed,
            } => {
                journal.route(Journaled::PersistenceFlushFailed { cause, target });
                let _ = observed.send(());
                CommandEffect::UNCHANGED
            }
            HostCommand::SnapshotPersistedState { reply } => {
                if let Some(snapshot) = persistence_snapshots::snapshot_persisted_state(engine, now)
                {
                    let _ = reply.send(snapshot);
                }
                CommandEffect::UNCHANGED
            }
            HostCommand::SnapshotSelfRatchets { reply } => {
                let _ = reply.send(persistence_snapshots::snapshot_self_ratchets(engine));
                CommandEffect::UNCHANGED
            }
            HostCommand::SnapshotSelfRatchet { destination, reply } => {
                let snapshot = persistence_snapshots::snapshot_self_ratchet(engine, destination);
                let _ = reply.send(snapshot);
                CommandEffect::UNCHANGED
            }
            #[cfg(feature = "runtime-metrics")]
            HostCommand::SnapshotMetrics { reply } => {
                let _ = reply.send(RuntimeMetricsSnapshot {
                    taken_at: now,
                    engine: engine.metrics_snapshot(),
                    egress: topology.egress.metrics_snapshot(&topology.pacers, now),
                    crypto: crypto_pool.map(CryptoPool::metrics_snapshot),
                    reliability: journal.reliability_metrics(),
                });
                CommandEffect::UNCHANGED
            }
        }
    }
}
