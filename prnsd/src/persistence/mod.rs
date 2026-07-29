use core::time::Duration;

use personal_rns::runtime::{
    FlushFailurePolicy, NodePersistence, PersistenceFlushStatus, PrnsNodeHandle,
};
use personal_rns::wire::DestinationHash;
use prnsd_control::ManagedProcess;

use crate::cli::PersistencePolicy;
use crate::shutdown::ShutdownSignal;

mod restore;
mod worker;

pub(crate) use restore::{restore, RestoreInputs};
pub(crate) use worker::{OperatingSystemShutdown, PersistenceWorker};

pub(crate) fn capture_operating_system_shutdown() -> OperatingSystemShutdown {
    OperatingSystemShutdown::capture()
}

const PERSIST_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub(crate) fn prepare_worker(
    persistence: NodePersistence,
    handle: PrnsNodeHandle,
    rotated: tokio::sync::mpsc::UnboundedReceiver<DestinationHash>,
    policy: PersistencePolicy,
) -> PersistenceWorker {
    let worker = persistence
        .worker(handle)
        .with_flush_interval(PERSIST_INTERVAL)
        .with_ratchet_rotations(rotated);
    let worker = match policy {
        PersistencePolicy::BestEffort => worker,
        PersistencePolicy::Required => worker.with_flush_failure_policy(FlushFailurePolicy::Exit),
    };
    PersistenceWorker::new(worker)
}

pub(crate) async fn run_until_shutdown(
    persistence: Option<PersistenceWorker>,
    managed: Option<&ManagedProcess>,
    shutdown: Option<ShutdownSignal>,
    operating_system_shutdown: OperatingSystemShutdown,
) -> PersistenceFlushStatus {
    worker::run_until_shutdown(persistence, managed, shutdown, operating_system_shutdown).await
}
