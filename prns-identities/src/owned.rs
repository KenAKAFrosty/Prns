//! One exclusive catalog, one bounded worker, and retained scoped effects.
//!
//! Opening does not activate saved enabled identities. Call `recover` explicitly.
//! Driver futures belong to the driver, never a caller's result future; Pending
//! work wakes this same worker and is re-polled under fresh scoped authority.

mod admission;
mod client;
mod dispatch;
mod driver;
pub use dispatch::DispatchLease;
mod engine;
pub use crate::api::{Client, IdentityClient, Snapshot};
pub use admission::{Access, Admission, AdmissionError, CommandHandler, Permit, RecoveryAdmission};
pub use client::{Commands, Job, RuntimeControl, SubmitFailure};
pub use driver::{Driver, IdentityOnly, KeyLease};

use crate::{creation, IdentityStore};
use prns_identities_core as core;
use prns_identities_runtime as runtime;
use prns_storage::{Completion, OwnerEvent, OwnerProgress, StartError, Worker};
use std::{
    num::NonZeroUsize,
    path::Path,
    sync::{Arc, Mutex},
    task::Waker,
};

#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityOwnerOptions"))]
pub struct Options {
    /// Queued commands, excluding the currently executing command (1..=1024).
    pub queue_capacity: NonZeroUsize,
    pub budget: runtime::Budget,
}

impl Options {
    /// Validate the same catalog, runtime and queue bounds used by both owner
    /// constructors and the borrowed runner, before an outer factory writes.
    pub fn validate<const I: usize, const O: usize, const S: usize, const L: usize>(
        self,
    ) -> Result<(), Error<std::convert::Infallible>> {
        validate::<I, O, S, L, _>(self)
    }
}

#[derive(Debug)]
pub enum Error<E> {
    InvalidOptions,
    Api(core::ApiError),
    Store(crate::Error),
    Creation(creation::CreateError),
    Authority(core::Fault),
    Runtime(runtime::Fault),
    Driver(E),
    Closing,
    WorkerStopped,
    Spawn(std::io::Error),
}
impl<E> Error<E> {
    /// Canonical public classification; retain this rich error separately when
    /// diagnostic details are needed. Classification never proves no effect.
    pub fn api_error(&self, classify: impl FnOnce(&E) -> core::ServiceFailure) -> core::ApiError {
        crate::api::owner_error(self, classify)
    }
}
impl<E: std::fmt::Debug> std::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "identity owner: {self:?}")
    }
}
impl<E: std::fmt::Debug> std::error::Error for Error<E> {}
impl<E> From<core::ApiError> for Error<E> {
    fn from(error: core::ApiError) -> Self {
        Self::Api(error)
    }
}
impl<E> From<crate::Error> for Error<E> {
    fn from(e: crate::Error) -> Self {
        Self::Store(e)
    }
}
impl<E> From<core::Fault> for Error<E> {
    fn from(e: core::Fault) -> Self {
        Self::Authority(e)
    }
}
impl<E> From<runtime::Fault> for Error<E> {
    fn from(e: runtime::Fault) -> Self {
        Self::Runtime(e)
    }
}
impl<E> From<core::Error<crate::Error>> for Error<E> {
    fn from(e: core::Error<crate::Error>) -> Self {
        match e {
            core::Error::Fault(e) => Self::Authority(e),
            core::Error::Storage(e) => Self::Store(e),
        }
    }
}
impl<E> From<runtime::Error<E>> for Error<E> {
    fn from(e: runtime::Error<E>) -> Self {
        match e {
            runtime::Error::Fault(e) => Self::Runtime(e),
            runtime::Error::Driver(e) => Self::Driver(e),
        }
    }
}

pub struct Diagnostic<E> {
    /// None for a catalog-wide failure or before creation identifies a persona.
    pub identity: Option<core::IdentityReference>,
    pub error: Arc<Error<E>>,
}
impl<E> Clone for Diagnostic<E> {
    fn clone(&self) -> Self {
        Self {
            identity: self.identity,
            error: self.error.clone(),
        }
    }
}
type Diagnostics<E> = Arc<Mutex<Option<Diagnostic<E>>>>;

