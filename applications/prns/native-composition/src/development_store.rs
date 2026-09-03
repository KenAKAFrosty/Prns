use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::directory::{self, DirectoryRequest, DirectoryResponse};

pub(crate) const DEVELOPMENT_META: TableDefinition<&str, u64> =
    TableDefinition::new("development_meta");
pub(crate) const CONTACTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("contacts");

const DEVELOPMENT_FORMAT_KEY: &str = "format";
const DEVELOPMENT_FORMAT: u64 = 1;
const STORE_LANE_CAPACITY: usize = 8;

pub(crate) type StoreReply = Result<DirectoryResponse, DevelopmentStoreFailure>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DevelopmentStoreFailure {
    Unavailable(String),
    ResetRequired(String),
}

impl DevelopmentStoreFailure {
    pub(crate) fn unavailable(detail: impl Into<String>) -> Self {
        Self::Unavailable(detail.into())
    }

    pub(crate) fn reset_required(reason: impl Into<String>) -> Self {
        Self::ResetRequired(reason.into())
    }
}

struct StoreJob {
    request: DirectoryRequest,
    response: SyncSender<StoreReply>,
}

pub(crate) struct DevelopmentStoreOwner {
    pub(crate) root: PathBuf,
    jobs: Option<SyncSender<StoreJob>>,
    join: Option<JoinHandle<()>>,
}

impl DevelopmentStoreOwner {
    pub(crate) fn open(root: &Path, database_path: &Path) -> Result<Self, DevelopmentStoreFailure> {
        let (jobs_tx, jobs_rx) = mpsc::sync_channel(STORE_LANE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let database_path = database_path.to_path_buf();
        let join = std::thread::Builder::new()
            .name("prns-app-development-store".to_owned())
            .spawn(move || {
                let database = open_database(&database_path);
                match database {
                    Ok(database) => {
                        let _ = ready_tx.send(Ok(()));
                        run_owner(&database, &jobs_rx);
                    }
                    Err(failure) => {
                        let _ = ready_tx.send(Err(failure));
                    }
                }
            })
            .map_err(|error| {
                DevelopmentStoreFailure::unavailable(format!(
                    "could not start the development database owner: {error}"
                ))
            })?;

        // redb open is a blocking, non-cancellable operation. Waiting here keeps
        // that thread owned; a timeout would detach a future database owner that
        // reset could neither drain nor join. All admitted work uses the bounded
        // lane above and callers apply bounded response waits.
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                root: root.to_path_buf(),
                jobs: Some(jobs_tx),
                join: Some(join),
            }),
            Ok(Err(failure)) => {
                let _ = join.join();
                Err(failure)
            }
            Err(_) => {
                let _ = join.join();
                Err(DevelopmentStoreFailure::unavailable(
                    "the development database owner exited before reporting readiness",
                ))
            }
        }
    }

    pub(crate) fn admit(
        &self,
        request: DirectoryRequest,
    ) -> Result<Receiver<StoreReply>, DevelopmentStoreFailure> {
        let Some(jobs) = self.jobs.as_ref() else {
            return Err(DevelopmentStoreFailure::unavailable(
                "the development database owner is closing",
            ));
        };
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        match jobs.try_send(StoreJob {
            request,
            response: response_tx,
        }) {
            Ok(()) => Ok(response_rx),
            Err(TrySendError::Full(_)) => Err(DevelopmentStoreFailure::unavailable(
                "the bounded development database lane is full",
            )),
            Err(TrySendError::Disconnected(_)) => Err(DevelopmentStoreFailure::unavailable(
                "the development database owner has stopped",
            )),
        }
    }

    pub(crate) fn close(mut self) -> Result<(), DevelopmentStoreFailure> {
        self.close_inner()
    }

    fn close_inner(&mut self) -> Result<(), DevelopmentStoreFailure> {
        self.jobs = None;
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        join.join().map_err(|_| {
            DevelopmentStoreFailure::unavailable(
                "the development database owner panicked while closing",
            )
        })
    }
}

impl Drop for DevelopmentStoreOwner {
    fn drop(&mut self) {
        let _ = self.close_inner();
    }
}

fn run_owner(database: &Database, jobs: &Receiver<StoreJob>) {
    while let Ok(job) = jobs.recv() {
        let reply = directory::execute(database, job.request);
        let _ = job.response.send(reply);
    }
}

