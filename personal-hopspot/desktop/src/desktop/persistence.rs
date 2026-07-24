use core::time::Duration;
use std::path::Path;
use std::sync::{Arc, Mutex};

use personal_rns::identity::vault::FileVault;
use personal_rns::persistence::FileStore;
use personal_rns::runtime::request_router::RouteSet;
use personal_rns::runtime::{
    boot_timeline_origin, FlushError, FlushMark, PrepareFlushError, PrnsEvent, PrnsNode,
    PrnsNodeHandle, SelfRatchetSnapshot, SelfRatchetsSnapshot,
};
use personal_rns::storage::StorageLayout;
use personal_rns::units::InstantMillis;
use personal_rns::wire::DestinationHash;
use tokio::sync::{mpsc, oneshot};

const PERSISTENCE_DIRECTORY: &str = "prns-hopspot";
const PERSISTENCE_INTERVAL: Duration = Duration::from_secs(60);
const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) struct PersistenceStorage {
    store: FileStore,
    vault: FileVault,
    mark: FlushMark,
}

pub(super) struct RestoreSummary {
    pub(super) routes: u32,
    pub(super) destination_identities: u32,
    pub(super) tunnels: u32,
    pub(super) ratchets: u32,
    pub(super) refused: u32,
    pub(super) dropped: u32,
}

impl PersistenceStorage {
    pub(super) fn new(storage_dir: &Path) -> Self {
        let path = storage_dir.join(PERSISTENCE_DIRECTORY);
        Self {
            store: FileStore::new(&path),
            vault: FileVault::new(path),
            mark: FlushMark::default(),
        }
    }

    pub(super) fn timeline_origin(&self) -> InstantMillis {
        boot_timeline_origin(&self.store)
    }

    pub(super) fn restore<St, R, F, S>(&self, node: &mut PrnsNode<St, R, F, S>) -> RestoreSummary
    where
        R: RouteSet<St>,
        F: FnMut(PrnsEvent<'_>, &St),
        S: StorageLayout,
    {
        let routes = node.seed_routes_from_store(&self.store);
        let destination_identities = node.seed_destination_identities_from_store(&self.store);
        let tunnels = node.seed_tunnels_from_store(&self.store);
        let ratchets = node.seed_self_ratchets_from_vault(&self.vault);
        RestoreSummary {
            routes: routes.seeded_count,
            destination_identities: destination_identities.seeded_count,
            tunnels: tunnels.seeded_count,
            ratchets: ratchets.seeded_count,
            refused: routes
                .refused_count
                .saturating_add(destination_identities.refused_count)
                .saturating_add(tunnels.refused_count)
                .saturating_add(ratchets.refused_count),
            dropped: routes
                .dropped_count
                .saturating_add(destination_identities.dropped_count)
                .saturating_add(tunnels.dropped_count)
                .saturating_add(ratchets.dropped_count),
        }
    }
}

pub(super) struct ShutdownFlush {
    shutdown: oneshot::Sender<()>,
    flushed: std::sync::mpsc::Receiver<()>,
}

impl ShutdownFlush {
    pub(super) fn flush_before_exit(self) {
        if self.shutdown.send(()).is_err() {
            return;
        }
        if self.flushed.recv_timeout(SHUTDOWN_FLUSH_TIMEOUT).is_err() {
            tracing::warn!(event = "persistence_shutdown_flush_timeout");
        }
    }
}

pub(super) fn spawn_worker(
    handle: PrnsNodeHandle,
    storage: PersistenceStorage,
    rotated: mpsc::UnboundedReceiver<DestinationHash>,
) -> ShutdownFlush {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (flushed_tx, flushed_rx) = std::sync::mpsc::channel();
    let worker = PersistenceWorker {
        handle,
        storage: Arc::new(Mutex::new(storage)),
        rotated,
    };
    tokio::spawn(async move {
        worker.run(shutdown_rx).await;
        let _ = flushed_tx.send(());
    });
    ShutdownFlush {
        shutdown: shutdown_tx,
        flushed: flushed_rx,
    }
}

struct PersistenceWorker {
    handle: PrnsNodeHandle,
    storage: Arc<Mutex<PersistenceStorage>>,
    rotated: mpsc::UnboundedReceiver<DestinationHash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlushOutcome {
    Landed,
    NodeStopped,
    Failed,
}

impl PersistenceWorker {
    async fn run(mut self, mut shutdown: oneshot::Receiver<()>) {
        if self.flush_state().await == FlushOutcome::NodeStopped {
            return;
        }
        if self.flush_ratchets().await == FlushOutcome::NodeStopped {
            return;
        }
        let mut ticker = tokio::time::interval(PERSISTENCE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        let mut rotations_open = true;
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    self.flush_state().await;
                    self.flush_ratchets().await;
                    return;
                }
                destination = self.rotated.recv(), if rotations_open => {
                    match destination {
                        Some(destination) => {
                            if self.flush_ratchet(destination).await == FlushOutcome::NodeStopped {
                                return;
                            }
                        }
                        None => rotations_open = false,
                    }
                }
                _ = ticker.tick() => {
                    if self.flush_state().await == FlushOutcome::NodeStopped {
                        return;
                    }
                }
            }
        }
    }