/// Retained diagnostic observation, independent of owner/finalizer lifetime.
/// An error observation does not prove that cleanup is complete.
pub struct DiagnosticObserver<E> {
    inner: Diagnostics<E>,
}
impl<E> Clone for DiagnosticObserver<E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
impl<E> Default for DiagnosticObserver<E> {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }
}
impl<E> DiagnosticObserver<E> {
    pub fn latest(&self) -> Option<Diagnostic<E>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Owns the process lock until accepted work, scoped cleanup, and key leases
/// drain. Construction blocks for readiness; `close` only fences synchronously.
/// Capacities are deliberate type parameters, with native memory validation.
pub struct Owner<
    const I: usize,
    const O: usize,
    const S: usize,
    const L: usize,
    D: Driver<S>,
    J = std::convert::Infallible,
> where
    <D as runtime::ScopedDriver<S>>::Error: Send + Sync + 'static,
{
    worker: Worker<client::Job<J>, Error<<D as runtime::ScopedDriver<S>>::Error>>,
    client: Client,
    diagnostics: Diagnostics<<D as runtime::ScopedDriver<S>>::Error>,
}
impl<const I: usize, const O: usize, const S: usize, const L: usize, D, J> Owner<I, O, S, L, D, J>
where
    D: CommandHandler<S, J> + Send + 'static,
    J: Send + 'static,
    <D as runtime::ScopedDriver<S>>::Error: Send + Sync + 'static,
{
    pub fn create_new(
        path: impl AsRef<Path>,
        options: Options,
        factory: impl FnOnce(Waker) -> Result<D, <D as runtime::ScopedDriver<S>>::Error>
            + Send
            + 'static,
    ) -> Result<Self, Error<<D as runtime::ScopedDriver<S>>::Error>> {
        Self::start(path.as_ref(), true, options, factory)
    }
    pub fn open_existing(
        path: impl AsRef<Path>,
        options: Options,
        factory: impl FnOnce(Waker) -> Result<D, <D as runtime::ScopedDriver<S>>::Error>
            + Send
            + 'static,
    ) -> Result<Self, Error<<D as runtime::ScopedDriver<S>>::Error>> {
        Self::start(path.as_ref(), false, options, factory)
    }
    fn start(
        path: &Path,
        create: bool,
        options: Options,
        factory: impl FnOnce(Waker) -> Result<D, <D as runtime::ScopedDriver<S>>::Error>
            + Send
            + 'static,
    ) -> Result<Self, Error<<D as runtime::ScopedDriver<S>>::Error>> {
        validate::<I, O, S, L, _>(options)?;
        let path = std::path::absolute(path).map_err(crate::Error::Io)?;
        let diagnostics: Diagnostics<<D as runtime::ScopedDriver<S>>::Error> =
            Arc::new(Mutex::new(None));
        let worker_diagnostics = diagnostics.clone();
        let worker = Worker::start(
            "prns-identities",
            options.queue_capacity,
            move |inbox: prns_storage::Inbox<client::Job<J>>| {
                let mut store = if create {
                    IdentityStore::<I, O, S>::create_new(path)?
                } else {
                    IdentityStore::<I, O, S>::open(path)?
                };
                run::<I, O, S, L, D, J>(
                    inbox,
                    &mut store,
                    options,
                    DiagnosticObserver {
                        inner: worker_diagnostics,
                    },
                    factory,
                )
            },
        )
        .map_err(|e| match e {
            StartError::Spawn(e) => Error::Spawn(e),
            StartError::Initialization(e) => e,
            StartError::NotReady | StartError::Panicked => Error::WorkerStopped,
        })?;
        let client = Client::from_lane(worker.client(), worker.control_client(), O, S);
        Ok(Self {
            worker,
            client,
            diagnostics,
        })
    }
    pub fn client(&self) -> Client {
        self.client.clone()
    }
    /// Typed domain transport belongs to construction, not ordinary identity clients.
    pub fn commands(&self) -> Commands<J> {
        Commands::from_lane(self.worker.client())
    }

    /// Available after admission closes, including when a driver cannot yet
    /// prove drainage. Errors do not turn incomplete cleanup into completion.
    pub fn diagnostics(&self) -> DiagnosticObserver<<D as runtime::ScopedDriver<S>>::Error> {
        DiagnosticObserver {
            inner: self.diagnostics.clone(),
        }
    }
    pub fn close(&self) -> Completion<Error<<D as runtime::ScopedDriver<S>>::Error>> {
        self.worker.request_close();
        self.worker.closed()
    }
}

fn validate<const I: usize, const O: usize, const S: usize, const L: usize, E>(
    options: Options,
) -> Result<(), Error<E>> {
    crate::body_bound::<I, O, S>()?;
    if I == 0
        || options.queue_capacity.get() > 1024
        || std::mem::size_of::<runtime::RuntimeState<I, S, L>>() > crate::MAX_CATALOG_MEMORY_BYTES
    {
        return Err(Error::InvalidOptions);
    }
    options.budget.application_capacity()?;
    Ok(())
}

/// Run the same identity owner on a caller's lexical worker stack. A driver may
/// borrow already-opened domain databases/managers; no self-reference, second
/// executor, or shared authority mutex is needed. Create the worker with the
/// same queue capacity, and `Client::from_lane(worker.client(),worker.control_client(), O, S)` after readiness.
/// Keep domain resources and the store alive until this call finishes; the outer
/// initializer must drop all of them before its worker completion is published.
/// Factory construction may retain the weak waker but may not start live effects.
pub fn run<const I: usize, const O: usize, const S: usize, const L: usize, D, J>(
    inbox: prns_storage::Inbox<Job<J>>,
    store: &mut IdentityStore<I, O, S>,
    options: Options,
    diagnostics: DiagnosticObserver<<D as runtime::ScopedDriver<S>>::Error>,
    factory: impl FnOnce(Waker) -> Result<D, <D as runtime::ScopedDriver<S>>::Error>,
) -> Result<(), Error<<D as runtime::ScopedDriver<S>>::Error>>
where
    D: CommandHandler<S, J>,
    J: Send + 'static,
    <D as runtime::ScopedDriver<S>>::Error: Send + Sync + 'static,
{
    validate::<I, O, S, L, _>(options)?;
    run_validated::<I, O, S, L, D, J>(inbox, store, options, diagnostics, factory)
}

// Keep capacity-dependent native frames behind validation, including callers
// using the borrowed runner directly rather than the standalone constructor.
#[inline(never)]
fn run_validated<const I: usize, const O: usize, const S: usize, const L: usize, D, J>(
    inbox: prns_storage::Inbox<Job<J>>,
    store: &mut IdentityStore<I, O, S>,
    options: Options,
    diagnostics: DiagnosticObserver<<D as runtime::ScopedDriver<S>>::Error>,
    factory: impl FnOnce(Waker) -> Result<D, <D as runtime::ScopedDriver<S>>::Error>,
) -> Result<(), Error<<D as runtime::ScopedDriver<S>>::Error>>
where
    D: CommandHandler<S, J>,
    J: Send + 'static,
    <D as runtime::ScopedDriver<S>>::Error: Send + Sync + 'static,
{
    let mut current = Box::new(core::Catalog::new([0; 16]));
    let mut staging = Box::new(core::Catalog::new([0; 16]));
    let mut coordinator = core::Coordinator::open(store, &mut current, &mut staging)?;
    let mut incarnation = [0; 16];
    getrandom::fill(&mut incarnation).map_err(crate::Error::Entropy)?;
    let mut state = runtime::RuntimeState::<I, S, L>::new(incarnation, options.budget)?;
    let wake = inbox.progress_waker();
    let closed = inbox.close_signal();
    let mut driver = factory(wake.clone()).map_err(Error::Driver)?;
    let mut engine = engine::Engine::new(
        store,
        &mut coordinator,
        &mut state,
        &mut driver,
        wake,
        diagnostics.inner,
        closed,
    );
    inbox.run_owned(|event, context| {
        match event {
            OwnerEvent::Closing => engine.terminate(),
            OwnerEvent::Job(job) => engine.handle(job),
            OwnerEvent::Progress => {}
        }
        engine.poll_recovery(context);
        engine.poll_observations(context);
        engine.progress();
        engine.poll_suspension();
        if engine.drained() {
            OwnerProgress::Drained
        } else {
            OwnerProgress::Pending
        }
    });
    drop(engine);
    drop(driver);
    Ok(())
}
