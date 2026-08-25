use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::engine::test_support::{
    fixed_secret_key, personal_node_destination, sealed_single_packet,
};
use crate::engine::{InstantMillis, Journaled, PersistenceFlushCause, PersistenceFlushTarget};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceMode, ReportsStatus,
    TransportCapability,
};
use crate::manifold::interface_seam::{Interface, InterfaceSeam};
use crate::routing::announce::AnnounceObservation;
use crate::routing::links::resources::{ResourceMemoryLimits, ResourceStrategy};
use crate::routing::request_handlers::RequestHandlerError;
use crate::runtime::{
    ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNodeHandle, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use crate::wire::{DestinationHash, PacketType, WirePacketHeader};

use super::super::super::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use super::{
    notify_accepted_announce, persistence_restored_diagnostic, run_node_tasks,
    AcceptedAnnounceObserver, NodeRunError, PrnsNode,
};

static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct ProofLoopback {
    id: InterfaceId,
    inbound: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    outbound: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

impl Interface for ProofLoopback {
    const HW_MTU: usize = crate::wire::BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Loopback;

    fn channel_tag(&self) -> &[u8] {
        self.id.as_bytes()
    }

    fn descriptor(&self) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id: self.id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            gravity: crate::interfaces::InterfaceGravity::ZERO,
            bitrate: BitrateBps::guess(1_000_000),
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
            common: crate::interfaces::InterfaceCommonPolicy::RNS_DEFAULT,
        }
    }

    async fn run<S: InterfaceSeam>(mut self, mut seam: S) {
        loop {
            tokio::select! {
                inbound = self.inbound.recv() => match inbound {
                    Some(bytes) => seam.next_inbound(&bytes).await,
                    None => return,
                },
                outbound = seam.next_outbound() => {
                    let _ = self.outbound.send(outbound.to_vec());
                }
            }
        }
    }
}

impl ReportsStatus for ProofLoopback {}

fn persistence_test_directory(label: &str) -> PathBuf {
    let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "prns-recipe-persistence-{label}-{}-{sequence}",
        std::process::id()
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordedPersistenceEvent {
    Restored {
        routes: u32,
        destination_identities: u32,
        tunnels: u32,
        ratchets: u32,
        refused: u32,
        dropped: u32,
    },
    Flushed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },
    FlushFailed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },
}

fn record_persistence_event(
    events: &Arc<Mutex<Vec<RecordedPersistenceEvent>>>,
    event: crate::runtime::PrnsEvent<'_>,
) {
    let recorded = match event {
        crate::runtime::PrnsEvent::Diagnostic(
            crate::runtime::Diagnostic::PersistenceRestored {
                routes,
                destination_identities,
                tunnels,
                ratchets,
                refused,
                dropped,
            },
        ) => RecordedPersistenceEvent::Restored {
            routes,
            destination_identities,
            tunnels,
            ratchets,
            refused,
            dropped,
        },
        crate::runtime::PrnsEvent::Diagnostic(crate::runtime::Diagnostic::PersistenceFlushed {
            cause,
            target,
        }) => RecordedPersistenceEvent::Flushed { cause, target },
        crate::runtime::PrnsEvent::Diagnostic(
            crate::runtime::Diagnostic::PersistenceFlushFailed { cause, target },
        ) => RecordedPersistenceEvent::FlushFailed { cause, target },
        _ => return,
    };
    events.lock().unwrap().push(recorded);
}

#[tokio::test]
async fn node_task_panics_report_their_boundary() {
    assert_eq!(
        run_node_tasks(
            async { std::panic::panic_any("manifold") },
            std::future::pending(),
            std::future::pending(),
        )
        .await,
        Err(NodeRunError::ManifoldPanicked)
    );
    assert_eq!(
        run_node_tasks(
            std::future::pending(),
            async { std::panic::panic_any("router") },
            std::future::pending(),
        )
        .await,
        Err(NodeRunError::RequestEndpointrPanicked)
    );
    assert_eq!(
        run_node_tasks(std::future::pending(), std::future::pending(), async {
            std::panic::panic_any("driver")
        },)
        .await,
        Err(NodeRunError::InterfaceDriverPanicked)
    );
}

#[test]
fn restore_diagnostics_report_seeded_refused_and_dropped_totals() {
    let report = crate::runtime::PersistenceRestoreReport {
        routes: crate::runtime::RouteSeedReport {
            seeded_count: 1,
            refused_count: 2,
            dropped_count: 3,
        },
        destination_identities: crate::runtime::DestinationIdentitySeedReport {
            seeded_count: 4,
            refused_count: 5,
            dropped_count: 6,
        },
        tunnels: crate::runtime::TunnelSeedReport {
            seeded_count: 7,
            refused_count: 8,
            dropped_count: 9,
        },
        ratchets: crate::runtime::RatchetSeedReport {
            seeded_count: 10,
            refused_count: 11,
            dropped_count: 12,
        },
    };

    let crate::runtime::Diagnostic::PersistenceRestored {
        routes,
        destination_identities,
        tunnels,
        ratchets,
        refused,
        dropped,
    } = persistence_restored_diagnostic(&report)
    else {
        unreachable!();
    };

    assert_eq!(
        (
            routes,
            destination_identities,
            tunnels,
            ratchets,
            refused,
            dropped,
        ),
        (1, 4, 7, 10, 26, 30)
    );
}

