use crate::engine::{
    CryptoOwed, EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled, OwedWork,
    PrnsCommand, Respond, RespondData, SendRequest, SendRequestData, WakeSchedules,
};
use crate::interfaces::InterfaceIfac;
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

use super::crypto_pool::CryptoPool;
use super::egress::{
    clear_announce_queues, route_reaction, route_reaction_with_work, Egress, InterfacePacer,
    WireScratch,
};
use super::host_protocol::{
    HostCommand, HostResourceDigestPreparation, HostResourcePayload, RequestAnyHostCommand,
};
use super::interface_topology::InterfaceTopology;
use super::journal_delivery::JournalDispatch;
use super::owed_work::PendingOwedWork;

fn route_command_reaction_with_owed_work<J>(
    reaction: EngineReaction<'_, OwedWork<'_>>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    wire_scratch: &mut WireScratch,
    journal: &mut JournalDispatch<J>,
    owed_work: &mut PendingOwedWork,
    crypto_pool: Option<&CryptoPool>,
    now: InstantMillis,
) where
    J: for<'a> FnMut(Journaled<'a>),
{
    route_reaction_with_work(
        reaction,
        egress,
        ifacs,
        pacers,
        wire_scratch,
        now,
        &mut |journaled| journal.route(journaled),
        &mut |work| owed_work.push(work, crypto_pool),
    );
}

