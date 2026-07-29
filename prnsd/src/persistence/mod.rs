use core::time::Duration;

use personal_rns::runtime::{NodePersistence, PrnsNodeHandle};
use personal_rns::wire::DestinationHash;
use prnsd_control::ManagedProcess;

use crate::shutdown::ShutdownSignal;

mod restore;
mod worker;

pub(crate) use restore::{restore, RestoreInputs};
pub(crate) use worker::PersistenceWorker;

const PERSIST_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub(crate) fn prepare_worker(
    persistence: NodePersistence,
    handle: PrnsNodeHandle,
    rotated: tokio::sync::mpsc::UnboundedReceiver<DestinationHash>,
) -> PersistenceWorker {
    PersistenceWorker::new(
        persistence
            .worker(handle)
            .with_flush_interval(PERSIST_INTERVAL)
            .with_ratchet_rotations(rotated),
    )
}

pub(crate) async fn run_until_shutdown(
    persistence: Option<PersistenceWorker>,
    managed: Option<&ManagedProcess>,
    shutdown: Option<ShutdownSignal>,
) {
    worker::run_until_shutdown(persistence, managed, shutdown).await;
}