#[tokio::test]
async fn run_until_returns_when_a_non_persistent_node_is_asked_to_stop() {
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    });

    assert_eq!(node.run_until(async {}).await, Ok(()));
}

#[tokio::test]
async fn run_until_with_proof_decider_reaches_a_prove_if_recipe_destination() {
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
    let raw = sealed_single_packet(
        &identity,
        personal_node_destination(),
        b"facade proof decision",
    );
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            app_name: "personal",
            aspects: &["node"],
            identity: fixed_secret_key(),
            announce_app_data: &[],
            proof: crate::routing::ProofStrategy::ProveIf,
            link_requests: crate::routing::LinkRequestPolicy::AcceptAll,
            ratchet: crate::engine::RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        }],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    });
    let (wire_in, inbound) = tokio::sync::mpsc::unbounded_channel();
    let (outbound, mut wire_out) = tokio::sync::mpsc::unbounded_channel();
    let _attached = node.handle().add_interface(ProofLoopback {
        id: InterfaceId::from_channel_tag(InterfaceKind::Loopback, b"prove-if-facade"),
        inbound,
        outbound,
    });
    wire_in.send(raw).unwrap();

    let decisions = Arc::new(AtomicUsize::new(0));
    let decision_count = Arc::clone(&decisions);
    let seen_plaintext = Arc::new(Mutex::new(Vec::new()));
    let plaintext_sink = Arc::clone(&seen_plaintext);
    let proof = Arc::new(Mutex::new(None));
    let proof_sink = Arc::clone(&proof);
    let shutdown = async move {
        if let Ok(Some(bytes)) =
            tokio::time::timeout(std::time::Duration::from_secs(1), wire_out.recv()).await
        {
            *proof_sink.lock().unwrap() = Some(bytes);
        }
    };

    assert_eq!(
        node.run_until_with_proof_decider(shutdown, move |request| {
            decision_count.fetch_add(1, Ordering::SeqCst);
            plaintext_sink
                .lock()
                .unwrap()
                .extend_from_slice(request.plaintext);
            true
        })
        .await,
        Ok(()),
    );
    assert_eq!(decisions.load(Ordering::SeqCst), 1);
    assert_eq!(*seen_plaintext.lock().unwrap(), b"facade proof decision");
    let proof = proof.lock().unwrap();
    let proof = proof.as_ref().expect("the accepted decision emits a proof");
    assert_eq!(
        WirePacketHeader::parse(proof).unwrap().0.packet_type,
        PacketType::Proof
    );
}

#[tokio::test]
async fn graceful_shutdown_is_observed_after_state_and_ratchet_flushes() {
    let directory = persistence_test_directory("shutdown");
    let persistence = crate::runtime::NodePersistence::custom_dir(&directory).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let event_sink = Arc::clone(&events);
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence,
        on_event: move |event, _state: &()| record_persistence_event(&event_sink, event),
    });

    let result = node.run_until(async {}).await;
    let observed = events.lock().unwrap().clone();
    let _ = std::fs::remove_dir_all(directory);

    assert_eq!(result, Ok(()));
    assert_eq!(
        observed,
        [
            RecordedPersistenceEvent::Restored {
                routes: 0,
                destination_identities: 0,
                tunnels: 0,
                ratchets: 0,
                refused: 0,
                dropped: 0,
            },
            RecordedPersistenceEvent::Flushed {
                cause: PersistenceFlushCause::Shutdown,
                target: PersistenceFlushTarget::RoutingState,
            },
            RecordedPersistenceEvent::Flushed {
                cause: PersistenceFlushCause::Shutdown,
                target: PersistenceFlushTarget::Ratchets,
            },
        ]
    );
}

#[tokio::test]
async fn a_recipe_managed_write_failure_is_observed_before_run_returns() {
    let directory = persistence_test_directory("failure");
    let persistence = crate::runtime::NodePersistence::custom_dir(&directory).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let event_sink = Arc::clone(&events);
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence,
        on_event: move |event, _state: &()| record_persistence_event(&event_sink, event),
    });
    std::fs::remove_dir_all(&directory).unwrap();
    std::fs::write(&directory, b"persistence path blocked by a file").unwrap();

    let result = node.run_until(async {}).await;
    let observed = events.lock().unwrap();
    let _ = std::fs::remove_file(directory);

    assert_eq!(result, Err(NodeRunError::PersistenceFailed));
    assert!(observed.contains(&RecordedPersistenceEvent::FlushFailed {
        cause: PersistenceFlushCause::Shutdown,
        target: PersistenceFlushTarget::RoutingState,
    }));
}

