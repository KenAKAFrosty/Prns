use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::engine::{InstantMillis, Journaled, PersistenceFlushCause, PersistenceFlushTarget};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::InterfaceId;
use crate::routing::announce::AnnounceObservation;
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::request_handlers::RequestHandlerError;
use crate::runtime::{
    ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNodeHandle, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use crate::wire::DestinationHash;

use super::super::super::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use super::{
    notify_accepted_announce, persistence_restored_diagnostic, run_node_tasks,
    AcceptedAnnounceObserver, NodeRunError, PrnsNode,
};

static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
fn a_runtime_destination_registers_only_its_selected_route_types() {
    struct First;
    impl RequestEndpoint<()> for First {
        const ENDPOINT_ID: &'static str = "/first";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[]);

        async fn handle(_context: RequestContext<'_, ()>) -> Result<(), Decline> {
            Ok(())
        }
    }

    struct Second;
    impl RequestEndpoint<()> for Second {
        const ENDPOINT_ID: &'static str = "/second";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[]);

        async fn handle(_context: RequestContext<'_, ()>) -> Result<(), Decline> {
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
