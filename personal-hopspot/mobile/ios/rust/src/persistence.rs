use core::time::Duration;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::persistence::FileStore;
use personal_rns::runtime::request_router::RouteSet;
use personal_rns::runtime::{
    boot_timeline_origin, DestinationIdentitySeedReport, FlushError, FlushMark, FlushReport,
    PrepareFlushError, PrnsEvent, PrnsNode, PrnsNodeHandle, RegionFlush, RouteSeedReport,
    TunnelSeedReport,
};
use personal_rns::storage::StorageLayout;
use personal_rns::units::InstantMillis;

const STORE_DIRECTORY: &str = "prns";
const WRITE_PROBE: &str = ".write-probe";
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(250);
const PERIODIC_FLUSH_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub(crate) enum RouteTableChange {
    AcceptedAnnounce,
    RemovedRoute,
}

pub(crate) struct RestoreReport {
    pub(crate) routes: RouteSeedReport,
    pub(crate) destination_identities: DestinationIdentitySeedReport,
    pub(crate) tunnels: TunnelSeedReport,
}

pub(crate) struct PreparedPersistence {
    store: FileStore,
}

impl PreparedPersistence {
    pub(crate) fn open(storage_directory: &Path) -> Result<Self, std::io::Error> {
        let store_directory = storage_directory.join(STORE_DIRECTORY);
        fs::create_dir_all(&store_directory)?;
        if !fs::metadata(&store_directory)?.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                "the persistence store path is not a directory",
            ));
        }
        #[cfg(unix)]
        fs::set_permissions(&store_directory, fs::Permissions::from_mode(0o700))?;
        verify_writable(&store_directory)?;
        Ok(Self {
            store: FileStore::new(store_directory),
        })
    }

    pub(crate) fn timeline_origin(&self) -> InstantMillis {
        boot_timeline_origin(&self.store)
    }

    pub(crate) fn restore<St, R, F, S>(&self, node: &mut PrnsNode<St, R, F, S>) -> RestoreReport
    where
        R: RouteSet<St>,
        F: FnMut(PrnsEvent<'_>, &St),
        S: StorageLayout,
    {
        RestoreReport {
            routes: node.seed_routes_from_store(&self.store),
            destination_identities: node.seed_destination_identities_from_store(&self.store),
            tunnels: node.seed_tunnels_from_store(&self.store),
        }
    }

    pub(crate) fn start(
        self,
        handle: PrnsNodeHandle,
        changes: tokio::sync::mpsc::UnboundedReceiver<RouteTableChange>,
    ) -> PersistenceTask {
        PersistenceTask::start(handle, self.store, changes)
    }
}

fn verify_writable(store_directory: &Path) -> Result<(), std::io::Error> {
    let probe = store_directory.join(WRITE_PROBE);
    let tested = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&probe)?;
        file.write_all(b"hopspot")?;
        file.sync_all()
    })();
    let removed = fs::remove_file(&probe);
    tested.and(removed)
}

struct PersistenceStorage {
    store: FileStore,
    mark: FlushMark,
}

pub(crate) struct PersistenceTask {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<FlushOutcome>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistenceShutdown {
    Flushed,
    Failed,
    TimedOut,
    AlreadyStopped,
}

impl PersistenceTask {
    fn start(
        handle: PrnsNodeHandle,
        store: FileStore,
        changes: tokio::sync::mpsc::UnboundedReceiver<RouteTableChange>,
    ) -> Self {
        let storage = Arc::new(Mutex::new(PersistenceStorage {
            store,
            mark: FlushMark::default(),
        }));
        let (shutdown, requested) = tokio::sync::oneshot::channel();
        let join = tokio::spawn(run(handle, storage, changes, requested));
        Self {
            shutdown: Some(shutdown),
            join: Some(join),
        }
    }