#[tokio::test]
async fn a_restore_callback_panic_reports_the_manifold_boundary() {
    let directory = persistence_test_directory("restore-panic");
    let persistence = crate::runtime::NodePersistence::custom_dir(&directory).unwrap();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence,
        on_event: |event, _state: &()| {
            if matches!(
                event,
                crate::runtime::PrnsEvent::Diagnostic(
                    crate::runtime::Diagnostic::PersistenceRestored { .. }
                )
            ) {
                panic!("restore callback");
            }
        },
    });

    let result = node.run().await;
    let _ = std::fs::remove_dir_all(directory);

    assert_eq!(result, Err(NodeRunError::ManifoldPanicked));
}

#[test]
fn accepted_announce_observers_receive_the_complete_observation() {
    let captured = Arc::new(Mutex::new(None));
    let sink = captured.clone();
    let mut observer: Option<AcceptedAnnounceObserver> =
        Some(Box::new(move |observation: AnnounceObservation<'_>| {
            *sink.lock().unwrap() = Some((
                observation.destination,
                observation.announced_identity,
                observation.hops,
                observation.source_interface,
                observation.arrived_at,
                observation.app_data.to_vec(),
                observation.is_path_response,
            ));
        }));
    let app_data = [0x42, 0x43, 0x44];
    let observation = AnnounceObservation {
        destination: DestinationHash::new([0x11; 16]),
        announced_identity: crate::identity::IdentityHash::new([0x22; 16]),
        hops: crate::units::HopCount(3),
        source_interface: InterfaceId::new([0x33; 8]),
        arrived_at: InstantMillis(4_000),
        app_data: &app_data,
        is_path_response: false,
    };

    notify_accepted_announce(
        &mut observer,
        &Journaled::AnnounceHeard {
            observation,
            rate_accounting: crate::routing::announce::AnnounceRateAccounting::NotApplied,
        },
    );

    assert_eq!(
        *captured.lock().unwrap(),
        Some((
            observation.destination,
            observation.announced_identity,
            observation.hops,
            observation.source_interface,
            observation.arrived_at,
            app_data.to_vec(),
            observation.is_path_response,
        ))
    );
}

#[test]
fn new_with_handle_builds_state_from_the_nodes_handle() {
    let prns = PrnsNode::new_with_handle(|handle| PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: handle,
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &PrnsNodeHandle| {},
    });

    assert!(Arc::ptr_eq(&prns.handle.ids, &prns.node.state.ids));
}

#[test]
fn host_resource_memory_limits_reach_the_engine_before_run() {
    let limits = ResourceMemoryLimits {
        incoming_bytes: 1_024,
        outgoing_bytes: 2_048,
    };
    let prns = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    })
    .with_resource_memory_limits(limits);

    assert_eq!(prns.node.engine.resource_memory_limits(), limits);
}

#[test]
fn a_runtime_destination_registers_only_its_selected_route_types() {
    struct First;
    impl RequestEndpoint<()> for First {
        const ENDPOINT_ID: &'static str = "/first";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[]);

        async fn handle(
            _context: RequestContext<'_, ()>,
            _node: &impl crate::runtime::PrnsNodeApi,
        ) -> Result<(), Decline> {
            Ok(())
        }
    }

    struct Second;
    impl RequestEndpoint<()> for Second {
        const ENDPOINT_ID: &'static str = "/second";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[]);

        async fn handle(
            _context: RequestContext<'_, ()>,
            _node: &impl crate::runtime::PrnsNodeApi,
        ) -> Result<(), Decline> {
            Ok(())
        }
    }

    let mut prns = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![First, Second],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    });
    let destination = prns
        .register_preconfigured_destination(PreConfiguredDestination::Single {
            app_name: "typed",
            aspects: &["routes"],
            identity: Zeroizing::new([0x42; IDENTITY_SECRET_KEY_LEN]),
            announce_app_data: &[],
            proof: crate::routing::ProofStrategy::ProveNone,
            link_requests: crate::routing::LinkRequestPolicy::AcceptAll,
            ratchet: crate::engine::RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        })
        .unwrap();
    prns.register_request_route::<First>(&destination).unwrap();
    let identity = crate::identity::IdentityHash::new([0x31; 16]);

    assert_eq!(
        prns.allow_requester(&destination, First::ENDPOINT_ID, identity),
        Ok(())
    );
    assert_eq!(
        prns.allow_requester(&destination, Second::ENDPOINT_ID, identity),
        Err(RequestHandlerError::NoSuchHandler)
    );
}
