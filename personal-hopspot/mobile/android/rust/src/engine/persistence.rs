use core::time::Duration;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use personal_rns::identity::vault::FileVault;
use personal_rns::persistence::FileStore;
use personal_rns::runtime::{
    boot_timeline_origin, FlushError, FlushMark, PrepareFlushError, PrnsNodeHandle,
    SelfRatchetSnapshot, SelfRatchetsSnapshot,
};
use personal_rns::units::InstantMillis;
use personal_rns::wire::DestinationHash;
use tokio::sync::{mpsc, oneshot};

const PERSISTENCE_DIRECTORY: &str = "prns";
const PERSISTENCE_INTERVAL: Duration = Duration::from_secs(30);

pub(super) struct PersistenceStorage {
    store: FileStore,
    vault: FileVault,
    mark: FlushMark,
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

    pub(super) fn store(&self) -> &FileStore {
        &self.store
    }

    pub(super) fn vault(&self) -> &FileVault {
        &self.vault
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PersistenceRestore {
    pub(crate) routes: u32,
    pub(crate) destination_identities: u32,
    pub(crate) tunnels: u32,
    pub(crate) ratchets: u32,
    pub(crate) refused: u32,
    pub(crate) dropped: u32,
}

struct PersistenceHealthInner {
    restore: PersistenceRestore,
    successful_flushes: AtomicU64,
}

#[derive(Clone)]
pub(crate) struct PersistenceHealth {
    inner: Arc<PersistenceHealthInner>,
}

impl PersistenceHealth {
    fn new(restore: PersistenceRestore) -> Self {
        Self {
            inner: Arc::new(PersistenceHealthInner {
                restore,
                successful_flushes: AtomicU64::new(0),
            }),
        }
    }

    pub(super) fn snapshot(&self) -> PersistenceSnapshot {
        PersistenceSnapshot {
            restore: self.inner.restore,
            successful_flushes: self.inner.successful_flushes.load(Ordering::Relaxed),
        }
    }

    fn record_flush(&self) {
        self.inner
            .successful_flushes
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PersistenceSnapshot {
    pub(crate) restore: PersistenceRestore,
    pub(crate) successful_flushes: u64,
}

pub(super) struct PersistenceWorker {
    handle: PrnsNodeHandle,
    storage: Arc<Mutex<PersistenceStorage>>,
    rotated: mpsc::UnboundedReceiver<DestinationHash>,
    health: PersistenceHealth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlushOutcome {
    Landed,
    NodeStopped,
    Failed,
}

impl PersistenceWorker {
    pub(super) fn new(
        handle: PrnsNodeHandle,
        storage: PersistenceStorage,
        rotated: mpsc::UnboundedReceiver<DestinationHash>,
        restore: PersistenceRestore,
    ) -> Self {
        Self {
            handle,
            storage: Arc::new(Mutex::new(storage)),
            rotated,
            health: PersistenceHealth::new(restore),
        }
    }

    pub(super) fn health(&self) -> PersistenceHealth {
        self.health.clone()
    }

    pub(super) async fn initialize(&self) -> Result<(), ()> {
        if self.flush_state().await != FlushOutcome::Landed {
            return Err(());
        }
        if self.flush_ratchets().await != FlushOutcome::Landed {
            return Err(());
        }
        Ok(())
    }

    pub(super) async fn run(mut self, mut shutdown: oneshot::Receiver<()>) -> Result<(), ()> {
        let mut ticker = tokio::time::interval(PERSISTENCE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        let mut rotations_open = true;
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => return self.flush_shutdown().await,
                destination = self.rotated.recv(), if rotations_open => {
                    match destination {
                        Some(destination) => {
                            if self.flush_ratchet(destination).await != FlushOutcome::Landed {
                                return Err(());
                            }
                        }
                        None => rotations_open = false,
                    }
                }
                _ = ticker.tick() => {
                    match self.flush_state().await {
                        FlushOutcome::Landed => {}
                        FlushOutcome::NodeStopped | FlushOutcome::Failed => return Err(()),
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
            Ok(Ok(_)) => {
                self.health.record_flush();
                FlushOutcome::Landed
            }
            Ok(Err(FlushError::Store(error))) => {
                log::error!("Android runtime persistence failed: {error}");
                FlushOutcome::Failed
            }
            Ok(Err(FlushError::NodeStopped)) => FlushOutcome::NodeStopped,
            Err(error) => {
                log::error!("Android runtime persistence worker failed: {error}");
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
                log::error!("Android ratchet persistence failed: {error}");
                FlushOutcome::Failed
            }
            Err(error) => {
                log::error!("Android ratchet persistence worker failed: {error}");
                FlushOutcome::Failed
            }
        }
    }

    async fn flush_ratchets(&self) -> FlushOutcome {
        let Some(snapshot) = self.handle.snapshot_self_ratchets().await else {
            return FlushOutcome::NodeStopped;
        };
        self.store_ratchets(snapshot).await
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
                log::error!("Android ratchet persistence failed: {error}");
                FlushOutcome::Failed
            }
            Err(error) => {
                log::error!("Android ratchet persistence worker failed: {error}");
                FlushOutcome::Failed
            }
        }
    }

    async fn flush_shutdown(&self) -> Result<(), ()> {
        if self.flush_state().await != FlushOutcome::Landed {
            return Err(());
        }
        if self.flush_ratchets().await != FlushOutcome::Landed {
            return Err(());
        }
        Ok(())
    }
}
