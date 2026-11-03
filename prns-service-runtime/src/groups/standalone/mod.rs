//! One concrete owner for a host, a service executor, and the existing identity
//! and group lane. Startup and cleanup outlive their result observers.
mod error;
mod lifecycle;
mod network_state;
mod options;
pub use error::{AttachmentFailure, Error, StoreFailure};
pub use network_state::{NetworkChange, NetworkPhase, NetworkStatus};
use network_state::{NetworkRequest, RequestKind, Startup};
pub use options::{CarrierOptions, Options, StartupRecovery};

use super::network;
pub use prns_host::HostConfig;
#[cfg(feature = "connections")]
use prns_host_runtime::connections::{BindingClient, HostBinding, ProfileOwner};
use prns_host_runtime::owner::{HostClient, HostClientScope, OwnedSession, SessionError};
pub use prns_host_runtime::HostEmbedding;
use prns_identities::owned;
use prns_storage::{Completion, OwnerEvent, OwnerProgress, StartError, Worker};
#[cfg(test)]
use std::task::Context;
use std::{
    convert::Infallible,
    future::Future,
    num::NonZeroUsize,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    task::Poll,
};
use tokio::sync::oneshot;
type Local = network::Owner<4, 32, 4, 16>;
#[cfg(test)]
type LocalError = crate::local::Error<network::DriverError>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "ServiceRuntimePhase"))]
pub enum Phase {
    Starting,
    Ready,
    Closing,
    Failed,
    Unresolved,
    Closed,
}
/// Latest retained identity diagnostic, which may precede a current lifecycle
/// phase. It does not independently prove a failed or completed shutdown.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "ServiceRuntimeIdentityDiagnostic")
)]
pub struct IdentityDiagnostic {
    pub identity: Option<prns_identities_core::IdentityReference>,
    pub error: prns_identities_core::ApiError,
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "ServiceRuntimeStatus"))]
pub struct Status {
    pub phase: Phase,
    pub failure: Option<Error>,
    /// Host join and service-executor/resource release have actually finished.
    pub cleanup_complete: bool,
    /// Historical/latest diagnostic, retained independently of phase or cleanup.
    pub latest_identity_diagnostic: Option<IdentityDiagnostic>,
}
struct State {
    closed: bool,
    startup: Option<Result<(), Error>>,
    status: Status,
    local: Option<Arc<Local>>,
    host: Option<HostClientScope>,
    #[cfg(feature = "connections")]
    connections: Option<Arc<HostBinding>>,
    progress: Option<std::task::Waker>,
    cleanup_retry: u64,
    network: NetworkStatus,
    request: Option<NetworkRequest>,
    operation: Option<NetworkChange>,
    diagnostics: Option<owned::DiagnosticObserver<network::DriverError>>,
    waiters: Vec<oneshot::Sender<Result<(), Error>>>,
}
struct Shared {
    state: Mutex<State>,
}
impl Shared {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                closed: false,
                startup: None,
                status: Status {
                    phase: Phase::Starting,
                    failure: None,
                    cleanup_complete: false,
                    latest_identity_diagnostic: None,
                },
                local: None,
                host: None,
                #[cfg(feature = "connections")]
                connections: None,
                progress: None,
                cleanup_retry: 0,
                network: NetworkStatus {
                    phase: NetworkPhase::Offline,
                    generation: 0,
                    failure: None,
                },
                request: None,
                operation: None,
                diagnostics: None,
                waiters: Vec::new(),
            }),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn change(&self, update: impl FnOnce(&mut State)) {
        {
            update(&mut self.lock());
        }
        self.notify_ready();
    }
    fn fence(&self) {
        let (local, connections) = {
            let mut state = self.lock();
            state.closed = true;
            if let Some(host) = &state.host {
                host.fence();
            }
            if !matches!(state.status.phase, Phase::Unresolved | Phase::Closed) {
                state.status.phase = Phase::Closing;
            }
            #[cfg(feature = "connections")]
            let connections = state.connections.clone();
            #[cfg(not(feature = "connections"))]
            let connections = ();
            (state.local.clone(), connections)
        };
        // Retain the interfaces until domain cleanup finishes, while denying
        // fresh profile reconciliation. Wakers run outside this owner's lock.
        #[cfg(feature = "connections")]
        if let Some(connections) = connections {
            connections.fence();
        }
        #[cfg(not(feature = "connections"))]
        let () = connections;
        // Local owner.close fences its queue and external dispatch synchronously.
        if let Some(local) = local {
            drop(local.close());
        }
        self.wake();
        self.notify_ready();
    }
    fn wake(&self) {
        let wake = self.lock().progress.clone();
        if let Some(wake) = wake {
            wake.wake();
        }
    }
    fn notify_ready(&self) {
        let (waiters, result) = {
            let mut state = self.lock();
            let result = if state.closed {
                Err(Error::Closing)
            } else if let Some(result) = &state.startup {
                result.clone()
            } else {
                return;
            };
            (std::mem::take(&mut state.waiters), result)
        };
        // A foreign executor callback can reenter or panic. Never invoke or drop
        // it while holding state, and never strand the remaining observers.
        for waiter in waiters {
            let value = result.clone();
            let _ = catch_unwind(AssertUnwindSafe(|| waiter.send(value)));
        }
    }
    async fn ready(&self) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        let retired = {
            let mut state = self.lock();
            if state.closed {
                return Err(Error::Closing);
            }
            if let Some(result) = &state.startup {
                return result.clone();
            }
            let (live, retired): (Vec<_>, Vec<_>) = std::mem::take(&mut state.waiters)
                .into_iter()
                .partition(|sender| !sender.is_closed());
            state.waiters = live;
            if state.waiters.len() == 32 {
                drop(state);
                drop(retired);
                return Err(Error::ObserverCapacity);
            }
            state.waiters.push(sender);
            retired
        };
        // A cancelled readiness future closes its channel. Reclaim those bounded
        // slots at the next registration; destruction stays outside state locks.
        drop(retired);
        receiver.await.unwrap_or(Err(Error::LifecyclePanicked))
    }
    fn fail(&self, error: Error, unresolved: bool) {
        self.change(|state| {
            if state.startup.is_none() {
                state.startup = Some(Err(error.clone()));
            }
            state.status.failure = Some(error);
            state.status.phase = if unresolved {
                Phase::Unresolved
            } else {
                Phase::Failed
            };
        });
    }
}

