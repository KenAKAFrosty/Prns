use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::persistence::{FileStoreConfirmation, FileStoreError, SnapshotRegion};

use super::host::WorkerStorage;
use super::io::{run_io, PersistenceIo, PersistenceIoOperation};

#[cfg(test)]
mod tests;

const CONFIRMATION_RETRY: Duration = Duration::from_millis(250);

pub(super) struct AuthorizationRevision(Arc<()>);

pub(super) struct AuthorizationState {
    revision: Arc<()>,
    owner: Option<Arc<()>>,
}

impl AuthorizationState {
    pub(super) fn new() -> Self {
        Self {
            revision: Arc::new(()),
            owner: None,
        }
    }

    pub(super) fn flush_revision(&self) -> Result<AuthorizationRevision, AuthorizationOwnerError> {
        if self.owner.is_some() {
            return Err(AuthorizationOwnerError::Busy);
        }
        Ok(AuthorizationRevision(Arc::clone(&self.revision)))
    }

    pub(super) fn accepts_flush(&self, revision: &AuthorizationRevision) -> bool {
        self.owner.is_none() && Arc::ptr_eq(&self.revision, &revision.0)
    }

    fn begin(&mut self) -> Result<Arc<()>, AuthorizationOwnerError> {
        if self.owner.is_some() {
            return Err(AuthorizationOwnerError::Busy);
        }
        let owner = Arc::new(());
        self.revision = Arc::clone(&owner);
        self.owner = Some(Arc::clone(&owner));
        Ok(owner)
    }

    fn check_owner(&self, owner: &Arc<()>) -> Result<(), AuthorizationOwnerError> {
        match &self.owner {
            Some(active) if Arc::ptr_eq(active, owner) => Ok(()),
            _ => Err(AuthorizationOwnerError::Lost),
        }
    }
}

#[derive(Debug)]
pub(crate) enum AuthorizationOwnerError {
    Busy,
    Lost,
    Task,
}

impl core::fmt::Display for AuthorizationOwnerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::Busy => "authorization persistence is owned by an unsettled transaction",
            Self::Lost => "authorization persistence ownership changed",
            Self::Task => "authorization persistence task stopped",
        })
    }
}

#[derive(Clone)]
pub struct RemoteControlAuthorizationPersistence {
    pub(super) storage: Arc<Mutex<WorkerStorage>>,
    pub(super) io: Arc<dyn PersistenceIo>,
}

pub(crate) struct AuthorizationTransaction {
    io: Arc<dyn PersistenceIo>,
    storage: Arc<Mutex<WorkerStorage>>,
    owner: Arc<()>,
}

impl RemoteControlAuthorizationPersistence {
    #[cfg(test)]
    pub(crate) fn pause_test_storage(&self) -> impl Drop + '_ {
        self.storage.lock().expect("test storage is not poisoned")
    }

    pub(crate) async fn begin(&self) -> Result<AuthorizationTransaction, AuthorizationOwnerError> {
        let storage = Arc::clone(&self.storage);
        let owner = run_io(
            &self.io,
            PersistenceIoOperation::AuthorizationBegin,
            move || {
                storage
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .authorization
                    .begin()
            },
        )
        .await
        .map_err(|_| AuthorizationOwnerError::Task)??;
        Ok(AuthorizationTransaction {
            storage: Arc::clone(&self.storage),
            owner,
            io: Arc::clone(&self.io),
        })
    }
}

impl AuthorizationTransaction {
    pub(crate) async fn store(
        &self,
        region: SnapshotRegion,
        snapshot: Vec<u8>,
    ) -> Result<Result<(), FileStoreError>, AuthorizationOwnerError> {
        let write_io = Arc::clone(&self.io);
        let confirm_io = Arc::clone(&self.io);
        self.store_with(
            region,
            snapshot,
            move |store, region, bytes| write_io.store_authorization(store, region, bytes),
            move |store, region, bytes| confirm_io.confirm_authorization(store, region, bytes),
        )
        .await
    }

    async fn store_with(
        &self,
        region: SnapshotRegion,
        snapshot: Vec<u8>,
        write: impl FnOnce(
                &mut crate::persistence::FileStore,
                SnapshotRegion,
                &[u8],
            ) -> Result<(), FileStoreError>
            + Send
            + 'static,
        confirm: impl Fn(
                &crate::persistence::FileStore,
                SnapshotRegion,
                &[u8],
            ) -> Result<FileStoreConfirmation, FileStoreError>
            + Send
            + Sync
            + 'static,
    ) -> Result<Result<(), FileStoreError>, AuthorizationOwnerError> {
        let snapshot: Arc<[u8]> = snapshot.into();
        let storage = Arc::clone(&self.storage);
        let owner = Arc::clone(&self.owner);
        let candidate = Arc::clone(&snapshot);
        let result = run_io(
            &self.io,
            PersistenceIoOperation::AuthorizationStore(region),
            move || {
                let mut storage = storage.lock().unwrap_or_else(|error| error.into_inner());
                storage.authorization.check_owner(&owner)?;
                Ok::<_, AuthorizationOwnerError>(write(&mut storage.store, region, &candidate))
            },
        )
        .await
        .map_err(|_| AuthorizationOwnerError::Task)??;
        match result {
            Err(FileStoreError::PublishedDurabilityUnconfirmed(_)) => {}
            definitive => return Ok(definitive),
        }
        let confirm = Arc::new(confirm);
        loop {
            tokio::time::sleep(CONFIRMATION_RETRY).await;
            let storage = Arc::clone(&self.storage);
            let owner = Arc::clone(&self.owner);
            let candidate = Arc::clone(&snapshot);
            let confirm = Arc::clone(&confirm);
            let confirmation = run_io(
                &self.io,
                PersistenceIoOperation::AuthorizationConfirm(region),
                move || {
                    let storage = storage.lock().unwrap_or_else(|error| error.into_inner());
                    storage.authorization.check_owner(&owner)?;
                    Ok::<_, AuthorizationOwnerError>(confirm(&storage.store, region, &candidate))
                },
            )
            .await
            .map_err(|_| AuthorizationOwnerError::Task)??;
            match confirmation {
                Ok(FileStoreConfirmation::Confirmed) => return Ok(Ok(())),
                Ok(FileStoreConfirmation::Missing | FileStoreConfirmation::Different) | Err(_) => {}
            }
        }
    }

    // Dropping an unresolved owner must not permit a stale flush or another transaction.
    pub(crate) async fn finish(self) -> Result<(), AuthorizationOwnerError> {
        let io = Arc::clone(&self.io);
        run_io(
            &io,
            PersistenceIoOperation::AuthorizationFinish,
            move || {
                let mut storage = self
                    .storage
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                storage.authorization.check_owner(&self.owner)?;
                storage.authorization.owner = None;
                Ok(())
            },
        )
        .await
        .map_err(|_| AuthorizationOwnerError::Task)?
    }
}
