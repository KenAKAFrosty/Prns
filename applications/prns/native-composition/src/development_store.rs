use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::thread::JoinHandle;

use prns_lxmf::mailbox::{
    execute_mailbox_request, initialize_mailbox_tables, validate_mailbox_database, MailboxFailure,
    MailboxFuture, MailboxReply, MailboxRequest, MailboxSubmitter,
};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use tokio::sync::oneshot;

use crate::directory::{self, DirectoryRequest, DirectoryResponse};

pub(crate) const DEVELOPMENT_META: TableDefinition<&str, u64> =
    TableDefinition::new("development_meta");
pub(crate) const CONTACTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("contacts");

const DEVELOPMENT_FORMAT_KEY: &str = "format";
const DEVELOPMENT_FORMAT: u64 = 2;
const STORE_LANE_CAPACITY: usize = 8;

pub(crate) type StoreReply = Result<DirectoryResponse, DevelopmentStoreFailure>;
pub(crate) type MailboxStoreReply = Result<MailboxReply, MailboxFailure>;

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

enum StoreJob {
    DirectoryAsync {
        request: DirectoryRequest,
        response: oneshot::Sender<StoreReply>,
    },
    MailboxAsync {
        request: MailboxRequest,
        response: oneshot::Sender<MailboxStoreReply>,
    },
    #[cfg(test)]
    Barrier {
        entered: SyncSender<()>,
        release: Receiver<()>,
    },
}

struct StoreLane {
    jobs: StdMutex<Option<SyncSender<StoreJob>>>,
}

#[derive(Clone)]
pub(crate) struct DevelopmentMailboxSubmitter {
    lane: Weak<StoreLane>,
}

impl MailboxSubmitter for DevelopmentMailboxSubmitter {
    fn submit(&self, request: MailboxRequest) -> MailboxFuture<'_> {
        let submitter = self.clone();
        Box::pin(async move {
            let Some(lane) = submitter.lane.upgrade() else {
                return Err(MailboxFailure::Unavailable(
                    "the development database owner has closed".to_owned(),
                ));
            };
            let jobs = lane
                .jobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .ok_or_else(|| {
                    MailboxFailure::Unavailable(
                        "the development database owner is closing".to_owned(),
                    )
                })?;
            let (response, receiver) = oneshot::channel();
            match jobs.try_send(StoreJob::MailboxAsync { request, response }) {
                Ok(()) => receiver.await.map_err(|_| {
                    MailboxFailure::Unavailable(
                        "the development database owner stopped before replying".to_owned(),
                    )
                })?,
                Err(TrySendError::Full(_)) => Err(MailboxFailure::Busy),
                Err(TrySendError::Disconnected(_)) => Err(MailboxFailure::Unavailable(
                    "the development database owner has stopped".to_owned(),
                )),
            }
        })
    }
}

pub(crate) struct DevelopmentStoreOwner {
    pub(crate) root: PathBuf,
    lane: Arc<StoreLane>,
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
        // reset could neither drain nor join. All work uses the bounded
        // admission lane above; read-only callers may bound their waits, while
        // admitted mutation waiters are retained through a definitive reply.
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                root: root.to_path_buf(),
                lane: Arc::new(StoreLane {
                    jobs: StdMutex::new(Some(jobs_tx)),
                }),
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

    pub(crate) fn admit_directory_async(
        &self,
        request: DirectoryRequest,
    ) -> Result<oneshot::Receiver<StoreReply>, DevelopmentStoreFailure> {
        let jobs = self.jobs()?;
        let (response, receiver) = oneshot::channel();
        jobs.try_send(StoreJob::DirectoryAsync { request, response })
            .map_err(|_| {
                DevelopmentStoreFailure::unavailable(
                    "the development database lane is full or closed",
                )
            })?;
        Ok(receiver)
    }

    pub(crate) fn admit_mailbox_async(
        &self,
        request: MailboxRequest,
    ) -> Result<oneshot::Receiver<MailboxStoreReply>, MailboxFailure> {
        let jobs = self.jobs().map_err(map_development_failure_to_mailbox)?;
        let (response, receiver) = oneshot::channel();
        jobs.try_send(StoreJob::MailboxAsync { request, response })
            .map_err(|_| MailboxFailure::Busy)?;
        Ok(receiver)
    }

