use core::time::Duration;
use std::future;

use personal_rns::runtime::{PersistenceEvent, PersistenceTrigger};
use prnsd_control::ManagedProcess;

use crate::shutdown::ShutdownSignal;

pub(crate) struct PersistenceWorker {
    worker: personal_rns::runtime::PersistenceWorker,
}

impl PersistenceWorker {
    pub(super) fn new(worker: personal_rns::runtime::PersistenceWorker) -> Self {
        Self { worker }
    }
}

pub(super) async fn run_until_shutdown(
    persistence: Option<PersistenceWorker>,
    managed: Option<&ManagedProcess>,
    shutdown: Option<ShutdownSignal>,
) {
    match persistence {
        Some(persistence) => {
            let _ = persistence
                .worker
                .run(shutdown_signal(managed, shutdown), observe)
                .await;
        }
        None => shutdown_signal(managed, shutdown).await,
    }
    tracing::info!(event = "daemon_shutdown");
}

fn observe(event: PersistenceEvent<'_>) {
    match event {
        PersistenceEvent::Flushed {
            trigger: PersistenceTrigger::Shutdown,
            ..
        } => tracing::info!(event = "state_persisted"),
        PersistenceEvent::Flushed { .. } => {}
        PersistenceEvent::FlushFailed { error, .. } => {
            tracing::error!(event = "persistence_failed", %error);
        }
        PersistenceEvent::RatchetFlushFailed { error, .. } => {
            tracing::error!(event = "ratchet_persistence_failed", %error);
        }
        PersistenceEvent::RatchetsFlushed { .. } => {}
    }
}

async fn shutdown_signal(managed: Option<&ManagedProcess>, shutdown: Option<ShutdownSignal>) {
    tokio::select! {
        () = managed_shutdown_signal(managed) => {}
        () = operating_system_shutdown_signal() => {}
        () = tray_shutdown_signal(shutdown) => {}
    }
}

async fn managed_shutdown_signal(managed: Option<&ManagedProcess>) {
    let Some(managed) = managed else {
        return future::pending().await;
    };
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

async fn tray_shutdown_signal(shutdown: Option<ShutdownSignal>) {
    match shutdown {
        Some(shutdown) => shutdown.requested().await,
        None => future::pending().await,
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
