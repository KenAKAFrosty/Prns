use core::time::Duration;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::identity::vault::FileVault;
use crate::persistence::FileStore;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use crate::engine::InstantMillis;
use crate::manifold::driver::SelfRatchetSnapshot;

use prns_runtime::runtime::{Diagnostic, NoPersistence};

use crate::engine::PersistenceFlushCause;

use super::{
    boot_timeline_origin, DestinationIdentitySeedReport, FlushError, FlushMark, FlushReport,
    PrepareFlushError, PrnsEvent, PrnsNode, PrnsNodeHandle, RatchetSeedReport, RequestEndpointSet,
    RouteSeedProgress, RouteSeedReport, TunnelSeedReport,
};

const WRITE_PROBE: &str = ".write-probe";
const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const SAVE_ON_LEARN_DEBOUNCE: Duration = Duration::from_secs(2);

pub struct NodePersistence {
    store: FileStore,
    vault: FileVault,
}

#[derive(Debug)]
pub enum DefaultLocationError {
    HomeDirectoryUnavailable,
    Io(std::io::Error),
}

impl core::fmt::Display for DefaultLocationError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::HomeDirectoryUnavailable => formatter.write_str(
                "could not determine the conventional Reticulum directory; no home directory is available",
            ),
            Self::Io(error) => {
                write!(formatter, "could not open the default persistence directory: {error}")
            }
        }
    }
}

impl std::error::Error for DefaultLocationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::HomeDirectoryUnavailable => None,
            Self::Io(error) => Some(error),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PersistenceRestoreReport {
    pub routes: RouteSeedReport,
    pub destination_identities: DestinationIdentitySeedReport,
    pub tunnels: TunnelSeedReport,
    pub ratchets: RatchetSeedReport,
}

impl PersistenceRestoreReport {
    #[must_use]
    pub const fn refused_total(&self) -> u32 {
        self.routes
            .refused_count
            .saturating_add(self.destination_identities.refused_count)
            .saturating_add(self.tunnels.refused_count)
            .saturating_add(self.ratchets.refused_count)
    }

    #[must_use]
    pub const fn dropped_total(&self) -> u32 {
        self.routes
            .dropped_count
            .saturating_add(self.destination_identities.dropped_count)
            .saturating_add(self.tunnels.dropped_count)
            .saturating_add(self.ratchets.dropped_count)
    }
}

impl NodePersistence {
    /// Resolves the conventional Reticulum directory (`/etc/reticulum` holding a config, else `~/.config/reticulum` holding one, else `~/.reticulum`) and opens the snapshot store beneath it.
    pub fn at_default_location() -> Result<Self, DefaultLocationError> {
        let reticulum_dir = crate::persistence::reticulum_directory::resolve()
            .ok_or(DefaultLocationError::HomeDirectoryUnavailable)?;
        Self::in_reticulum_dir(reticulum_dir).map_err(DefaultLocationError::Io)
    }

    /// Snapshots live in a `prns` subdir of the RNS storage dir: a config dir shared with stock RNS keeps its own msgpack `storage/tunnels`, and our sealed region of the same name must never clobber it.
    pub fn in_reticulum_dir(directory: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        Self::custom_dir(directory.into().join("storage").join("prns"))
    }

    pub fn custom_dir(directory: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let directory = directory.into();
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        verify_writable(&directory)?;
        Ok(Self {
            store: FileStore::new(&directory),
            vault: FileVault::new(directory),
        })
    }

    #[must_use]
    pub fn timeline_origin(&self) -> InstantMillis {
        boot_timeline_origin(&self.store)
    }

    #[must_use]
    pub const fn store(&self) -> &FileStore {
        &self.store
    }

    #[must_use]
    pub const fn vault(&self) -> &FileVault {
        &self.vault
    }