    pub(crate) fn mailbox_submitter(&self) -> Arc<dyn MailboxSubmitter> {
        Arc::new(DevelopmentMailboxSubmitter {
            lane: Arc::downgrade(&self.lane),
        })
    }

    #[cfg(test)]
    pub(crate) fn admit_test_barrier(
        &self,
        entered: SyncSender<()>,
        release: Receiver<()>,
    ) -> Result<(), DevelopmentStoreFailure> {
        self.jobs()?
            .try_send(StoreJob::Barrier { entered, release })
            .map_err(|error| match error {
                TrySendError::Full(_) => DevelopmentStoreFailure::unavailable(
                    "the bounded development database lane is full",
                ),
                TrySendError::Disconnected(_) => DevelopmentStoreFailure::unavailable(
                    "the development database owner has stopped",
                ),
            })
    }

    fn jobs(&self) -> Result<SyncSender<StoreJob>, DevelopmentStoreFailure> {
        self.lane
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                DevelopmentStoreFailure::unavailable("the development database owner is closing")
            })
    }

    pub(crate) fn close(mut self) -> Result<(), DevelopmentStoreFailure> {
        self.close_inner()
    }

    fn close_inner(&mut self) -> Result<(), DevelopmentStoreFailure> {
        *self
            .lane
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
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
        match job {
            StoreJob::DirectoryAsync { request, response } => {
                // Admission owns a mutation through commit even if its caller
                // has dropped the receiving future.
                let _ = response.send(directory::execute(database, request));
            }
            StoreJob::MailboxAsync { request, response } => {
                let _ = response.send(execute_mailbox_request(database, request));
            }
            #[cfg(test)]
            StoreJob::Barrier { entered, release } => {
                let _ = entered.send(());
                let _ = release.recv();
            }
        }
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
    initialize_mailbox_tables(&write).map_err(map_mailbox_failure)?;
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
    drop(contacts);
    drop(read);
    validate_mailbox_database(database).map_err(map_mailbox_failure)
}

fn map_mailbox_failure(failure: MailboxFailure) -> DevelopmentStoreFailure {
    match failure {
        MailboxFailure::Busy => DevelopmentStoreFailure::unavailable(
            "the development mailbox owner was unexpectedly busy",
        ),
        MailboxFailure::Unavailable(detail) => DevelopmentStoreFailure::unavailable(detail),
        MailboxFailure::ResetRequired(reason) => DevelopmentStoreFailure::reset_required(reason),
    }
}