    pub(crate) async fn shutdown(&mut self, timeout: Duration) -> PersistenceShutdown {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let Some(mut join) = self.join.take() else {
            return PersistenceShutdown::AlreadyStopped;
        };
        match tokio::time::timeout(timeout, &mut join).await {
            Ok(Ok(FlushOutcome::Landed)) => PersistenceShutdown::Flushed,
            Ok(Ok(FlushOutcome::Failed | FlushOutcome::NodeStopped)) | Ok(Err(_)) => {
                PersistenceShutdown::Failed
            }
            Err(_) => {
                join.abort();
                let _ = join.await;
                PersistenceShutdown::TimedOut
            }
        }
    }

    pub(crate) async fn abort(&mut self) {
        self.shutdown.take();
        if let Some(join) = self.join.take() {
            join.abort();
            let _ = join.await;
        }
    }
}

async fn run(
    handle: PrnsNodeHandle,
    storage: Arc<Mutex<PersistenceStorage>>,
    mut changes: tokio::sync::mpsc::UnboundedReceiver<RouteTableChange>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> FlushOutcome {
    let mut periodic = tokio::time::interval(PERIODIC_FLUSH_INTERVAL);
    periodic.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    periodic.tick().await;
    let mut changes_open = true;
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                return flush(&handle, &storage, FlushReason::Shutdown).await;
            }
            change = changes.recv(), if changes_open => {
                match change {
                    Some(RouteTableChange::AcceptedAnnounce | RouteTableChange::RemovedRoute) => {
                        tokio::time::sleep(CHANGE_DEBOUNCE).await;
                        while changes.try_recv().is_ok() {}
                        let _ = flush(&handle, &storage, FlushReason::RouteTableChanged).await;
                    }
                    None => changes_open = false,
                }
            }
            _ = periodic.tick() => {
                let _ = flush(&handle, &storage, FlushReason::Periodic).await;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FlushReason {
    RouteTableChanged,
    Periodic,
    Shutdown,
}

impl FlushReason {
    const fn name(self) -> &'static str {
        match self {
            Self::RouteTableChanged => "route_change",
            Self::Periodic => "periodic",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlushOutcome {
    Landed,
    NodeStopped,
    Failed,
}

async fn flush(
    handle: &PrnsNodeHandle,
    storage: &Arc<Mutex<PersistenceStorage>>,
    reason: FlushReason,
) -> FlushOutcome {
    let routes = handle.routes().await.len();
    let prepared = match handle.prepare_flush().await {
        Ok(prepared) => prepared,
        Err(PrepareFlushError::NodeStopped) => return FlushOutcome::NodeStopped,
    };
    let storage = Arc::clone(storage);
    let committed = tokio::task::spawn_blocking(move || {
        let mut storage = match storage.lock() {
            Ok(storage) => storage,
            Err(poisoned) => poisoned.into_inner(),
        };
        let PersistenceStorage { store, mark } = &mut *storage;
        prepared.commit_to_store(store, mark)
    })
    .await;
    match committed {
        Ok(Ok(report)) => {
            log_flush(reason, routes, report);
            FlushOutcome::Landed
        }
        Ok(Err(FlushError::Store(error))) => {
            crate::engine::diagnostic(
                "persistence",
                format_args!(
                    "state=failed reason={} routes={routes} error={error}",
                    reason.name()
                ),
            );
            FlushOutcome::Failed
        }
        Ok(Err(FlushError::NodeStopped)) => FlushOutcome::NodeStopped,
        Err(error) => {
            crate::engine::diagnostic(
                "persistence",
                format_args!(
                    "state=failed reason={} routes={routes} worker_error={error}",
                    reason.name()
                ),
            );
            FlushOutcome::Failed
        }
    }
}

fn log_flush(reason: FlushReason, routes: usize, report: FlushReport) {
    crate::engine::diagnostic(
        "persistence",
        format_args!(
            "state=flushed reason={} routes={routes} routing={} tunnels={} destinations={}",
            reason.name(),
            region_name(report.routing_table),
            region_name(report.tunnels),
            region_name(report.destination_identities)
        ),
    );
}

const fn region_name(region: RegionFlush) -> &'static str {
    match region {
        RegionFlush::Wrote => "wrote",
        RegionFlush::UnchangedSkipped => "unchanged",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::engine::{
        AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
    };
    use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
    use personal_rns::interfaces::{BitrateBps, InterfaceId};
    use personal_rns::persistence::{read_routing_table_snapshot, PersistedStore, SnapshotRegion};
    use personal_rns::reactor::reconnect::ReconnectPolicy;
    use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
    use personal_rns::runtime::{
        Manual, PreConfiguredDestination, PrnsNodeRecipe, RequestHandlerRegistration,
    };
    use personal_rns::storage::GrowableHeap;
    use personal_rns::tcp::{TcpClientInterface, TcpServer};
    const TEST_BITRATE: BitrateBps = BitrateBps::guess(1_000_000);

    #[test]
    fn preparation_creates_a_private_writable_store_without_leaving_the_probe() {
        let root = tempfile::tempdir().unwrap();
        let prepared = PreparedPersistence::open(root.path()).unwrap();

        assert_eq!(prepared.store.dir(), root.path().join(STORE_DIRECTORY));
        assert!(prepared.store.dir().is_dir());
        assert!(!prepared.store.dir().join(WRITE_PROBE).exists());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(prepared.store.dir())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn preparation_rejects_a_store_path_that_is_not_a_directory() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(STORE_DIRECTORY), b"not a directory").unwrap();

        assert!(PreparedPersistence::open(root.path()).is_err());
    }

    #[test]
    fn a_corrupt_route_snapshot_is_refused_without_blocking_restore() {
        let root = tempfile::tempdir().unwrap();
        let prepared = PreparedPersistence::open(root.path()).unwrap();
        let mut store = FileStore::new(root.path().join(STORE_DIRECTORY));
        store
            .store(SnapshotRegion::RoutingTable, b"not a route snapshot")
            .unwrap();
        let mut node = test_node(
            test_destination(0xB2),
            |_event, _state| {},
            prepared.timeline_origin(),
        );

        let restored = prepared.restore(&mut node);

        assert_eq!(restored.routes.seeded_count, 0);
        assert_eq!(restored.routes.refused_count, 1);
        assert_eq!(restored.routes.dropped_count, 0);
        assert_eq!(restored.destination_identities.seeded_count, 0);
        assert_eq!(restored.tunnels.seeded_count, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_accepted_announce_flushes_and_restores_a_route_that_answers_path_requests() {
        let root = tempfile::tempdir().unwrap();
        let prepared = PreparedPersistence::open(root.path()).unwrap();
        let announced = test_destination(0xA1);
        let destination = announced.destination_hash().unwrap();

        let server = TcpServer::bind("127.0.0.1:0", TEST_BITRATE).await.unwrap();
        let server_address = server.local_addr().unwrap().to_string();
        let node_a = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [test_destination(0xA1)],
            app_state: (),
            storage: GrowableHeap,
            routes: personal_rns::routes![],
            interfaces: Manual,
            on_event: |_event, _state| {},
        });
        let handle_a = node_a.handle();
        let _server_supervisor = handle_a.supervise(server);

        let client = TcpClientInterface::new_with_id(
            InterfaceId::new(*b"\x00iospers"),
            server_address,
            TEST_BITRATE,
            ReconnectPolicy::STANDARD,
        );
        let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
        let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
        let node_b = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [test_destination(0xB2)],
            app_state: (),
            storage: GrowableHeap,
            routes: personal_rns::routes![],
            interfaces: |handle: &PrnsNodeHandle| {
                handle.attach(client);
            },
            on_event: move |event, _state| {
                if let PrnsEvent::Diagnostic(personal_rns::runtime::Diagnostic::AnnounceHeard {
                    destination,
                    ..
                }) = event
                {
                    let _ = heard_tx.send(destination);
                    let _ = change_tx.send(RouteTableChange::AcceptedAnnounce);
                }
            },
        })
        .with_timeline_origin(prepared.timeline_origin());
        let handle_b = node_b.handle();
        let mut persistence = prepared.start(handle_b, change_rx);

        let announce_handle = handle_a.clone();
        let announcer = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                if announce_handle
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });

        let hear_flush_and_stop = async {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                    .await
                    .unwrap(),
                Some(destination)
            );
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if stored_route_count(root.path()) == Some(1) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                persistence.shutdown(Duration::from_secs(2)).await,
                PersistenceShutdown::Flushed
            );
        };
        tokio::select! {
            biased;
            () = hear_flush_and_stop => {}
            result = node_a.run() => unreachable!("the announcing node stopped: {result:?}"),
            result = node_b.run() => unreachable!("the persisting node stopped: {result:?}"),
        }

        announcer.abort();

        let restarted = PreparedPersistence::open(root.path()).unwrap();
        let restarted_server = TcpServer::bind("127.0.0.1:0", TEST_BITRATE).await.unwrap();
        let restarted_address = restarted_server.local_addr().unwrap().to_string();
        let mut restarted_node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: Some(Zeroizing::new([0xB3; IDENTITY_SECRET_KEY_LEN])),
            pre_configured_destinations: [test_destination(0xB2)],
            app_state: (),
            storage: GrowableHeap,
            routes: personal_rns::routes![],
            interfaces: Manual,
            on_event: |_event, _state| {},
        })
        .with_timeline_origin(restarted.timeline_origin());
        let restored = restarted.restore(&mut restarted_node);

        assert_eq!(restored.routes.seeded_count, 1);
        assert_eq!(restored.routes.refused_count, 0);
        assert_eq!(restored.routes.dropped_count, 0);

        let restarted_handle = restarted_node.handle();
        let _restarted_server_supervisor = restarted_handle.supervise(restarted_server);
        let requester_client = TcpClientInterface::new_with_id(
            InterfaceId::new(*b"\x00iosreqs"),
            restarted_address,
            TEST_BITRATE,
            ReconnectPolicy::STANDARD,
        );
        let requester_node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
            app_state: (),
            storage: GrowableHeap,
            routes: personal_rns::routes![],
            interfaces: move |handle: &PrnsNodeHandle| {
                handle.attach(requester_client);
            },
            on_event: |_event, _state| {},
        });
        let requester_handle = requester_node.handle();
        let request = async {
            tokio::time::sleep(Duration::from_millis(250)).await;
            tokio::time::timeout(
                Duration::from_secs(5),
                requester_handle.request_path(destination),
            )
            .await
            .expect("the restored route request timed out")
            .expect("the restored route request failed")
        };
        let found = tokio::select! {
            found = request => found,
            result = restarted_node.run() => {
                unreachable!("the restored transport stopped: {result:?}")
            }
            result = requester_node.run() => {
                unreachable!("the route requester stopped: {result:?}")
            }
        };
        assert_eq!(found.hops.0, 2);
    }

    fn test_destination(byte: u8) -> PreConfiguredDestination<'static> {
        PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "ios-persistence",
            aspects: &["route"],
            identity: Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN]),
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            request_handlers: RequestHandlerRegistration::None,
        }
    }

    fn test_node<F>(
        destination: PreConfiguredDestination<'static>,
        on_event: F,
        timeline_origin: InstantMillis,
    ) -> PrnsNode<(), (), F, GrowableHeap>
    where
        F: FnMut(PrnsEvent<'_>, &()) + Send + 'static,
    {
        PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [destination],
            app_state: (),
            storage: GrowableHeap,
            routes: personal_rns::routes![],
            interfaces: Manual,
            on_event,
        })
        .with_timeline_origin(timeline_origin)
    }

    fn stored_route_count(storage_directory: &Path) -> Option<usize> {
        let store = FileStore::new(storage_directory.join(STORE_DIRECTORY));
        let len = store.stored_len(SnapshotRegion::RoutingTable).ok()??;
        let mut bytes = vec![0; len];
        let snapshot = store
            .load(SnapshotRegion::RoutingTable, &mut bytes)
            .ok()??;
        read_routing_table_snapshot(snapshot)
            .ok()
            .map(Iterator::count)
    }
}