    pub fn restore<St, R, F, S>(&self, node: &mut PrnsNode<St, R, F, S>) -> PersistenceRestoreReport
    where
        R: RequestEndpointSet<St>,
        F: FnMut(PrnsEvent<'_>, &St),
        S: StorageLayout,
    {
        self.restore_reporting(node, |_| {})
    }

    pub fn restore_reporting<St, R, F, S>(
        &self,
        node: &mut PrnsNode<St, R, F, S>,
        progress: impl FnMut(RouteSeedProgress),
    ) -> PersistenceRestoreReport
    where
        R: RequestEndpointSet<St>,
        F: FnMut(PrnsEvent<'_>, &St),
        S: StorageLayout,
    {
        PersistenceRestoreReport {
            routes: node.seed_routes_from_store_reporting(&self.store, progress),
            destination_identities: node.seed_destination_identities_from_store(&self.store),
            tunnels: node.seed_tunnels_from_store(&self.store),
            ratchets: node.seed_self_ratchets_from_vault(&self.vault),
        }
    }

    #[must_use]
    pub fn worker(self, handle: PrnsNodeHandle) -> PersistenceWorker {
        PersistenceWorker {
            handle,
            storage: Arc::new(Mutex::new(WorkerStorage {
                store: self.store,
                vault: self.vault,
                mark: FlushMark::default(),
            })),
            flush_interval: DEFAULT_FLUSH_INTERVAL,
            rotations: None,
            changes: None,
            change_debounce: Duration::ZERO,
            failure_policy: FlushFailurePolicy::Tolerate,
        }
    }
}

pub struct SaveOnLearn {
    route_changes: mpsc::UnboundedSender<()>,
    ratchet_rotations: mpsc::UnboundedSender<DestinationHash>,
}

pub struct SaveOnLearnWiring {
    route_changes: mpsc::UnboundedReceiver<()>,
    ratchet_rotations: mpsc::UnboundedReceiver<DestinationHash>,
}

impl SaveOnLearn {
    #[must_use]
    pub fn channel() -> (Self, SaveOnLearnWiring) {
        let (route_changes_sender, route_changes) = mpsc::unbounded_channel();
        let (ratchet_rotations_sender, ratchet_rotations) = mpsc::unbounded_channel();
        (
            Self {
                route_changes: route_changes_sender,
                ratchet_rotations: ratchet_rotations_sender,
            },
            SaveOnLearnWiring {
                route_changes,
                ratchet_rotations,
            },
        )
    }

    pub fn observe(&self, event: &PrnsEvent<'_>) {
        match event {
            PrnsEvent::Diagnostic(
                Diagnostic::AnnounceHeard { .. } | Diagnostic::RouteRemoved { .. },
            ) => {
                let _ignored = self.route_changes.send(());
            }
            PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated { destination }) => {
                let _ignored = self.ratchet_rotations.send(*destination);
            }
            _ => {}
        }
    }
}

/// The recipe's `persistence` field vocabulary: `NoPersistence` or a [`NodePersistence`], resolved by the host at construction.
pub trait PersistenceIntent {
    fn into_node_persistence(self) -> Option<NodePersistence>;
}

impl PersistenceIntent for NoPersistence {
    fn into_node_persistence(self) -> Option<NodePersistence> {
        None
    }
}

impl PersistenceIntent for NodePersistence {
    fn into_node_persistence(self) -> Option<NodePersistence> {
        Some(self)
    }
}

impl From<PersistenceTrigger> for PersistenceFlushCause {
    fn from(trigger: PersistenceTrigger) -> Self {
        match trigger {
            PersistenceTrigger::Startup => Self::Startup,
            PersistenceTrigger::Interval => Self::Interval,
            PersistenceTrigger::RouteChange => Self::RouteChange,
            PersistenceTrigger::RatchetRotation => Self::RatchetRotation,
            PersistenceTrigger::Shutdown => Self::Shutdown,
        }
    }
}

fn verify_writable(directory: &std::path::Path) -> Result<(), std::io::Error> {
    use std::io::Write as _;
    let probe = directory.join(WRITE_PROBE);
    let tested = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&probe)?;
        file.write_all(b"prns")?;
        file.sync_all()
    })();
    let removed = std::fs::remove_file(&probe);
    tested.and(removed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceTrigger {
    Startup,
    Interval,
    RouteChange,
    RatchetRotation,
    Shutdown,
}

impl PersistenceTrigger {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Interval => "interval",
            Self::RouteChange => "route_change",
            Self::RatchetRotation => "ratchet_rotation",
            Self::Shutdown => "shutdown",
        }
    }
}

