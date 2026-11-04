use super::{files, Error};
use prns_groups::{fixture, Client, Job, StoreSession, Submit};
use prns_storage::{CloseError, StartError, SubmitError, Worker};
use std::{io::Read, num::NonZeroUsize, path::Path, sync::Arc};
use zeroize::Zeroizing;

struct BorrowedLane(prns_storage::Client<Box<Job>>, prns_groups::InputBudget);
impl Submit for BorrowedLane {
    fn input_budget(&self) -> &prns_groups::InputBudget {
        &self.1
    }
    fn submit(&self, job: Job) -> Result<(), prns_groups::Error> {
        use prns_groups::Error::{Busy, Unavailable};
        self.0
            .try_submit(Box::new(job))
            .map_err(|error| match error {
                SubmitError::Full(_) => Busy,
                SubmitError::Closed(_) | SubmitError::Reentrant(_) => Unavailable,
            })
    }
}

pub(super) struct Storage {
    pub client: Client,
    worker: Worker<Box<Job>, Error>,
}
impl Storage {
    pub fn open(root: &Path, identity: prns_host::IdentityHash) -> Result<Self, Error> {
        files::private_file(&root.join("group-fixture.json"))?;
        files::regular_file_if_present(&root.join("application.redb"))?;
        let mut fixture = Zeroizing::new(Vec::new());
        std::fs::File::open(root.join("group-fixture.json"))?
            .take(fixture::MAX_FIXTURE_BYTES as u64 + 1)
            .read_to_end(&mut fixture)?;
        if fixture.len() > fixture::MAX_FIXTURE_BYTES {
            return Err(Error::FixtureTooLarge);
        }
        let database_path = root.join("application.redb");
        let worker = Worker::start(
            "group-peer-storage",
            NonZeroUsize::new(8).ok_or(Error::StorageWorker)?,
            move |inbox: prns_storage::Inbox<Box<Job>>| {
                let database = redb::Database::create(database_path).map_err(Error::Database)?;
                let provisioned = fixture::load_and_install(&database, &fixture, identity)
                    .map_err(Error::Fixture)?;
                let mut session =
                    StoreSession::open(&database, provisioned).map_err(Error::Runner)?;
                drop(fixture);
                // Database and its borrowing manager stay on this thread. The
                // domain handler owns its transactions and result publication.
                inbox.run(|job| job.execute(&mut session));
                Ok(())
            },
        )
        .map_err(|error| match error {
            StartError::Spawn(error) => Error::Io(error),
            StartError::Initialization(error) => error,
            StartError::NotReady | StartError::Panicked => Error::StorageWorker,
        })?;
        let client = Client::new(Arc::new(BorrowedLane(worker.client(), Default::default())));
        Ok(Self { client, worker })
    }
    pub fn stop(&mut self) -> Result<(), Error> {
        self.worker.close().map_err(|error| match error {
            CloseError::Failed(_) => Error::StorageWorker,
            CloseError::Panicked | CloseError::Reentrant => Error::StorageWorker,
        })
    }
}
