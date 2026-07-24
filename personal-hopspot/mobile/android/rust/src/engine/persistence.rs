use core::time::Duration;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use personal_rns::runtime::{
    FlushFailurePolicy, NodePersistence, PersistenceEvent, PersistenceFlushStatus,
    PersistenceRestoreReport, PrnsNodeHandle,
};
use personal_rns::wire::DestinationHash;
use tokio::sync::{mpsc, oneshot};

const PERSISTENCE_DIRECTORY: &str = "prns";
const PERSISTENCE_INTERVAL: Duration = Duration::from_secs(30);

pub(super) fn open(storage_dir: &Path) -> Result<NodePersistence, std::io::Error> {
    NodePersistence::open(storage_dir.join(PERSISTENCE_DIRECTORY))
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

impl PersistenceRestore {
    pub(super) fn from_report(report: &PersistenceRestoreReport) -> Self {
        Self {
            routes: report.routes.seeded_count,
            destination_identities: report.destination_identities.seeded_count,
            tunnels: report.tunnels.seeded_count,
            ratchets: report.ratchets.seeded_count,
            refused: report.refused_total(),
            dropped: report.dropped_total(),
        }
    }
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
    worker: personal_rns::runtime::PersistenceWorker,
    health: PersistenceHealth,
}

impl PersistenceWorker {
    pub(super) fn new(
        handle: PrnsNodeHandle,
        persistence: NodePersistence,
        rotated: mpsc::UnboundedReceiver<DestinationHash>,
        restore: PersistenceRestore,
    ) -> Self {
        Self {
            worker: persistence
                .worker(handle)
                .with_flush_interval(PERSISTENCE_INTERVAL)
                .with_ratchet_rotations(rotated)
                .with_flush_failure_policy(FlushFailurePolicy::Exit),
            health: PersistenceHealth::new(restore),
        }
    }

    pub(super) fn health(&self) -> PersistenceHealth {
        self.health.clone()
    }

    pub(super) async fn initialize(&self) -> Result<(), ()> {
        let health = self.health.clone();
        let mut observer = move |event: PersistenceEvent<'_>| observe(&health, event);
        match self.worker.flush_now(&mut observer).await {
            PersistenceFlushStatus::Landed => Ok(()),
            PersistenceFlushStatus::NodeStopped | PersistenceFlushStatus::Failed => Err(()),
        }
    }

    pub(super) async fn run(self, shutdown: oneshot::Receiver<()>) -> Result<(), ()> {
        let health = self.health;
        let status = self
            .worker
            .run(
                async move {
                    let _ = shutdown.await;
                },
                move |event| observe(&health, event),
            )
            .await;
        match status {
            PersistenceFlushStatus::Landed => Ok(()),
            PersistenceFlushStatus::NodeStopped | PersistenceFlushStatus::Failed => Err(()),
        }
    }
}

fn observe(health: &PersistenceHealth, event: PersistenceEvent<'_>) {
    match event {
        PersistenceEvent::Flushed { .. } => health.record_flush(),
        PersistenceEvent::FlushFailed { error, .. } => {
            log::error!("Android runtime persistence failed: {error}");
        }
        PersistenceEvent::RatchetFlushFailed { error, .. } => {
            log::error!("Android ratchet persistence failed: {error}");
        }
        PersistenceEvent::RatchetsFlushed { .. } => {}
    }
}