pub enum PersistenceEvent<'a> {
    Flushed {
        trigger: PersistenceTrigger,
        report: FlushReport,
    },
    FlushFailed {
        trigger: PersistenceTrigger,
        error: &'a dyn core::fmt::Display,
    },
    RatchetsFlushed {
        trigger: PersistenceTrigger,
        stored: u32,
    },
    RatchetFlushFailed {
        trigger: PersistenceTrigger,
        error: &'a dyn core::fmt::Display,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushFailurePolicy {
    Tolerate,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceFlushStatus {
    Landed,
    NodeStopped,
    Failed,
}

impl PersistenceFlushStatus {
    const fn worst(self, other: Self) -> Self {
        match (self, other) {
            (Self::NodeStopped, _) | (_, Self::NodeStopped) => Self::NodeStopped,
            (Self::Failed, _) | (_, Self::Failed) => Self::Failed,
            (Self::Landed, Self::Landed) => Self::Landed,
        }
    }
}

struct WorkerStorage {
    store: FileStore,
    vault: FileVault,
    mark: FlushMark,
}

pub struct PersistenceWorker {
    handle: PrnsNodeHandle,
    storage: Arc<Mutex<WorkerStorage>>,
    flush_interval: Duration,
    rotations: Option<mpsc::UnboundedReceiver<DestinationHash>>,
    changes: Option<mpsc::UnboundedReceiver<()>>,
    change_debounce: Duration,
    failure_policy: FlushFailurePolicy,
}

impl PersistenceWorker {
    #[must_use]
    pub fn with_flush_interval(mut self, interval: Duration) -> Self {
        self.flush_interval = interval;
        self
    }

    #[must_use]
    pub fn with_flush_failure_policy(mut self, policy: FlushFailurePolicy) -> Self {
        self.failure_policy = policy;
        self
    }

    #[must_use]
    pub fn with_ratchet_rotations(
        mut self,
        rotations: mpsc::UnboundedReceiver<DestinationHash>,
    ) -> Self {
        self.rotations = Some(rotations);
        self
    }

    #[must_use]
    pub fn with_route_changes(
        mut self,
        changes: mpsc::UnboundedReceiver<()>,
        debounce: Duration,
    ) -> Self {
        self.changes = Some(changes);
        self.change_debounce = debounce;
        self
    }

    /// A flush rewrites each changed region whole, the same granularity the RNS reference uses for its path table; tens of thousands of routes still serialize to a few megabytes, unchanged regions are fingerprint-skipped, and the debounce coalesces bursts, so this trigger governs write frequency rather than write size.
    #[must_use]
    pub fn with_save_on_learn(self, wiring: SaveOnLearnWiring) -> Self {
        self.with_ratchet_rotations(wiring.ratchet_rotations)
            .with_route_changes(wiring.route_changes, SAVE_ON_LEARN_DEBOUNCE)
    }

    pub async fn flush_now(
        &self,
        on_event: &mut (dyn FnMut(PersistenceEvent<'_>) + Send),
    ) -> PersistenceFlushStatus {
        let state = flush_state(
            &self.handle,
            &self.storage,
            PersistenceTrigger::Startup,
            on_event,
        )
        .await;
        if let PersistenceFlushStatus::NodeStopped = state {
            return state;
        }
        let ratchets = flush_all_ratchets(
            &self.handle,
            &self.storage,
            PersistenceTrigger::Startup,
            on_event,
        )
        .await;
        state.worst(ratchets)
    }

    pub async fn run(
        self,
        shutdown: impl core::future::Future<Output = ()>,
        mut on_event: impl FnMut(PersistenceEvent<'_>) + Send,
    ) -> PersistenceFlushStatus {
        let Self {
            handle,
            storage,
            flush_interval,
            mut rotations,
            mut changes,
            change_debounce,
            failure_policy,
        } = self;
        let on_event: &mut (dyn FnMut(PersistenceEvent<'_>) + Send) = &mut on_event;
        let mut ticker = tokio::time::interval(flush_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        tokio::pin!(shutdown);
        let mut rotations_open = rotations.is_some();
        let mut changes_open = changes.is_some();
        loop {
            tokio::select! {
                biased;
                () = &mut shutdown => {
                    let state = flush_state(&handle, &storage, PersistenceTrigger::Shutdown, on_event).await;
                    if let PersistenceFlushStatus::NodeStopped = state {
                        return state;
                    }
                    let ratchets = flush_all_ratchets(&handle, &storage, PersistenceTrigger::Shutdown, on_event).await;
                    return state.worst(ratchets);
                }
                destination = recv_or_pending(rotations.as_mut()), if rotations_open => {
                    match destination {
                        Some(destination) => {
                            let status = flush_rotated_ratchet(&handle, &storage, destination, on_event).await;
                            if should_exit(status, failure_policy) {
                                return status;
                            }
                        }
                        None => rotations_open = false,
                    }
                }
                change = recv_or_pending(changes.as_mut()), if changes_open => {
                    match change {
                        Some(()) => {
                            tokio::time::sleep(change_debounce).await;
                            if let Some(changes) = changes.as_mut() {
                                while changes.try_recv().is_ok() {}
                            }
                            let status = flush_state(&handle, &storage, PersistenceTrigger::RouteChange, on_event).await;
                            if should_exit(status, failure_policy) {
                                return status;
                            }
                        }
                        None => changes_open = false,
                    }
                }
                _ = ticker.tick() => {
                    let status = flush_state(&handle, &storage, PersistenceTrigger::Interval, on_event).await;
                    if should_exit(status, failure_policy) {
                        return status;
                    }
                }
            }
        }
    }
}

const fn should_exit(status: PersistenceFlushStatus, policy: FlushFailurePolicy) -> bool {
    match status {
        PersistenceFlushStatus::NodeStopped => true,
        PersistenceFlushStatus::Failed => matches!(policy, FlushFailurePolicy::Exit),
        PersistenceFlushStatus::Landed => false,
    }
}

async fn recv_or_pending<T>(receiver: Option<&mut mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => core::future::pending().await,
    }
}

async fn flush_state(
    handle: &PrnsNodeHandle,
    storage: &Arc<Mutex<WorkerStorage>>,
    trigger: PersistenceTrigger,
    on_event: &mut (dyn FnMut(PersistenceEvent<'_>) + Send),
) -> PersistenceFlushStatus {
    let prepared = match handle.prepare_flush().await {
        Ok(prepared) => prepared,
        Err(PrepareFlushError::NodeStopped) => return PersistenceFlushStatus::NodeStopped,
    };
    let storage = Arc::clone(storage);
    let committed = tokio::task::spawn_blocking(move || {
        let mut storage = match storage.lock() {
            Ok(storage) => storage,
            Err(poisoned) => poisoned.into_inner(),
        };
        let WorkerStorage { store, mark, .. } = &mut *storage;
        prepared.commit_to_store(store, mark)
    })
    .await;
    match committed {
        Ok(Ok(report)) => {
            on_event(PersistenceEvent::Flushed { trigger, report });
            PersistenceFlushStatus::Landed
        }
        Ok(Err(FlushError::NodeStopped)) => PersistenceFlushStatus::NodeStopped,
        Ok(Err(FlushError::Store(error))) => {
            on_event(PersistenceEvent::FlushFailed {
                trigger,
                error: &error,
            });
            PersistenceFlushStatus::Failed
        }
        Err(error) => {
            on_event(PersistenceEvent::FlushFailed {
                trigger,
                error: &error,
            });
            PersistenceFlushStatus::Failed
        }
    }
}

async fn flush_rotated_ratchet(
    handle: &PrnsNodeHandle,
    storage: &Arc<Mutex<WorkerStorage>>,
    destination: DestinationHash,
    on_event: &mut (dyn FnMut(PersistenceEvent<'_>) + Send),
) -> PersistenceFlushStatus {
    let snapshot = match handle.snapshot_self_ratchet(destination).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return PersistenceFlushStatus::Landed,
        Err(PrepareFlushError::NodeStopped) => return PersistenceFlushStatus::NodeStopped,
    };
    store_single_ratchet(
        storage,
        snapshot,
        PersistenceTrigger::RatchetRotation,
        on_event,
    )
    .await
}

async fn flush_all_ratchets(
    handle: &PrnsNodeHandle,
    storage: &Arc<Mutex<WorkerStorage>>,
    trigger: PersistenceTrigger,
    on_event: &mut (dyn FnMut(PersistenceEvent<'_>) + Send),
) -> PersistenceFlushStatus {
    let Some(snapshot) = handle.snapshot_self_ratchets().await else {
        return PersistenceFlushStatus::NodeStopped;
    };
    let storage = Arc::clone(storage);
    let committed = tokio::task::spawn_blocking(move || {
        let mut storage = match storage.lock() {
            Ok(storage) => storage,
            Err(poisoned) => poisoned.into_inner(),
        };
        snapshot.store_into(&mut storage.vault)
    })
    .await;
    match committed {
        Ok(Ok(stored)) => {
            on_event(PersistenceEvent::RatchetsFlushed { trigger, stored });
            PersistenceFlushStatus::Landed
        }
        Ok(Err(error)) => {
            on_event(PersistenceEvent::RatchetFlushFailed {
                trigger,
                error: &error,
            });
            PersistenceFlushStatus::Failed
        }
        Err(error) => {
            on_event(PersistenceEvent::RatchetFlushFailed {
                trigger,
                error: &error,
            });
            PersistenceFlushStatus::Failed
        }
    }
}

async fn store_single_ratchet(
    storage: &Arc<Mutex<WorkerStorage>>,
    snapshot: SelfRatchetSnapshot,
    trigger: PersistenceTrigger,
    on_event: &mut (dyn FnMut(PersistenceEvent<'_>) + Send),
) -> PersistenceFlushStatus {
    let storage = Arc::clone(storage);
    let committed = tokio::task::spawn_blocking(move || {
        let mut storage = match storage.lock() {
            Ok(storage) => storage,
            Err(poisoned) => poisoned.into_inner(),
        };
        snapshot.store_into(&mut storage.vault)
    })
    .await;
    match committed {
        Ok(Ok(())) => {
            on_event(PersistenceEvent::RatchetsFlushed { trigger, stored: 1 });
            PersistenceFlushStatus::Landed
        }
        Ok(Err(error)) => {
            on_event(PersistenceEvent::RatchetFlushFailed {
                trigger,
                error: &error,
            });
            PersistenceFlushStatus::Failed
        }
        Err(error) => {
            on_event(PersistenceEvent::RatchetFlushFailed {
                trigger,
                error: &error,
            });
            PersistenceFlushStatus::Failed
        }
    }
}