/// Sole ownership of construction and shutdown. Ordinary domain and host clients
/// remain borrowed; releasing them cannot stop this owner.
pub struct Owner {
    worker: Worker<Infallible, Error>,
    shared: Arc<Shared>,
}
impl Owner {
    pub fn create_new(
        path: impl AsRef<Path>,
        host: prns_host::HostConfig,
        options: Options,
    ) -> Result<Self, Error> {
        Self::create_new_with_embedding(path, host, options, HostEmbedding::default())
    }
    pub fn open_existing(
        path: impl AsRef<Path>,
        host: prns_host::HostConfig,
        options: Options,
    ) -> Result<Self, Error> {
        Self::open_existing_with_embedding(path, host, options, HostEmbedding::default())
    }
    pub fn create_new_with_embedding(
        path: impl AsRef<Path>,
        host: prns_host::HostConfig,
        options: Options,
        embedding: HostEmbedding,
    ) -> Result<Self, Error> {
        Self::start(
            path.as_ref(),
            true,
            Some(Startup { host, embedding }),
            options,
            #[cfg(feature = "connections")]
            None,
        )
    }
    pub fn open_existing_with_embedding(
        path: impl AsRef<Path>,
        host: prns_host::HostConfig,
        options: Options,
        embedding: HostEmbedding,
    ) -> Result<Self, Error> {
        Self::start(
            path.as_ref(),
            false,
            Some(Startup { host, embedding }),
            options,
            #[cfg(feature = "connections")]
            None,
        )
    }
    /// Compose a borrowed profile store with this host generation. The caller
    /// owns that store independently; its ordinary client remains usable after
    /// this owner stops. Network readiness acknowledges binding, not peer connectivity.
    #[cfg(feature = "connections")]
    pub fn create_new_with_connections(
        path: impl AsRef<Path>,
        host: prns_host::HostConfig,
        options: Options,
        embedding: HostEmbedding,
        profiles: Arc<ProfileOwner>,
    ) -> Result<Self, Error> {
        Self::start(
            path.as_ref(),
            true,
            Some(Startup { host, embedding }),
            options,
            Some(profiles),
        )
    }
    #[cfg(feature = "connections")]
    pub fn open_existing_with_connections(
        path: impl AsRef<Path>,
        host: prns_host::HostConfig,
        options: Options,
        embedding: HostEmbedding,
        profiles: Arc<ProfileOwner>,
    ) -> Result<Self, Error> {
        Self::start(
            path.as_ref(),
            false,
            Some(Startup { host, embedding }),
            options,
            Some(profiles),
        )
    }
    /// Open local data without constructing a host, interfaces or recovery runs.
    /// Networking is admitted separately with an explicit fresh HostConfig.
    pub fn create_new_offline(path: impl AsRef<Path>, options: Options) -> Result<Self, Error> {
        Self::start(
            path.as_ref(),
            true,
            None,
            options,
            #[cfg(feature = "connections")]
            None,
        )
    }
    pub fn open_existing_offline(path: impl AsRef<Path>, options: Options) -> Result<Self, Error> {
        Self::start(
            path.as_ref(),
            false,
            None,
            options,
            #[cfg(feature = "connections")]
            None,
        )
    }
    #[cfg(feature = "connections")]
    pub fn create_new_offline_with_connections(
        path: impl AsRef<Path>,
        options: Options,
        profiles: Arc<ProfileOwner>,
    ) -> Result<Self, Error> {
        Self::start(path.as_ref(), true, None, options, Some(profiles))
    }
    #[cfg(feature = "connections")]
    pub fn open_existing_offline_with_connections(
        path: impl AsRef<Path>,
        options: Options,
        profiles: Arc<ProfileOwner>,
    ) -> Result<Self, Error> {
        Self::start(path.as_ref(), false, None, options, Some(profiles))
    }
    fn start(
        path: &Path,
        create: bool,
        startup: Option<Startup>,
        options: Options,
        #[cfg(feature = "connections")] profiles: Option<Arc<ProfileOwner>>,
    ) -> Result<Self, Error> {
        let startup_recovery = options.startup_recovery;
        let options = options.checked()?;
        if startup.as_ref().is_some_and(|startup| {
            startup.embedding.application_events
                == prns_host_runtime::ApplicationEventDispatch::EmbeddingCallback
                && startup.embedding.on_event.is_none()
        }) {
            return Err(Error::InvalidOptions);
        }
        let path = std::path::absolute(path).map_err(|e| Error::Io {
            code: e.raw_os_error(),
        })?;
        let shared = Arc::new(Shared::new());
        if let Some(startup) = startup {
            shared.request_network(RequestKind::Start(Box::new(startup)))?;
        }
        let state = shared.clone();

        let worker = Worker::start("prns-service-runtime", NonZeroUsize::MIN, move |inbox| {
            state.lock().progress = Some(inbox.progress_waker());
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .max_blocking_threads(1)
                .enable_all()
                .build()
                .map_err(|e| Error::Io {
                    code: e.raw_os_error(),
                })?;
            let running = Box::pin(lifecycle::run(
                state.clone(),
                path,
                create,
                options,
                startup_recovery,
                runtime.handle().clone(),
                #[cfg(feature = "connections")]
                profiles,
            ));
            let mut resources = Some((runtime, running));
            let mut outcome = None;
            let result = catch_unwind(AssertUnwindSafe(|| {
                inbox.run_owned(|event, context| {
                    match event {
                        OwnerEvent::Job(never) => match never {},
                        OwnerEvent::Closing => state.fence(),
                        OwnerEvent::Progress => {}
                    }
                    if let Some((runtime, running)) = resources.as_mut() {
                        let polled = {
                            let _entered = runtime.enter();
                            running.as_mut().poll(context)
                        };
                        if let Poll::Ready(result) = polled {
                            if let Some((runtime, running)) = resources.take() {
                                drop(running);
                                drop(runtime);
                            }
                            state.finish_cleanup(&result);
                            outcome = Some(result);
                        }
                    }
                    if outcome.is_some() {
                        OwnerProgress::Drained
                    } else {
                        OwnerProgress::Pending
                    }
                })
            }));
            if result.is_err() {
                // A panic cannot prove service drainage. Keep the executor,
                // host and any native effects alive, with inspectable failure.
                state.fail(Error::LifecyclePanicked, true);
                loop {
                    std::thread::park();
                }
            }
            outcome.unwrap_or(Err(Error::LifecyclePanicked))
        })
        .map_err(|error| match error {
            StartError::Initialization(error) => error,
            StartError::Spawn(error) => Error::Io {
                code: error.raw_os_error(),
            },
            StartError::NotReady | StartError::Panicked => Error::LifecyclePanicked,
        })?;
        Ok(Self { worker, shared })
    }
    /// Losing a readiness waiter does not cancel startup. Close/finalization is
    /// the only shutdown authority, including while construction is in progress.
    /// Readiness concerns local data only. Observe network_status separately;
    /// network failure or suspension never replaces this owner.
    pub async fn ready(&self) -> Result<(), Error> {
        self.shared.ready().await
    }
    pub fn status(&self) -> Status {
        let (mut status, diagnostics) = {
            let state = self.shared.lock();
            (state.status.clone(), state.diagnostics.clone())
        };
        status.latest_identity_diagnostic =
            diagnostics
                .and_then(|observer| observer.latest())
                .map(|diagnostic| IdentityDiagnostic {
                    identity: diagnostic.identity,
                    error: diagnostic
                        .error
                        .api_error(network::DriverError::service_failure),
                });
        status
    }
    /// The same bounded rich-error observer used by the actual identity owner.
    /// It remains available during and after close; reading it grants no work.
    pub fn diagnostics(&self) -> Option<owned::DiagnosticObserver<network::DriverError>> {
        self.shared.lock().diagnostics.clone()
    }
    fn local(&self) -> Result<Arc<Local>, Error> {
        let state = self.shared.lock();
        if state.closed {
            return Err(Error::Closing);
        }
        if let Some(Err(error)) = &state.startup {
            return Err(error.clone());
        }
        if state.status.phase != Phase::Ready {
            return Err(Error::NotReady);
        }
        state.local.clone().ok_or(Error::NotReady)
    }
    pub fn identities(&self) -> Result<owned::Client, Error> {
        Ok(self.local()?.identities())
    }
    /// Result's canonical identity error also preserves checked foreign lifting.
    pub fn groups(
        &self,
        reference: prns_identities_core::IdentityReference,
    ) -> Result<prns_groups::Client, prns_identities_core::ApiError> {
        let local = self.local().map_err(|error| match error {
            Error::NotReady => prns_identities_core::ApiError::Runtime {
                reason: prns_identities_core::RuntimeFailure::Busy,
            },
            Error::Closing => prns_identities_core::ApiError::Closing,
            _ => prns_identities_core::ApiError::WorkerStopped,
        })?;
        Ok(super::client(
            local.submission(),
            reference,
            network::Command::Group,
        ))
    }
    pub fn host(&self) -> Result<HostClient, Error> {
        self.local()?;
        if self.network_status().phase != NetworkPhase::Ready {
            return Err(Error::NotReady);
        }
        self.shared
            .lock()
            .host
            .as_ref()
            .map(HostClientScope::client)
            .ok_or(Error::NotReady)
    }
    /// Borrow only observation and retry capabilities, never the binding's
    /// shutdown authority. Previously returned clients observe the close fence.
    #[cfg(feature = "connections")]
    pub fn connection_binding(&self) -> Result<BindingClient, Error> {
        self.local()?;
        if self.network_status().phase != NetworkPhase::Ready {
            return Err(Error::NotReady);
        }
        self.shared
            .lock()
            .connections
            .as_ref()
            .map(|binding| binding.client())
            .ok_or(Error::ConnectionsUnavailable)
    }
    pub fn close(&self) -> Completion<Error> {
        {
            let mut state = self.shared.lock();
            if state.status.phase == Phase::Unresolved {
                state.cleanup_retry = state.cleanup_retry.wrapping_add(1);
                state.status.phase = Phase::Closing;
            }
        }
        self.shared.wake();
        self.shared.fence();
        self.worker.request_close();
        self.closed()
    }
    /// Observe final resource release without initiating shutdown.
    pub fn closed(&self) -> Completion<Error> {
        self.worker.closed()
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.fence();
        self.worker.request_close();
    }
}

/// This construction pass uses only the ordinary catalog API. Exact references
/// are captured once; current authority and effects remain the engine's concern.
async fn recover_current(catalog: owned::Client) -> Result<(), prns_identities_core::ApiError> {
    for record in catalog.identities().await? {
        if record.desired == prns_identities_core::DesiredState::Enabled
            || record.pending.is_some()
            || record.recovery_cleanup.is_some()
        {
            catalog.identity(record.reference())?.recover().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
