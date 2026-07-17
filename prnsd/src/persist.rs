//! The daemon's persistence policy over the runtime's flush/seed mechanism: seed at boot, flush
//! on a tight interval, flush once more on shutdown.
//! RNS 1.3.5 persists every 12 hours (`Reticulum.PERSIST_INTERVAL`) because each of its persists
//! is an unconditional full rewrite; our interval flush skips regions whose sealed bytes did not
//! change, so a quiet tick costs 22 bytes of timebase and the interval affords 5 minutes — the
//! crash-loss window shrinks accordingly.
//! A shared-instance client neither seeds nor flushes (the reference's
//! `is_connected_to_shared_instance` gate): the instance owns the tables, so main only starts
//! this module's loops after winning the instance role or running standalone.

use core::time::Duration;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use personal_rns::identity::vault::FileVault;
use personal_rns::persistence::FileStore;
use personal_rns::runtime::{
    FlushError, FlushMark, PrepareFlushError, SelfRatchetSnapshot, SelfRatchetsSnapshot,
    TokioPrnsHandle,
};
use personal_rns::wire::DestinationHash;
use prnsd_control::ManagedProcess;

pub const PERSIST_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The snapshot directory: a `prns` subdir of the RNS storage dir, because a config dir shared
/// with stock RNS keeps its own msgpack `storage/tunnels`, and our sealed region of the same
/// name must never clobber it.
pub fn store_dir(storage_dir: &Path) -> PathBuf {
    storage_dir.join("prns")
}

struct PersistenceStorage {
    store: FileStore,
    vault: FileVault,
    mark: FlushMark,
}

pub struct Persistence {
    handle: TokioPrnsHandle,
    storage: Arc<Mutex<PersistenceStorage>>,
    rotated: tokio::sync::mpsc::UnboundedReceiver<DestinationHash>,
    interval: Duration,
}

enum FlushOutcome {
    Landed,
    NodeStopped,
    Failed,
}

impl Persistence {
    pub fn new(
        handle: TokioPrnsHandle,
        store: FileStore,
        vault: FileVault,
        rotated: tokio::sync::mpsc::UnboundedReceiver<DestinationHash>,
        interval: Duration,
    ) -> Self {
        Self {
            handle,
            storage: Arc::new(Mutex::new(PersistenceStorage {
                store,
                vault,
                mark: FlushMark::default(),
            })),
            rotated,
            interval,
        }
    }

    pub async fn run(mut self, managed: Option<&ManagedProcess>) {
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        let shutdown = shutdown_signal(managed);
        tokio::pin!(shutdown);
        let mut rotations_open = true;
        loop {
            tokio::select! {
                biased;
                () = &mut shutdown => {
                    self.flush_shutdown().await;
                    break;
                }
                destination = self.rotated.recv(), if rotations_open => {
                    match destination {
                        Some(destination) => {
                            if matches!(self.flush_ratchet(destination).await, FlushOutcome::NodeStopped) {
                                break;
                            }
                        }
                        None => rotations_open = false,
                    }
                }
                _ = ticker.tick() => {
                    if matches!(self.flush_state().await, FlushOutcome::NodeStopped) {
                        break;
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
            let mut storage = match storage.lock() {
                Ok(storage) => storage,
                Err(poisoned) => poisoned.into_inner(),
            };
            let PersistenceStorage { store, mark, .. } = &mut *storage;
            prepared.commit_to_store(store, mark)
        })
        .await;
        match committed {
            Ok(Ok(_)) => FlushOutcome::Landed,
            Ok(Err(FlushError::Store(error))) => {
                tracing::error!(event = "persistence_failed", error = ?error);
                FlushOutcome::Failed
            }
            Ok(Err(FlushError::NodeStopped)) => FlushOutcome::NodeStopped,
            Err(error) => {
                tracing::error!(event = "persistence_worker_failed", error = ?error);
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
            let mut storage = match storage.lock() {
                Ok(storage) => storage,
                Err(poisoned) => poisoned.into_inner(),
            };
            snapshot.store_into(&mut storage.vault)
        })
        .await;
        match committed {
            Ok(Ok(())) => FlushOutcome::Landed,
            Ok(Err(error)) => {
                tracing::error!(event = "ratchet_persistence_failed", error = ?error);
                FlushOutcome::Failed
            }
            Err(error) => {
                tracing::error!(event = "persistence_worker_failed", error = ?error);
                FlushOutcome::Failed
            }
        }
    }

    async fn store_ratchets(&self, snapshot: SelfRatchetsSnapshot) -> FlushOutcome {
        let storage = Arc::clone(&self.storage);
        let committed = tokio::task::spawn_blocking(move || {
            let mut storage = match storage.lock() {
                Ok(storage) => storage,
                Err(poisoned) => poisoned.into_inner(),
            };
            snapshot.store_into(&mut storage.vault)
        })
        .await;
        match committed {
            Ok(Ok(_)) => FlushOutcome::Landed,
            Ok(Err(error)) => {
                tracing::error!(event = "ratchet_persistence_failed", error = ?error);
                FlushOutcome::Failed
            }
            Err(error) => {
                tracing::error!(event = "persistence_worker_failed", error = ?error);
                FlushOutcome::Failed
            }
        }
    }

    async fn flush_shutdown(&self) {
        if matches!(self.flush_state().await, FlushOutcome::Landed) {
            tracing::info!(event = "state_persisted");
        }
        if let Some(snapshot) = self.handle.snapshot_self_ratchets().await {
            let _ = self.store_ratchets(snapshot).await;
        }
        tracing::info!(event = "daemon_shutdown");
    }
}

pub async fn run_until_shutdown(
    persistence: Option<Persistence>,
    managed: Option<&ManagedProcess>,
) {
    match persistence {
        Some(persistence) => persistence.run(managed).await,
        None => {
            shutdown_signal(managed).await;
            tracing::info!(event = "daemon_shutdown");
        }
    }
}

async fn shutdown_signal(managed: Option<&ManagedProcess>) {
    match managed {
        Some(managed) => {
            tokio::select! {
                () = managed_shutdown_signal(managed) => {}
                () = operating_system_shutdown_signal() => {}
            }
        }
        None => operating_system_shutdown_signal().await,
    }
}

async fn managed_shutdown_signal(managed: &ManagedProcess) {
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        interval.tick().await;
        match managed.stop_requested() {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                tracing::error!(event = "managed_control_failed", error = %error);
                return;
            }
        }
    }
}

#[cfg(unix)]
async fn operating_system_shutdown_signal() {
    let mut terminate =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(terminate) => terminate,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn operating_system_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
