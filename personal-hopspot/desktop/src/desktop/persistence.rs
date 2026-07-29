use core::time::Duration;
use std::path::Path;

use personal_rns::runtime::{NodePersistence, PersistenceEvent, PrnsNodeHandle};
use personal_rns::wire::DestinationHash;
use tokio::sync::{mpsc, oneshot};

const PERSISTENCE_DIRECTORY: &str = "prns-hopspot";
const PERSISTENCE_INTERVAL: Duration = Duration::from_secs(60);
const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) fn open(storage_dir: &Path) -> Result<NodePersistence, std::io::Error> {
    NodePersistence::custom_dir(storage_dir.join(PERSISTENCE_DIRECTORY))
}

pub(super) struct ShutdownFlush {
    shutdown: oneshot::Sender<()>,
    flushed: std::sync::mpsc::Receiver<()>,
}

impl ShutdownFlush {
    pub(super) fn disabled() -> Self {
        let (shutdown, _) = oneshot::channel();
        let (_, flushed) = std::sync::mpsc::channel();
        Self { shutdown, flushed }
    }

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
    persistence: NodePersistence,
    handle: PrnsNodeHandle,
    rotated: mpsc::UnboundedReceiver<DestinationHash>,
) -> ShutdownFlush {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (flushed_tx, flushed_rx) = std::sync::mpsc::channel();
    let worker = persistence
        .worker(handle)
        .with_flush_interval(PERSISTENCE_INTERVAL)
        .with_ratchet_rotations(rotated);
    tokio::spawn(async move {
        let mut startup_observer = observe;
        let _ = worker.flush_now(&mut startup_observer).await;
        let _ = worker
            .run(
                async move {
                    let _ = shutdown_rx.await;
                },
                observe,
            )
            .await;
        let _ = flushed_tx.send(());
    });
    ShutdownFlush {
        shutdown: shutdown_tx,
        flushed: flushed_rx,
    }
}

fn observe(event: PersistenceEvent<'_>) {
    match event {
        PersistenceEvent::FlushFailed { trigger, error } => {
            tracing::warn!(
                event = "persistence_flush_failed",
                trigger = trigger.name(),
                %error,
            );
        }
        PersistenceEvent::RatchetFlushFailed { trigger, error } => {
            tracing::warn!(
                event = "persistence_ratchet_flush_failed",
                trigger = trigger.name(),
                %error,
            );
        }
        PersistenceEvent::Flushed { .. } | PersistenceEvent::RatchetsFlushed { .. } => {}
    }
}
