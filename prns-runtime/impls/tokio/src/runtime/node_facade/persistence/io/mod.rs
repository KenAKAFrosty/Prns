use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::oneshot;

use crate::persistence::{
    FileStore, FileStoreConfirmation, FileStoreError, PersistedStore, SnapshotRegion,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistenceIoOperation {
    AuthorizationBegin,
    AuthorizationStore(SnapshotRegion),
    AuthorizationConfirm(SnapshotRegion),
    AuthorizationFinish,
    FlushRevision,
    FlushCommit,
    VaultStore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistenceIoError {
    WorkerStopped,
    CompletionLost,
}

impl std::fmt::Display for PersistenceIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::WorkerStopped => "persistence I/O worker stopped",
            Self::CompletionLost => "persistence I/O completion was lost",
        })
    }
}

impl std::error::Error for PersistenceIoError {}

pub struct PersistenceIoTask {
    operation: PersistenceIoOperation,
    work: Box<dyn FnOnce() + Send>,
}

impl PersistenceIoTask {
    pub fn operation(&self) -> &PersistenceIoOperation {
        &self.operation
    }

    pub fn run(self) {
        (self.work)();
    }
}

pub type PersistenceIoCompletion<'a> =
    Pin<Box<dyn Future<Output = Result<(), PersistenceIoError>> + Send + 'a>>;

/// Execution and storage effects belong to the host. Transaction ownership,
/// activation and confirmation retry policy remain in the persistence owner.
pub trait PersistenceIo: Send + Sync {
    fn execute(&self, task: PersistenceIoTask) -> PersistenceIoCompletion<'_>;

    fn store_authorization(
        &self,
        store: &mut FileStore,
        region: SnapshotRegion,
        snapshot: &[u8],
    ) -> Result<(), FileStoreError> {
        store.store(region, snapshot)
    }

    fn confirm_authorization(
        &self,
        store: &FileStore,
        region: SnapshotRegion,
        snapshot: &[u8],
    ) -> Result<FileStoreConfirmation, FileStoreError> {
        store.confirm_store(region, snapshot)
    }
}

pub(super) struct NativePersistenceIo;

impl PersistenceIo for NativePersistenceIo {
    fn execute(&self, task: PersistenceIoTask) -> PersistenceIoCompletion<'_> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || task.run())
                .await
                .map_err(|_| PersistenceIoError::WorkerStopped)
        })
    }
}

pub(super) async fn run_io<T: Send + 'static>(
    io: &Arc<dyn PersistenceIo>,
    operation: PersistenceIoOperation,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, PersistenceIoError> {
    let (completion, result) = oneshot::channel();
    io.execute(PersistenceIoTask {
        operation,
        work: Box::new(move || {
            let _delivered = completion.send(work());
        }),
    })
    .await?;
    result.await.map_err(|_| PersistenceIoError::CompletionLost)
}
