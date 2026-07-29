use core::time::Duration;
use std::future;

use personal_rns::runtime::{PersistenceEvent, PersistenceFlushStatus, PersistenceTrigger};
use prnsd_control::ManagedProcess;

use crate::shutdown::ShutdownSignal;

pub(crate) struct PersistenceWorker {
    worker: personal_rns::runtime::PersistenceWorker,
}

pub(crate) struct OperatingSystemShutdown {
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
    #[cfg(windows)]
    control_c: Option<tokio::signal::windows::CtrlC>,
    #[cfg(not(any(unix, windows)))]
    unsupported: (),
}

impl OperatingSystemShutdown {
    pub(crate) fn capture() -> Self {
        #[cfg(unix)]
        {
            Self {
                interrupt:
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok(),
                terminate:
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok(),
            }
        }
        #[cfg(windows)]
        {
            Self {
                control_c: tokio::signal::windows::ctrl_c().ok(),
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            Self { unsupported: () }
        }
    }

    async fn requested(mut self) {
        #[cfg(unix)]
        match (self.interrupt.as_mut(), self.terminate.as_mut()) {
            (Some(interrupt), Some(terminate)) => {
                tokio::select! {
                    _ = interrupt.recv() => {}
                    _ = terminate.recv() => {}
                }
            }
            (Some(interrupt), None) => {
                let _ = interrupt.recv().await;
            }
            (None, Some(terminate)) => {
                let _ = terminate.recv().await;
            }
            (None, None) => future::pending().await,
        }
        #[cfg(windows)]
        match self.control_c.as_mut() {
            Some(control_c) => {
                let _ = control_c.recv().await;
            }
            None => future::pending().await,
        }
        #[cfg(not(any(unix, windows)))]
        future::pending().await
    }
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
    operating_system_shutdown: OperatingSystemShutdown,
) -> PersistenceFlushStatus {
    let status = match persistence {
        Some(persistence) => {
            persistence
                .worker
                .run(
                    shutdown_signal(managed, shutdown, operating_system_shutdown),
                    observe,
                )
                .await
        }
        None => {
            shutdown_signal(managed, shutdown, operating_system_shutdown).await;
            PersistenceFlushStatus::Landed
        }
    };
    tracing::info!(event = "daemon_shutdown");
    status
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
        PersistenceEvent::RatchetsFlushed {
            trigger: PersistenceTrigger::Shutdown,
            ..
        } => tracing::info!(event = "ratchets_persisted"),
        PersistenceEvent::RatchetsFlushed { .. } => {}
    }
}

async fn shutdown_signal(
    managed: Option<&ManagedProcess>,
    shutdown: Option<ShutdownSignal>,
    operating_system_shutdown: OperatingSystemShutdown,
) {
    tokio::select! {
        () = managed_shutdown_signal(managed) => {}
        () = operating_system_shutdown.requested() => {}
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