fn map_development_failure_to_mailbox(failure: DevelopmentStoreFailure) -> MailboxFailure {
    match failure {
        DevelopmentStoreFailure::Unavailable(detail) => MailboxFailure::Unavailable(detail),
        DevelopmentStoreFailure::ResetRequired(reason) => MailboxFailure::ResetRequired(reason),
    }
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

    fn mailbox_list() -> MailboxRequest {
        MailboxRequest::List(prns_lxmf::mailbox::MailboxListRequest {
            peer: None,
            direction: None,
            before: None,
            limit: 100,
        })
    }

    fn outbound_message() -> prns_lxmf::mailbox::NewOutboundMessage {
        let identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
            &[0x71; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        )
        .expect("fixed local identity");
        let source = identity.destination();
        let destination = [0x42; 16];
        let mut output = [0_u8; prns_lxmf::wire::MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = prns_lxmf::wire::compose_basic_direct_lxmf(
            destination,
            source,
            1_700_000_009_000,
            b"Retained waiter",
            b"Exact commit",
            None,
            &identity,
            &mut output,
        )
        .expect("small direct message");
        prns_lxmf::mailbox::NewOutboundMessage {
            message_id: prepared.message_id(),
            source,
            destination,
            timestamp_unix_ms: 1_700_000_009_000,
            title: b"Retained waiter".to_vec(),
            content: b"Exact commit".to_vec(),
            exact_wire: output[..usize::from(prepared.wire_len())].to_vec(),
        }
    }

    fn call(owner: &DevelopmentStoreOwner, request: DirectoryRequest) -> StoreReply {
        owner
            .admit_directory_async(request)
            .unwrap()
            .blocking_recv()
            .unwrap()
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
        prns_lxmf::mailbox::validate_mailbox_database(&database).unwrap();
        drop(database);

        DevelopmentStoreOwner::open(root.path(), &path)
            .unwrap()
            .close()
            .unwrap();
    }

    #[test]
    fn dropping_an_admitted_async_waiter_does_not_cancel_its_commit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &path).unwrap();
        let jobs = owner.jobs().unwrap();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        assert!(jobs
            .try_send(StoreJob::Barrier {
                entered: entered_tx,
                release: release_rx,
            })
            .is_ok());
        entered_rx.recv().expect("owner reached the barrier");

        let (response, waiter) = oneshot::channel();
        assert!(jobs
            .try_send(StoreJob::MailboxAsync {
                request: MailboxRequest::InsertOutbound(outbound_message()),
                response,
            })
            .is_ok());
        drop(waiter);
        release_tx.send(()).expect("release owner");
        drop(jobs);
        owner.close().unwrap();

        let reopened = DevelopmentStoreOwner::open(root.path(), &path).unwrap();
        let reply = reopened
            .admit_mailbox_async(mailbox_list())
            .unwrap()
            .blocking_recv()
            .unwrap()
            .unwrap();
        let MailboxReply::Listed { messages, .. } = reply else {
            panic!("unexpected mailbox reply");
        };
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0].delivery_state,
            prns_lxmf::mailbox::DurableLxmfDeliveryState::Queued { .. }
        ));
        reopened.close().unwrap();
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
    fn schema_v1_requires_explicit_reset_without_migration() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        let database = Database::create(&path).unwrap();
        let write = database.begin_write().unwrap();
        {
            let mut meta = write.open_table(DEVELOPMENT_META).unwrap();
            meta.insert(DEVELOPMENT_FORMAT_KEY, 1).unwrap();
        }
        write.open_table(CONTACTS).unwrap();
        write.commit().unwrap();
        drop(database);

        assert!(matches!(
            DevelopmentStoreOwner::open(root.path(), &path),
            Err(DevelopmentStoreFailure::ResetRequired(reason)) if reason.contains("is 1, expected 2")
        ));
    }

    #[test]
    fn directory_and_mailbox_jobs_share_one_bounded_owner() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &path).unwrap();
        assert!(matches!(
            call(
                &owner,
                DirectoryRequest::CreateManual {
                    destination: [7; 16],
                    identity: None,
                    alias: Some("Shared owner".to_owned()),
                },
            )
            .unwrap(),
            DirectoryResponse::Mutation(ContactMutationOutcome::Saved { .. })
        ));
        let mailbox = owner
            .admit_mailbox_async(mailbox_list())
            .unwrap()
            .blocking_recv()
            .unwrap()
            .unwrap();
        assert!(matches!(
            mailbox,
            MailboxReply::Listed { messages, revision: 0 } if messages.is_empty()
        ));
        assert!(matches!(
            call(&owner, DirectoryRequest::List).unwrap(),
            DirectoryResponse::List(ContactListOutcome::Listed { contacts }) if contacts.len() == 1
        ));
        owner.close().unwrap();
    }

    #[tokio::test]
    async fn cloned_mailbox_submitter_cannot_keep_the_owner_open() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &path).unwrap();
        let submitter = owner.mailbox_submitter();
        owner.close().unwrap();

        assert!(matches!(
            submitter.submit(mailbox_list()).await,
            Err(MailboxFailure::Unavailable(_))
        ));
        drop(Database::open(path).unwrap());
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
        let response = owner.admit_directory_async(DirectoryRequest::CreateManual {
            destination: [7; 16],
            identity: Some([8; 16]),
            alias: Some(" Alice ".to_owned()),
        });
        let mailbox = owner.admit_mailbox_async(mailbox_list());
        owner.close().unwrap();
        assert!(matches!(
            response.unwrap().blocking_recv().unwrap().unwrap(),
            DirectoryResponse::Mutation(ContactMutationOutcome::Saved { .. })
        ));
        assert!(matches!(
            mailbox.unwrap().blocking_recv().unwrap().unwrap(),
            MailboxReply::Listed { .. }
        ));
        std::fs::remove_file(path).unwrap();
    }
}