    async fn flush_state(&self) -> FlushOutcome {
        let prepared = match self.handle.prepare_flush().await {
            Ok(prepared) => prepared,
            Err(PrepareFlushError::NodeStopped) => return FlushOutcome::NodeStopped,
        };
        let storage = Arc::clone(&self.storage);
        let committed = tokio::task::spawn_blocking(move || {
            let mut storage = storage
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let PersistenceStorage { store, mark, .. } = &mut *storage;
            prepared.commit_to_store(store, mark)
        })
        .await;
        match committed {
            Ok(Ok(_)) => FlushOutcome::Landed,
            Ok(Err(FlushError::Store(error))) => {
                tracing::warn!(event = "persistence_flush_failed", %error);
                FlushOutcome::Failed
            }
            Ok(Err(FlushError::NodeStopped)) => FlushOutcome::NodeStopped,
            Err(error) => {
                tracing::warn!(event = "persistence_flush_failed", %error);
                FlushOutcome::Failed
            }
        }
    }

    async fn flush_ratchet(&self, destination: DestinationHash) -> FlushOutcome {
        let snapshot = match self.handle.snapshot_self_ratchet(destination).await {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => return FlushOutcome::Landed,
            Err(PrepareFlushError::NodeStopped) => return FlushOutcome::NodeStopped,
        };
        self.store_ratchet(snapshot).await
    }

    async fn flush_ratchets(&self) -> FlushOutcome {
        let Some(snapshot) = self.handle.snapshot_self_ratchets().await else {
            return FlushOutcome::NodeStopped;
        };
        self.store_ratchets(snapshot).await
    }

    async fn store_ratchet(&self, snapshot: SelfRatchetSnapshot) -> FlushOutcome {
        let storage = Arc::clone(&self.storage);
        let committed = tokio::task::spawn_blocking(move || {
            let mut storage = storage
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            snapshot.store_into(&mut storage.vault)
        })
        .await;
        match committed {
            Ok(Ok(())) => FlushOutcome::Landed,
            Ok(Err(error)) => {
                tracing::warn!(event = "persistence_ratchet_flush_failed", %error);
                FlushOutcome::Failed
            }
            Err(error) => {
                tracing::warn!(event = "persistence_ratchet_flush_failed", %error);
                FlushOutcome::Failed
            }
        }
    }

    async fn store_ratchets(&self, snapshot: SelfRatchetsSnapshot) -> FlushOutcome {
        let storage = Arc::clone(&self.storage);
        let committed = tokio::task::spawn_blocking(move || {
            let mut storage = storage
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            snapshot.store_into(&mut storage.vault)
        })
        .await;
        match committed {
            Ok(Ok(_)) => FlushOutcome::Landed,
            Ok(Err(error)) => {
                tracing::warn!(event = "persistence_ratchet_flush_failed", %error);
                FlushOutcome::Failed
            }
            Err(error) => {
                tracing::warn!(event = "persistence_ratchet_flush_failed", %error);
                FlushOutcome::Failed
            }
        }
    }
}
