//! Selected-domain construction on the existing identity owner's single lane.
//! The factory may attach supported local schemas, but may not acquire live
//! effects. Readiness is published only after it succeeds. Startup failures
//! preserve all files, including partially created files, for explicit recovery.
use crate::stores::{self, Stores};
use prns_identities::{owned, IdentityStore};
use prns_identities_core::{Catalog, Storage};
use prns_identities_runtime as runtime;
use prns_storage::{files, Completion, StartError, Worker};
use std::{path::Path, task::Waker};

type DriverError<D, const S: usize> = <D as runtime::ScopedDriver<S>>::Error;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub identities: owned::Options,
    pub stores: stores::Options,
}
#[derive(Debug)]
pub enum Error<E> {
    Identity(owned::Error<E>),
    InvalidOptions(owned::Error<std::convert::Infallible>),
    Stores(stores::Error<std::convert::Infallible>),
    File(files::Error),
    Spawn(std::io::Error),
    WorkerStopped,
}
impl<E: std::fmt::Debug> std::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native local assembly: {self:?}")
    }
}
impl<E: std::fmt::Debug> std::error::Error for Error<E> {}

/// One catalog process lock and one physical worker for selected typed domains.
/// Independent domain clients retain admission handles, not ownership of the
/// worker or an identity's private activation key.
pub struct Owner<const I: usize, const O: usize, const S: usize, const L: usize, D, J>
where
    D: owned::CommandHandler<S, J>,
    DriverError<D, S>: Send + Sync + 'static,
{
    worker: Worker<owned::Job<J>, Error<DriverError<D, S>>>,
    client: owned::Client,
    diagnostics: owned::DiagnosticObserver<DriverError<D, S>>,
}
impl<const I: usize, const O: usize, const S: usize, const L: usize, D, J> Owner<I, O, S, L, D, J>
where
    D: owned::CommandHandler<S, J>,
    DriverError<D, S>: Send + Sync + 'static,
    J: Send + 'static,
{
    pub fn create_new(
        path: impl AsRef<Path>,
        options: Options,
        factory: impl FnOnce(
                Stores<I>,
                &Catalog<I, O, S>,
                Waker,
                owned::Commands<J>,
            ) -> Result<D, DriverError<D, S>>
            + Send
            + 'static,
    ) -> Result<Self, Error<DriverError<D, S>>> {
        Self::start(path.as_ref(), true, options, factory)
    }
    pub fn open_existing(
        path: impl AsRef<Path>,
        options: Options,
        factory: impl FnOnce(
                Stores<I>,
                &Catalog<I, O, S>,
                Waker,
                owned::Commands<J>,
            ) -> Result<D, DriverError<D, S>>
            + Send
            + 'static,
    ) -> Result<Self, Error<DriverError<D, S>>> {
        Self::start(path.as_ref(), false, options, factory)
    }
    fn start(
        path: &Path,
        create: bool,
        options: Options,
        factory: impl FnOnce(
                Stores<I>,
                &Catalog<I, O, S>,
                Waker,
                owned::Commands<J>,
            ) -> Result<D, DriverError<D, S>>
            + Send
            + 'static,
    ) -> Result<Self, Error<DriverError<D, S>>> {
        options
            .identities
            .validate::<I, O, S, L>()
            .map_err(Error::InvalidOptions)?;
        options.stores.validate::<I>().map_err(Error::Stores)?;
        let path =
            std::path::absolute(path).map_err(|error| Error::File(files::Error::Io(error)))?;
        let diagnostics = owned::DiagnosticObserver::default();
        let worker_diagnostics = diagnostics.clone();
        let worker = Worker::start(
            "prns-service-runtime",
            options.identities.queue_capacity,
            move |inbox| {
                if create {
                    files::create_private_directory(&path).map_err(Error::File)?;
                    files::create_private_directory(&path.join("domains")).map_err(Error::File)?;
                } else {
                    files::check_private_directory(&path).map_err(Error::File)?;
                }
                let stores = Stores::<I>::open(&path.join("domains"), options.stores)
                    .map_err(Error::Stores)?;
                let mut catalog = if create {
                    IdentityStore::<I, O, S>::create_new(path.join("identities"))
                } else {
                    IdentityStore::<I, O, S>::open(path.join("identities"))
                }
                .map_err(|error| Error::Identity(owned::Error::Store(error)))?;
                let mut snapshot = Box::new(Catalog::<I, O, S>::new([0; 16]));
                catalog
                    .load(&mut snapshot)
                    .map_err(|error| Error::Identity(owned::Error::Store(error)))?;
                let commands = owned::Commands::from_lane(inbox.client());
                owned::run::<I, O, S, L, D, J>(
                    inbox,
                    &mut catalog,
                    options.identities,
                    worker_diagnostics,
                    move |wake| factory(stores, &snapshot, wake, commands),
                )
                .map_err(Error::Identity)
            },
        )
        .map_err(|error| match error {
            StartError::Spawn(error) => Error::Spawn(error),
            StartError::Initialization(error) => error,
            StartError::NotReady | StartError::Panicked => Error::WorkerStopped,
        })?;
        let client = owned::Client::from_lane(worker.client(), worker.control_client(), O, S);
        Ok(Self {
            worker,
            client,
            diagnostics,
        })
    }
    pub fn identities(&self) -> owned::Client {
        self.client.clone()
    }
    pub fn commands(&self) -> owned::Commands<J> {
        owned::Commands::from_lane(self.worker.client())
    }

    pub fn runtime_control(&self) -> owned::RuntimeControl<J> {
        owned::RuntimeControl::from_lane(self.worker.control_client())
    }

    pub fn diagnostics(&self) -> owned::DiagnosticObserver<DriverError<D, S>> {
        self.diagnostics.clone()
    }
    /// Fences synchronously; accepted native effects own their cleanup even if
    /// the caller drops this observer or the owner itself.
    pub fn close(&self) -> Completion<Error<DriverError<D, S>>> {
        self.worker.request_close();
        self.worker.closed()
    }
}