fn open_database(path: &Path) -> Result<Database, DevelopmentStoreFailure> {
    let (database, fresh) = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => (
            Database::builder().create_file(file).map_err(|error| {
                classify_redb_error(error.into(), "create development database")
            })?,
            true,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
            Database::open(path)
                .map_err(|error| classify_redb_error(error.into(), "open development database"))?,
            false,
        ),
        Err(error) => {
            return Err(DevelopmentStoreFailure::unavailable(format!(
                "could not create the development database file: {error}"
            )))
        }
    };

    if fresh {
        initialize_database(&database)?;
    } else {
        validate_existing_database(&database)?;
    }
    Ok(database)
}

fn initialize_database(database: &Database) -> Result<(), DevelopmentStoreFailure> {
    let write = database
        .begin_write()
        .map_err(|error| classify_redb_error(error.into(), "initialize development database"))?;
    {
        let mut meta = write.open_table(DEVELOPMENT_META).map_err(|error| {
            classify_redb_error(error.into(), "open development metadata table")
        })?;
        meta.insert(DEVELOPMENT_FORMAT_KEY, DEVELOPMENT_FORMAT)
            .map_err(|error| {
                classify_redb_error(error.into(), "write development format marker")
            })?;
    }
    {
        write.open_table(CONTACTS).map_err(|error| {
            classify_redb_error(error.into(), "create development contacts table")
        })?;
    }
    write.commit().map_err(|error| {
        classify_redb_error(error.into(), "commit development database initialization")
    })
}

fn validate_existing_database(database: &Database) -> Result<(), DevelopmentStoreFailure> {
    let read = database
        .begin_read()
        .map_err(|error| classify_redb_error(error.into(), "read development database"))?;
    let meta = read
        .open_table(DEVELOPMENT_META)
        .map_err(|error| classify_redb_error(error.into(), "open development metadata table"))?;
    let marker = meta
        .get(DEVELOPMENT_FORMAT_KEY)
        .map_err(|error| classify_redb_error(error.into(), "read development format marker"))?
        .map(|value| value.value());
    if marker != Some(DEVELOPMENT_FORMAT) {
        return Err(DevelopmentStoreFailure::reset_required(match marker {
            Some(found) => format!(
                "the development database format marker is {found}, expected {DEVELOPMENT_FORMAT}"
            ),
            None => "the development database format marker is missing".to_owned(),
        }));
    }
    let contacts = read
        .open_table(CONTACTS)
        .map_err(|error| classify_redb_error(error.into(), "open development contacts table"))?;
    let entries = contacts
        .iter()
        .map_err(|error| classify_redb_error(error.into(), "scan development contacts table"))?;
    for entry in entries {
        let (key, value) = entry.map_err(|error| {
            classify_redb_error(error.into(), "scan development contact record")
        })?;
        directory::decode_contact(key.value(), value.value())?;
    }
    Ok(())
}