fn route_command_reaction<J>(
    reaction: EngineReaction<'_>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    wire_scratch: &mut WireScratch,
    journal: &mut JournalDispatch<J>,
    now: InstantMillis,
) where
    J: for<'a> FnMut(Journaled<'a>),
{
    route_reaction(
        reaction,
        egress,
        ifacs,
        pacers,
        wire_scratch,
        now,
        &mut |journaled| journal.route(journaled),
    );
}

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
    pub(super) owed_work: &'a mut PendingOwedWork,
    #[cfg(feature = "runtime-metrics")]
    pub(super) manifold_metrics: &'a super::scheduling_metrics::ManifoldMetrics,
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
            owed_work,
            #[cfg(feature = "runtime-metrics")]
            manifold_metrics,
        } = self;
        macro_rules! defer_resource {
            ($send:expr, $segment:expr, $digest:expr) => {{
                let correlation = $send.request_id.map_or(
                    ResourceCorrelation::Unsolicited,
                    ResourceCorrelation::Response,
                );
                let mut plan = None;
                engine.request_resource_build(
                    &ResourceSend {
                        id: $send.id,
                        link_id: $send.link_id,
                        body: ResourceBody {
                            data: $send.data.as_slice(),
                            compressed_candidate: $send
                                .compressed_candidate
                                .as_ref()
                                .map(HostResourcePayload::as_slice),
                            metadata: $send.metadata.as_engine(),
                        },
                        correlation,
                    },
                    $segment,
                    &mut |reaction| {
                        route_reaction_with_work(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            now,
                            &mut |journaled| journal.route(journaled),
                            &mut |work| match work {
                                OwedWork::ResourceBuild(owed) => plan = Some(owed.into_plan()),
                                OwedWork::ResourceSeal(owed) => {
                                    owed_work.push(OwedWork::ResourceSeal(owed), crypto_pool);
                                }
                                OwedWork::Crypto(owed) => owed_work.push_crypto(owed),
                                OwedWork::ResourceOpen(owed) => {
                                    owed_work.push_resource_open(owed, crypto_pool);
                                }
                                OwedWork::WholeResourceOpen(owed) => {
                                    owed_work.push(OwedWork::WholeResourceOpen(owed), crypto_pool);
                                }
                                OwedWork::ResourceDecompression(owed) => {
                                    owed_work
                                        .push(OwedWork::ResourceDecompression(owed), crypto_pool);
                                }
                            },
                        )
                    },
                );
                if let Some(plan) = plan {
                    owed_work.push_resource_build(
                        plan,
                        $send.data,
                        $send.compressed_candidate,
                        $send.metadata,
                        $digest,
                    );
                }
                CommandEffect::UNCHANGED
            }};
        }

        match command {
            HostCommand::Engine(issued) => {
                CommandEffect::Delta(engine.ingest_command_into_with_work(
                    issued,
                    topology.interfaces.view(),
                    now,
                    &mut |entropy| host.fill_random(entropy),
                    &mut |reaction| {
                        route_command_reaction_with_owed_work(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            owed_work,
                            crypto_pool,
                            now,
                        )
                    },
                ))
            }
            HostCommand::AwaitedEngine { issued, completion } => {
                journal.register_completion(issued.id, completion);
                CommandEffect::Delta(engine.ingest_command_into_with_work(
                    issued,
                    topology.interfaces.view(),
                    now,
                    &mut |entropy| host.fill_random(entropy),
                    &mut |reaction| {
                        route_command_reaction_with_owed_work(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            owed_work,
                            crypto_pool,
                            now,
                        )
                    },
                ))
            }
            HostCommand::EngineWithTiming { issued, timing } => {
                CommandEffect::Delta(engine.ingest_command_into_with_timing_and_work(
                    issued,
                    topology.interfaces.view(),
                    now,
                    timing,
                    &mut |entropy| host.fill_random(entropy),
                    &mut |reaction| {
                        route_command_reaction_with_owed_work(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            owed_work,
                            crypto_pool,
                            now,
                        )
                    },
                ))
            }
            HostCommand::AwaitedEngineWithTiming {
                issued,
                timing,
                completion,
            } => {
                journal.register_completion(issued.id, completion);
                CommandEffect::Delta(engine.ingest_command_into_with_timing_and_work(
                    issued,
                    topology.interfaces.view(),
                    now,
                    timing,
                    &mut |entropy| host.fill_random(entropy),
                    &mut |reaction| {
                        route_command_reaction_with_owed_work(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            owed_work,
                            crypto_pool,
                            now,
                        )
                    },
                ))
            }
            HostCommand::SendResource(send) => match crypto_pool {
                Some(_) => {
                    let segment = ResourceSegment::whole(send.data.len() as u64);
                    defer_resource!(send, segment, HostResourceDigestPreparation::Calculate)
                }
                None => CommandEffect::Delta(
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
                        &mut |entropy| host.fill_random(entropy),
                        &mut |reaction| {
                            route_command_reaction(
                                reaction,
                                &mut topology.egress,
                                &topology.ifacs,
                                &mut topology.pacers,
                                wire_scratch,
                                journal,
                                now,
                            )
                        },
                    ),
                ),
            },
            HostCommand::SendResourceSegment(send) => {
                journal.register_completion(send.id, send.completion);
                if crypto_pool.is_some() {
                    let segment = ResourceSegment {
                        index: send.segment_index,
                        total_segments: send.total_segments,
                        total_data_bytes: send.total_data_bytes,
                    };
                    let digest = send.digest;
                    defer_resource!(send, segment, digest)
                } else {
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
                            &mut |entropy| host.fill_random(entropy),
                            &mut |reaction| {
                                route_command_reaction(
                                    reaction,
                                    &mut topology.egress,
                                    &topology.ifacs,
                                    &mut topology.pacers,
                                    wire_scratch,
                                    journal,
                                    now,
                                )
                            },
                        ),
                    )
                }
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
                        &mut |entropy| host.fill_random(entropy),
                        &mut |reaction| {
                            route_command_reaction(
                                reaction,
                                &mut topology.egress,
                                &topology.ifacs,
                                &mut topology.pacers,
                                wire_scratch,
                                journal,
                                now,
                            )
                        },
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
                        &mut |entropy| host.fill_random(entropy),
                        &mut |reaction| {
                            route_command_reaction(
                                reaction,
                                &mut topology.egress,
                                &topology.ifacs,
                                &mut topology.pacers,
                                wire_scratch,
                                journal,
                                now,
                            )
                        },
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
                            &mut |entropy| host.fill_random(entropy),
                            &mut |reaction| {
                                route_command_reaction(
                                    reaction,
                                    &mut topology.egress,
                                    &topology.ifacs,
                                    &mut topology.pacers,
                                    wire_scratch,
                                    journal,
                                    now,
                                )
                            },
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
                                &mut |entropy| host.fill_random(entropy),
                                &mut |reaction| {
                                    route_command_reaction(
                                        reaction,
                                        &mut topology.egress,
                                        &topology.ifacs,
                                        &mut topology.pacers,
                                        wire_scratch,
                                        journal,
                                        now,
                                    )
                                },
                            )
                        }
                        Err(_) => journal.fail_request(id),
                    }
                };
                CommandEffect::Delta(delta)
            }
            HostCommand::ProvideDecompressed(provide) => {
                CommandEffect::Delta(engine.resume_resource_decompression(
                    crate::engine::ResourceDecompressionCompleted {
                        link_id: provide.link_id,
                        hash: provide.hash,
                        plaintext: provide.plaintext.as_slice(),
                    },
                    now,
                    &mut |reaction| {
                        route_command_reaction(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            now,
                        )
                    },
                ))
            }
            HostCommand::AddInterface(add) => match topology.attach(engine, *add, now) {
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
                host.fill_random(&mut random_hash);
                if let Ok(owed) = engine.prepare_tunnel_synthesize_sign(interface, random_hash) {
                    owed_work.push(
                        OwedWork::Crypto(CryptoOwed::TunnelSynthesizeSign(owed)),
                        crypto_pool,
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
                    manifold: manifold_metrics.snapshot(),
                    reliability: journal.reliability_metrics(),
                });
                CommandEffect::UNCHANGED
            }
        }
    }
}
