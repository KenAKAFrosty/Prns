use core::time::Duration;
use std::path::Path;

use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::runtime::{
    NodePersistence, PersistenceEvent, PersistenceFlushStatus, PersistenceRestoreReport, PrnsEvent,
    PrnsNode, PrnsNodeHandle, RegionFlush,
};
use personal_rns::storage::StorageLayout;
use personal_rns::units::InstantMillis;
use personal_rns::wire::DestinationHash;

const STORE_DIRECTORY: &str = "prns";
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(250);
const PERIODIC_FLUSH_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub(crate) struct PreparedPersistence {
    persistence: NodePersistence,
}

impl PreparedPersistence {
    pub(crate) fn open(storage_directory: &Path) -> Result<Self, std::io::Error> {
        Ok(Self {
            persistence: NodePersistence::custom_dir(storage_directory.join(STORE_DIRECTORY))?,
        })
    }

    pub(crate) fn timeline_origin(&self) -> InstantMillis {
        self.persistence.timeline_origin()
    }

    pub(crate) fn restore<St, R, F, S>(
        &self,
        node: &mut PrnsNode<St, R, F, S>,
    ) -> PersistenceRestoreReport
    where
        R: RequestEndpointSet<St>,
        F: FnMut(PrnsEvent<'_>, &St),
        S: StorageLayout,
    {
        self.persistence.restore(node)
    }

    pub(crate) fn start(
        self,
        handle: PrnsNodeHandle,
        changes: tokio::sync::mpsc::UnboundedReceiver<()>,
        rotated: tokio::sync::mpsc::UnboundedReceiver<DestinationHash>,
    ) -> PersistenceTask {
        PersistenceTask::start(self.persistence, handle, changes, rotated)
    }
}

pub(crate) struct PersistenceTask {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<PersistenceFlushStatus>>,
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
        persistence: NodePersistence,
        handle: PrnsNodeHandle,
        changes: tokio::sync::mpsc::UnboundedReceiver<()>,
        rotated: tokio::sync::mpsc::UnboundedReceiver<DestinationHash>,
    ) -> Self {
        let (shutdown, requested) = tokio::sync::oneshot::channel();
        let worker = persistence
            .worker(handle)
            .with_flush_interval(PERIODIC_FLUSH_INTERVAL)
            .with_route_changes(changes, CHANGE_DEBOUNCE)
            .with_ratchet_rotations(rotated);
        let join = tokio::spawn(worker.run(
            async move {
                let _ = requested.await;
            },
            observe,
        ));
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
            Ok(Ok(PersistenceFlushStatus::Landed)) => PersistenceShutdown::Flushed,
            Ok(Ok(PersistenceFlushStatus::Failed | PersistenceFlushStatus::NodeStopped))
            | Ok(Err(_)) => PersistenceShutdown::Failed,
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

fn observe(event: PersistenceEvent<'_>) {
    match event {
        PersistenceEvent::Flushed { trigger, report } => {
            crate::engine::diagnostic(
                "persistence",
                format_args!(
                    "state=flushed reason={} routing={} tunnels={} destinations={}",
                    trigger.name(),
                    region_name(report.routing_table),
                    region_name(report.tunnels),
                    region_name(report.destination_identities)
                ),
            );
        }
        PersistenceEvent::FlushFailed { trigger, error } => {
            crate::engine::diagnostic(
                "persistence",
                format_args!("state=failed reason={} error={error}", trigger.name()),
            );
        }
        PersistenceEvent::RatchetFlushFailed { trigger, error } => {
            crate::engine::diagnostic(
                "persistence",
                format_args!(
                    "state=failed reason=ratchet trigger={} error={error}",
                    trigger.name()
                ),
            );
        }
        PersistenceEvent::RatchetsFlushed { .. } => {}
    }
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
        AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy,
    };
    use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
    use personal_rns::interfaces::{BitrateBps, InterfaceId};
    use personal_rns::manifold::reconnect::ReconnectPolicy;
    use personal_rns::persistence::{
        read_routing_table_snapshot, FileStore, PersistedStore, SnapshotRegion,
    };
    use personal_rns::remote_control::{
        RemoteControlControllerIdentitySecret, RemoteControlInitialAccess,
        RemoteControlNodeIdentitySecrets, RemoteControlSelfAnnouncement, RemoteControlService,
        RemoteControlTargetIdentitySecret,
    };
    use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
    use personal_rns::runtime::{
        Diagnostic, ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNodeRecipe,
        ServeMyRequestEndpoints,
    };
    use personal_rns::storage::GrowableHeap;
    use personal_rns::tcp::{TcpClientInterface, TcpServer};
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    const TEST_BITRATE: BitrateBps = BitrateBps::guess(1_000_000);
    const WRITE_PROBE: &str = ".write-probe";

    #[test]
    fn preparation_creates_a_private_writable_store_without_leaving_the_probe() {
        let root = tempfile::tempdir().unwrap();
        let prepared = PreparedPersistence::open(root.path()).unwrap();

        assert_eq!(
            prepared.persistence.store().dir(),
            root.path().join(STORE_DIRECTORY)
        );
        assert!(prepared.persistence.store().dir().is_dir());
        assert!(!prepared
            .persistence
            .store()
            .dir()
            .join(WRITE_PROBE)
            .exists());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(prepared.persistence.store().dir())
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
        assert_eq!(restored.ratchets.seeded_count, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_accepted_announce_flushes_and_restores_a_route_that_answers_path_requests() {
        let root = tempfile::tempdir().unwrap();
        let prepared = PreparedPersistence::open(root.path()).unwrap();
        let announced = test_destination(0xA1);
        let destination = announced.destination_hash().unwrap();

        let server = TcpServer::bind_with_bitrate("127.0.0.1:0", TEST_BITRATE)
            .await
            .unwrap();
        let server_address = server.local_addr().unwrap().to_string();
        let node_a = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: test_remote_control(),
            pre_configured_destinations: [test_destination(0xA1)],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![],
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
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
        let (_rotated_tx, rotated_rx) = tokio::sync::mpsc::unbounded_channel::<DestinationHash>();
        let node_b = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: test_remote_control(),
            pre_configured_destinations: [test_destination(0xB2)],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![],
            interfaces: |handle: &PrnsNodeHandle| {
                handle.attach(client);
            },
            on_event: move |event, _state| {
                if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event
                {
                    let _ = heard_tx.send(destination);
                    let _ = change_tx.send(());
                }
            },
            persistence: NoPersistence,
        })
        .with_timeline_origin(prepared.timeline_origin());
        let handle_b = node_b.handle();
        let mut persistence = prepared.start(handle_b, change_rx, rotated_rx);

        let announce_handle = handle_a.clone();
        let announcer = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                if announce_handle
                    .issue(PrnsCommand::AnnounceNow(AnnounceNow {
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
        let restarted_server = TcpServer::bind_with_bitrate("127.0.0.1:0", TEST_BITRATE)
            .await
            .unwrap();
        let restarted_address = restarted_server.local_addr().unwrap().to_string();
        let mut restarted_node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: Some(Zeroizing::new([0xB3; IDENTITY_SECRET_KEY_LEN])),
            remote_control: test_remote_control(),
            pre_configured_destinations: [test_destination(0xB2)],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![],
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
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
            remote_control: test_remote_control(),
            pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![],
            interfaces: move |handle: &PrnsNodeHandle| {
                handle.attach(requester_client);
            },
            on_event: |_event, _state| {},
            persistence: NoPersistence,
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

    fn test_remote_control() -> RemoteControlService<'static> {
        let identity_secrets = RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(Zeroizing::new(
                [0x43; IDENTITY_SECRET_KEY_LEN],
            )),
            RemoteControlTargetIdentitySecret::from(Zeroizing::new(
                [0x54; IDENTITY_SECRET_KEY_LEN],
            )),
        )
        .expect("test controller and target identities must remain distinct");
        RemoteControlService::new(
            identity_secrets,
            RemoteControlInitialAccess::Nobody,
            RemoteControlSelfAnnouncement::Unavailable,
        )
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
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
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
            remote_control: test_remote_control(),
            pre_configured_destinations: [destination],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![],
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
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