pub(crate) fn classify_redb_error(error: redb::Error, operation: &str) -> DevelopmentStoreFailure {
    let detail = format!("could not {operation}: {error}");
    match error {
        redb::Error::Corrupted(_)
        | redb::Error::UpgradeRequired(_)
        | redb::Error::RepairAborted
        | redb::Error::TableTypeMismatch { .. }
        | redb::Error::TableIsMultimap(_)
        | redb::Error::TableIsNotMultimap(_)
        | redb::Error::TypeDefinitionChanged { .. }
        | redb::Error::TableDoesNotExist(_) => DevelopmentStoreFailure::reset_required(detail),
        redb::Error::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
            ) =>
        {
            DevelopmentStoreFailure::reset_required(detail)
        }
        _ => DevelopmentStoreFailure::unavailable(detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ContactListOutcome, ContactMutationOutcome};
    use crate::directory::DirectoryRequest;

    fn call(owner: &DevelopmentStoreOwner, request: DirectoryRequest) -> StoreReply {
        owner.admit(request).unwrap().recv().unwrap()
    }

    #[test]
    fn first_open_is_atomic_and_reopens() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &path).unwrap();
        assert_eq!(
            call(&owner, DirectoryRequest::List).unwrap(),
            DirectoryResponse::List(ContactListOutcome::Listed { contacts: vec![] })
        );
        owner.close().unwrap();

        let database = Database::open(&path).unwrap();
        let read = database.begin_read().unwrap();
        let meta = read.open_table(DEVELOPMENT_META).unwrap();
        assert_eq!(
            meta.get(DEVELOPMENT_FORMAT_KEY).unwrap().unwrap().value(),
            DEVELOPMENT_FORMAT
        );
        read.open_table(CONTACTS).unwrap();
        drop(read);
        drop(database);

        DevelopmentStoreOwner::open(root.path(), &path)
            .unwrap()
            .close()
            .unwrap();
    }

    #[test]
    fn missing_marker_requires_reset() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        let database = Database::create(&path).unwrap();
        let write = database.begin_write().unwrap();
        write.open_table(DEVELOPMENT_META).unwrap();
        write.open_table(CONTACTS).unwrap();
        write.commit().unwrap();
        drop(database);

        assert!(matches!(
            DevelopmentStoreOwner::open(root.path(), &path),
            Err(DevelopmentStoreFailure::ResetRequired(_))
        ));
    }

    #[test]
    fn existing_empty_database_requires_reset() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        std::fs::File::create(&path).unwrap();

        let failure = match DevelopmentStoreOwner::open(root.path(), &path) {
            Ok(_) => panic!("an existing empty database unexpectedly opened"),
            Err(failure) => failure,
        };
        assert!(
            matches!(failure, DevelopmentStoreFailure::ResetRequired(_)),
            "unexpected classification: {failure:?}"
        );
    }

    #[test]
    fn wrong_marker_requires_reset() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        DevelopmentStoreOwner::open(root.path(), &path)
            .unwrap()
            .close()
            .unwrap();
        let database = Database::open(&path).unwrap();
        let write = database.begin_write().unwrap();
        {
            let mut meta = write.open_table(DEVELOPMENT_META).unwrap();
            meta.insert(DEVELOPMENT_FORMAT_KEY, DEVELOPMENT_FORMAT + 1)
                .unwrap();
        }
        write.commit().unwrap();
        drop(database);

        assert!(matches!(
            DevelopmentStoreOwner::open(root.path(), &path),
            Err(DevelopmentStoreFailure::ResetRequired(_))
        ));
    }

    #[test]
    fn malformed_record_is_rejected_during_open_scan() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        DevelopmentStoreOwner::open(root.path(), &path)
            .unwrap()
            .close()
            .unwrap();
        let database = Database::open(&path).unwrap();
        let write = database.begin_write().unwrap();
        {
            let mut contacts = write.open_table(CONTACTS).unwrap();
            contacts
                .insert(
                    [9_u8; 16].as_slice(),
                    br#"{"alias":null,"pinned":false}"#.as_slice(),
                )
                .unwrap();
        }
        write.commit().unwrap();
        drop(database);

        assert!(matches!(
            DevelopmentStoreOwner::open(root.path(), &path),
            Err(DevelopmentStoreFailure::ResetRequired(_))
        ));
    }

    #[test]
    fn noncanonical_records_are_rejected_during_open_scan() {
        for encoded in [
            br#"{"alias":" Alice ","identity":null,"pinned":false}"#.as_slice(),
            br#"{"alias":null,"identity":null,"pinned":true}"#.as_slice(),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("application.redb");
            DevelopmentStoreOwner::open(root.path(), &path)
                .unwrap()
                .close()
                .unwrap();
            let database = Database::open(&path).unwrap();
            let write = database.begin_write().unwrap();
            {
                let mut contacts = write.open_table(CONTACTS).unwrap();
                contacts.insert([9_u8; 16].as_slice(), encoded).unwrap();
            }
            write.commit().unwrap();
            drop(database);

            assert!(matches!(
                DevelopmentStoreOwner::open(root.path(), &path),
                Err(DevelopmentStoreFailure::ResetRequired(_))
            ));
        }
    }

    #[test]
    fn database_lock_is_unavailable_without_reset_advice() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        DevelopmentStoreOwner::open(root.path(), &path)
            .unwrap()
            .close()
            .unwrap();
        let held = Database::open(&path).unwrap();
        assert!(matches!(
            DevelopmentStoreOwner::open(root.path(), &path),
            Err(DevelopmentStoreFailure::Unavailable(_))
        ));
        drop(held);
    }

    #[test]
    fn owner_drains_before_database_can_be_removed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &path).unwrap();
        let response = owner.admit(DirectoryRequest::CreateManual {
            destination: [7; 16],
            identity: Some([8; 16]),
            alias: Some(" Alice ".to_owned()),
        });
        owner.close().unwrap();
        assert!(matches!(
            response.unwrap().recv().unwrap().unwrap(),
            DirectoryResponse::Mutation(ContactMutationOutcome::Saved { .. })
        ));
        std::fs::remove_file(path).unwrap();
    }
}
