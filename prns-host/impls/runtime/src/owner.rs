//! Process-owned native sessions with nonblocking foreign leases.
//!
//! Creating and joining native hosts is serialized on one lifecycle worker.
//! Commands and snapshots use the host's own runtime and never occupy that worker.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};

use prns_host::{
    CommandFailure, CommandOutcome, DestinationHash, HostCommand, HostConfig, HostSnapshot,
    IdentityHash, LinkId, ResourceCompression,
};
use tokio::sync::{oneshot, watch};

use crate::session::{HostSessionEvents, HostSessionObserver};

#[path = "owner/remote_control.generated.rs"]
mod remote_control;
use crate::{HostRuntime, HostSnapshotError, HostStartError, HostStopError, HostSubmitError};

const MAX_LIVE_SESSIONS: usize = 32;
static LIVE_SESSIONS: AtomicUsize = AtomicUsize::new(0);
type LifecycleJob = Box<dyn FnOnce() + Send>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionError {
    Busy,
    Stopped,
    Start(HostStartError),
    Stop(HostStopError),
    LifecycleUnavailable,
    /// A scoped borrow cannot export unrestricted protocol/service capabilities.
    RestrictedCapability,
    Snapshot(HostSnapshotError),
}

/// The sole stop authority. Keep this owner in native composition for a retained host.
pub struct OwnedSession {
    shared: Arc<SessionState>,
}

/// A borrowed command/query client. Releasing it cannot stop a retained owner.
#[derive(Clone)]
pub struct HostClient {
    shared: Arc<SessionState>,
    admission: Option<Arc<Mutex<bool>>>,
}

/// Revocation authority for one family of borrowed clients. Fencing or dropping
/// the scope denies new mutations without stopping the host or accepted work.
/// Keep an unrestricted `OwnedSession::client` for internal cleanup operations.
pub struct HostClientScope {
    client: HostClient,
}

impl HostClientScope {
    #[must_use]
    pub fn client(&self) -> HostClient {
        self.client.clone()
    }

    pub fn fence(&self) {
        if let Some(closed) = &self.client.admission {
            *crate::lock(closed) = true;
        }
    }
}

impl Drop for HostClientScope {
    fn drop(&mut self) {
        self.fence();
    }
}

/// Read-only completion of the owner's actual joined stop; waiting never requests stop.
#[derive(Clone)]
pub struct SessionStopped {
    completion: watch::Receiver<Option<Result<(), SessionError>>>,
}

impl SessionStopped {
    pub async fn wait(&self) -> Result<(), SessionError> {
        let mut completion = self.completion.clone();
        loop {
            let settled = completion.borrow().clone();
            if let Some(result) = settled {
                return result;
            }
            completion
                .changed()
                .await
                .map_err(|_| SessionError::LifecycleUnavailable)?;
        }
    }
}

struct SessionState {
    running: Mutex<Option<RunningHost>>,
    events: HostSessionEvents,
    lifecycle: mpsc::Sender<LifecycleJob>,
    stopped: watch::Sender<Option<Result<(), SessionError>>>,
    application_events: crate::ApplicationEventDispatch,
}

/// Every admitted startup reserves one slot through completion of shutdown.
struct SessionSlot;

impl SessionSlot {
    fn acquire() -> Result<Self, SessionError> {
        Self::reserve(&LIVE_SESSIONS)?;
        Ok(Self)
    }

    fn reserve(live_sessions: &AtomicUsize) -> Result<(), SessionError> {
        let mut count = live_sessions.load(Ordering::Acquire);
        loop {
            if count >= MAX_LIVE_SESSIONS {
                return Err(SessionError::Busy);
            }
            match live_sessions.compare_exchange_weak(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(current) => count = current,
            }
        }
    }
}

impl Drop for SessionSlot {
    fn drop(&mut self) {
        LIVE_SESSIONS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct RunningHost {
    host: Arc<HostRuntime>,
    slot: SessionSlot,
    _embedding_consumer: Option<crate::HostRuntimeEventStream>,
}

fn lifecycle_executor() -> Result<mpsc::Sender<LifecycleJob>, SessionError> {
    static EXECUTOR: OnceLock<Result<mpsc::Sender<LifecycleJob>, SessionError>> = OnceLock::new();
    EXECUTOR
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<LifecycleJob>();
            std::thread::Builder::new()
                .name("prns-host-lifecycle".into())
                .spawn(move || {
                    for job in receiver {
                        // A malformed embedding must not strand all other owners.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    }
                })
                .map_err(|_| SessionError::LifecycleUnavailable)?;
            Ok(sender)
        })
        .clone()
}

impl OwnedSession {
    /// Admission is bounded even if the foreign future is cancelled during startup.
    pub async fn open(config: HostConfig) -> Result<Arc<Self>, SessionError> {
        Self::open_with_embedding(config, crate::HostEmbedding::default()).await
    }

    pub async fn open_with_embedding(
        config: HostConfig,
        embedding: crate::HostEmbedding,
    ) -> Result<Arc<Self>, SessionError> {
        let slot = SessionSlot::acquire()?;
        let lifecycle = lifecycle_executor()?;
        let worker = lifecycle.clone();
        let (reply, result) = oneshot::channel();
        lifecycle
            .send(Box::new(move || {
                let events = HostSessionEvents::new(config.limits, false);
                let application_events = embedding.application_events;
                let embedding_consumer = if embedding.application_events
                    == crate::ApplicationEventDispatch::EmbeddingCallback
                {
                    match events.claim_stream(prns_host::ConsumerLane::ApplicationEvents) {
                        Ok(stream) => Some(stream),
                        Err(_) => {
                            let _ = reply.send(Err(SessionError::Busy));
                            return;
                        }
                    }
                } else {
                    None
                };
                let started =
                    HostRuntime::start_with_embedding(config, Arc::new(events.clone()), embedding)
                        .map_err(SessionError::Start)
                        .map(|host| {
                            let (stopped, _) = watch::channel(None);
                            Arc::new(Self {
                                shared: Arc::new(SessionState {
                                    running: Mutex::new(Some(RunningHost {
                                        host: Arc::new(host),
                                        slot,
                                        _embedding_consumer: embedding_consumer,
                                    })),
                                    events,
                                    lifecycle: worker,
                                    stopped,
                                    application_events,
                                }),
                            })
                        });
                // On cancelled startup, dropping the returned owner schedules shutdown here.
                let _ = reply.send(started);
            }))
            .map_err(|_| SessionError::LifecycleUnavailable)?;
        result
            .await
            .map_err(|_| SessionError::LifecycleUnavailable)?
    }

    #[must_use]
    pub fn client(&self) -> HostClient {
        HostClient {
            shared: Arc::clone(&self.shared),
            admission: None,
        }
    }

    /// Borrow clients with a separately owned admission fence. Revocation affects
    /// only this scope and its client clones; reads and accepted handles remain usable.
    #[must_use]
    pub fn client_scope(&self) -> HostClientScope {
        HostClientScope {
            client: HostClient {
                shared: Arc::clone(&self.shared),
                admission: Some(Arc::new(Mutex::new(false))),
            },
        }
    }

    /// Create an exclusive attachment claim, without transferring host stop authority.
    pub fn attachment_scope(
        &self,
    ) -> Result<crate::attachment_scope::AttachmentScope, SessionError> {
        Ok(crate::attachment_scope::AttachmentScope::new(
            self.shared.host()?,
            self.stopped(),
        ))
    }

    pub fn stopped(&self) -> SessionStopped {
        SessionStopped {
            completion: self.shared.stopped.subscribe(),
        }
    }

    pub async fn stop(&self) -> Result<(), SessionError> {
        let completion = self.stopped();
        self.shared.request_stop();
        completion.wait().await
    }
}

impl Drop for OwnedSession {
    fn drop(&mut self) {
        self.shared.request_stop();
    }
}

impl SessionState {
    fn host(&self) -> Result<Arc<HostRuntime>, SessionError> {
        crate::lock(&self.running)
            .as_ref()
            .map(|running| Arc::clone(&running.host))
            .ok_or(SessionError::Stopped)
    }

    fn request_stop(&self) {
        let Some(running) = crate::lock(&self.running).take() else {
            return;
        };
        self.events.request_stop();
        let completion = self.stopped.clone();
        let job: LifecycleJob = Box::new(move || {
            let RunningHost {
                host,
                slot,
                _embedding_consumer,
            } = running;
            let result = host.stop_result().map_err(SessionError::Stop);
            drop(host);
            drop(_embedding_consumer);
            drop(slot);
            completion.send_replace(Some(result));
        });
        if let Err(error) = self.lifecycle.send(job) {
            // The process executor is unavailable. Never join on a foreign finalizer.
            // Keep the unfinished native owner alive, and report failure to explicit waiters.
            std::mem::forget(error.0);
            self.stopped
                .send_replace(Some(Err(SessionError::LifecycleUnavailable)));
        }
    }
}

impl HostClient {
    // Only native-owned, non-callback admission closures use this helper. The
    // mutex covers the stopped check and actual bounded queue insertion. It is
    // released before awaiting, executing user work, or observing completion.
    pub(crate) fn admit<T>(
        &self,
        submit: impl FnOnce(&HostRuntime) -> Result<T, HostSubmitError>,
    ) -> Result<T, HostSubmitError> {
        let gate = self.admission.as_ref().map(|gate| crate::lock(gate));
        if gate.as_deref().is_some_and(|closed| *closed) {
            return Err(HostSubmitError::Stopped);
        }
        let host = self.shared.host().map_err(|_| HostSubmitError::Stopped)?;
        submit(&host)
    }

    async fn bounded_operation<T, F, Fut>(&self, operation: F) -> Result<T, SessionError>
    where
        T: Send + 'static,
        F: FnOnce(personal_rns::PrnsNodeHandle) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send + 'static,
    {
        let reply = self
            .admit(|host| host.begin_preview_operation(operation))
            .map_err(submit_error)?;
        reply.await.map_err(|_| SessionError::Stopped)
    }

    /// Immutable ownership mode chosen before host startup. This observes the
    /// existing event dispatch contract; it cannot claim or replace a callback.
    pub fn application_event_dispatch(&self) -> crate::ApplicationEventDispatch {
        self.shared.application_events
    }

    pub async fn execute(
        &self,
        command: HostCommand,
    ) -> Result<Result<CommandOutcome, CommandFailure>, SessionError> {
        let command = self
            .admit(|host| host.submit(command))
            .map_err(submit_error)?;
        match command.wait_async().await {
            crate::CommandWait::Completed(result) => Ok(result),
            crate::CommandWait::Interrupted | crate::CommandWait::TimedOut => {
                Err(SessionError::Stopped)
            }
        }
    }

    pub async fn snapshot(&self) -> Result<HostSnapshot, SessionError> {
        self.shared
            .host()?
            .snapshot_async()
            .await
            .map_err(SessionError::Snapshot)
    }

    /// Rust-only physical and logical inspection from the same host capture.
    pub async fn inspection(&self) -> Result<crate::HostInspectionSnapshot, SessionError> {
        self.shared
            .host()?
            .inspection_async()
            .await
            .map_err(SessionError::Snapshot)
    }

    #[must_use]
    pub fn events(&self) -> HostSessionObserver {
        self.shared.events.observer()
    }

    /// Unrestricted native capability, unavailable on a revocable client because
    /// its raw protocol handle could bypass the admission fence after export.
    pub fn service_client(&self) -> Result<crate::HostServiceClient, SessionError> {
        if self.admission.is_some() {
            return Err(SessionError::RestrictedCapability);
        }
        self.shared.host()?.service_client().map_err(submit_error)
    }

    /// Reuse an existing public Rust protocol API on the bounded host runtime.
    /// Accepted operations survive waiter cancellation and are accounted for by
    /// joined shutdown. Unavailable on revocable clients because an arbitrary
    /// callback could retain the raw protocol handle beyond its accepted operation.
    pub async fn protocol_operation<T, F, Fut>(&self, operation: F) -> Result<T, SessionError>
    where
        T: Send + 'static,
        F: FnOnce(personal_rns::PrnsNodeHandle) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send + 'static,
    {
        if self.admission.is_some() {
            return Err(SessionError::RestrictedCapability);
        }
        self.bounded_operation(operation).await
    }

    pub fn identity_hash(&self) -> Result<IdentityHash, SessionError> {
        Ok(self.shared.host()?.identity_hash())
    }

    pub fn destination_hashes(&self) -> Result<Vec<DestinationHash>, SessionError> {
        Ok(self.shared.host()?.destination_hashes().to_vec())
    }

    /// Public identity learned from an authenticated destination announcement.
    /// Never returns the local signing secret or an unverified announcement key.
    pub async fn destination_public_key(
        &self,
        destination: DestinationHash,
    ) -> Result<Option<[u8; 64]>, SessionError> {
        use personal_rns::node_introspection::DestinationIdentityQuery;
        self.shared
            .host()?
            .on_preview_runtime(move |handle| async move {
                handle
                    .destination_identity(DestinationIdentityQuery::Destination(
                        personal_rns::wire::DestinationHash::new(destination.into_bytes()),
                    ))
                    .await
                    .map(|snapshot| *snapshot.public.as_bytes())
            })
            .await
            .map_err(submit_error)
    }

    pub async fn remote_control_exchange(
        &self,
        link_id: LinkId,
        request: personal_rns::remote_control::RemoteControlRequest,
    ) -> Result<
        (
            personal_rns::remote_control::RemoteControlResponse,
            personal_rns::units::RttMillis,
        ),
        crate::HostRemoteControlError,
    > {
        self.admit(|host| {
            host.begin_remote_control_exchange(
                personal_rns::routing::links::LinkId::new(link_id.into_bytes()),
                request,
            )
        })
        .map_err(crate::HostRemoteControlError::Admission)?
        .await
        .map_err(|_| crate::HostRemoteControlError::Admission(HostSubmitError::Stopped))?
    }

    pub async fn remote_control_target_exchange(
        &self,
        target: IdentityHash,
        request: personal_rns::remote_control::RemoteControlRequest,
    ) -> Result<
        (
            personal_rns::remote_control::RemoteControlResponse,
            personal_rns::units::RttMillis,
        ),
        crate::HostRemoteControlError,
    > {
        self.admit(|host| {
            host.begin_remote_control_target_exchange(
                personal_rns::identity::IdentityHash::new(target.into_bytes()),
                request,
            )
        })
        .map_err(crate::HostRemoteControlError::Admission)?
        .await
        .map_err(|_| crate::HostRemoteControlError::Admission(HostSubmitError::Stopped))?
    }

    pub fn begin_resource_upload(
        &self,
        link_id: LinkId,
        declared_length: u64,
        metadata: Option<Vec<u8>>,
        compression: ResourceCompression,
    ) -> Result<crate::HostRuntimeUpload, SessionError> {
        self.admit(|host| {
            host.begin_resource_upload(link_id, declared_length, metadata, compression)
        })
        .map_err(submit_error)
    }
}

fn submit_error(error: HostSubmitError) -> SessionError {
    match error {
        HostSubmitError::Busy => SessionError::Busy,
        HostSubmitError::Stopped => SessionError::Stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_host::{
        ConsumerLane, HostRole, IdentityConfig, LifecycleState, PersistenceConfig, PrnsLimits,
    };
    use std::future::Future;
    use std::task::Poll;

    fn config() -> HostConfig {
        HostConfig {
            identity: IdentityConfig::GenerateEphemeral,
            persistence: PersistenceConfig::Ephemeral,
            role: HostRole::Endpoint,
            destinations: Vec::new(),
            required_capabilities: Vec::new(),
            limits: PrnsLimits::balanced(),
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
    }

    #[test]
    fn concurrent_admission_stays_bounded_and_reuses_released_capacity() {
        let live_sessions = AtomicUsize::new(0);
        let admitted = AtomicUsize::new(0);
        let ready = std::sync::Barrier::new(8);
        std::thread::scope(|threads| {
            for _ in 0..8 {
                threads.spawn(|| {
                    ready.wait();
                    while SessionSlot::reserve(&live_sessions).is_ok() {
                        admitted.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(admitted.load(Ordering::Relaxed), MAX_LIVE_SESSIONS);
        assert_eq!(live_sessions.load(Ordering::Acquire), MAX_LIVE_SESSIONS);
        assert_eq!(
            SessionSlot::reserve(&live_sessions),
            Err(SessionError::Busy)
        );
        live_sessions.fetch_sub(1, Ordering::AcqRel);
        assert_eq!(SessionSlot::reserve(&live_sessions), Ok(()));
        assert_eq!(
            SessionSlot::reserve(&live_sessions),
            Err(SessionError::Busy)
        );
    }

    fn attach_command() -> HostCommand {
        HostCommand::AttachTcpServer {
            bind: "127.0.0.1:0".into(),
            bitrate: prns_host::Bitrate::Auto,
        }
    }

    #[test]
    fn scoped_fence_preserves_reads_and_accepted_handles_but_denies_raw_capabilities() {
        runtime().block_on(async {
            let owner = OwnedSession::open(config()).await.expect("start");
            let scope = owner.client_scope();
            let client = scope.client();
            assert!(matches!(
                client.service_client(),
                Err(SessionError::RestrictedCapability)
            ));
            assert_eq!(
                client
                    .protocol_operation(|handle| async move {
                        drop(handle);
                    })
                    .await,
                Err(SessionError::RestrictedCapability)
            );
            // Existing generated protocol methods still work through typed admission.
            assert!(client.close_remote_control_pairing().await.is_ok());
            let stream = client
                .events()
                .claim_stream(ConsumerLane::Diagnostics)
                .expect("stream");
            let upload = client
                .begin_resource_upload(LinkId::new([1; 16]), 0, None, ResourceCompression::Never)
                .expect("admitted upload");
            scope.fence();
            assert_eq!(
                client.execute(attach_command()).await,
                Err(SessionError::Stopped)
            );
            assert_eq!(
                client.close_remote_control_pairing().await,
                Err(SessionError::Stopped)
            );
            assert!(matches!(
                client.begin_resource_upload(
                    LinkId::new([1; 16]),
                    0,
                    None,
                    ResourceCompression::Never
                ),
                Err(SessionError::Stopped)
            ));
            assert!(client.snapshot().await.is_ok());
            assert_eq!(
                client
                    .destination_public_key(DestinationHash::new([2; 16]))
                    .await,
                Ok(None)
            );
            assert_eq!(client.events().lifecycle().state, LifecycleState::Running);
            // This handle was admitted before the fence. Its normal settlement
            // remains available, even though this particular Link does not exist.
            assert!(matches!(
                upload.finish().wait_async().await,
                crate::CommandWait::Completed(_)
            ));
            assert!(owner.client().service_client().is_ok());
            assert!(matches!(
                owner.client().execute(attach_command()).await,
                Ok(Ok(CommandOutcome::InterfaceAttached { .. }))
            ));
            drop(scope);
            assert_eq!(
                client.execute(attach_command()).await,
                Err(SessionError::Stopped)
            );
            owner.stop().await.expect("stop");
            // The event lease observes the actual host stop, not scope revocation.
            while stream.try_next().is_ok() {}
            assert!(matches!(
                stream.ready().await,
                Err(crate::HostRuntimeStreamError::Stopped)
            ));
        });
    }

    #[test]
    fn fence_linearizes_with_the_actual_queue_insertion() {
        runtime().block_on(async {
            let owner = OwnedSession::open(config()).await.expect("start");
            let scope = Arc::new(owner.client_scope());
            let client = scope.client();
            let (entered_tx, entered_rx) = mpsc::sync_channel(1);
            let (release_tx, release_rx) = mpsc::sync_channel(1);
            let (fenced_tx, fenced_rx) = mpsc::sync_channel(1);
            let admitted = client.clone();
            let task = std::thread::spawn(move || {
                admitted.admit(|host| {
                    entered_tx.send(()).expect("entered");
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .expect("release");
                    host.submit(attach_command())
                })
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("admitting");
            let closing = scope.clone();
            let fence = std::thread::spawn(move || {
                closing.fence();
                fenced_tx.send(()).expect("fenced");
            });
            assert!(matches!(
                fenced_rx.recv_timeout(std::time::Duration::from_millis(20)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            release_tx.send(()).expect("release");
            let accepted = task.join().expect("submit task").expect("admission");
            fence.join().expect("fence task");
            fenced_rx.recv().expect("fenced");
            assert!(matches!(
                accepted.wait_async().await,
                crate::CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { .. }))
            ));
            assert_eq!(
                client.execute(attach_command()).await,
                Err(SessionError::Stopped)
            );
            assert_eq!(
                client.snapshot().await.expect("snapshot").interfaces.len(),
                1
            );
            owner.stop().await.expect("stop");
        });
    }

    #[test]
    fn accepted_native_task_outlives_waiter_and_scope_without_holding_admission_lock() {
        runtime().block_on(async {
            let owner = OwnedSession::open(config()).await.expect("start");
            let scope = owner.client_scope();
            let client = scope.client();
            let (release, released) = oneshot::channel();
            let (completed, completion) = oneshot::channel();
            let mut wait = Box::pin(client.bounded_operation(move |handle| async move {
                let _ = released.await;
                // The already accepted operation may still use its native
                // capability; scope fencing does not invalidate continuations.
                drop(handle);
                let _ = completed.send(());
            }));
            assert!(matches!(
                std::future::poll_fn(|cx| Poll::Ready(wait.as_mut().poll(cx))).await,
                Poll::Pending
            ));
            drop(wait);
            drop(scope);
            assert_eq!(
                client.execute(attach_command()).await,
                Err(SessionError::Stopped)
            );
            release.send(()).expect("release");
            tokio::time::timeout(std::time::Duration::from_secs(5), completion)
                .await
                .expect("deadline")
                .expect("accepted task finished");
            owner.stop().await.expect("stop");
        });
    }

    #[test]
    fn remote_control_admission_is_fenced_before_any_protocol_work() {
        runtime().block_on(async {
            let owner = OwnedSession::open(config()).await.expect("start");
            let scope = owner.client_scope();
            let client = scope.client();
            scope.fence();
            let stopped = Err(crate::HostRemoteControlError::Admission(
                HostSubmitError::Stopped,
            ));
            assert_eq!(
                client
                    .remote_control_exchange(
                        LinkId::new([1; 16]),
                        personal_rns::remote_control::RemoteControlRequest::Describe
                    )
                    .await,
                stopped
            );
            assert_eq!(
                client
                    .remote_control_target_exchange(
                        IdentityHash::new([1; 16]),
                        personal_rns::remote_control::RemoteControlRequest::Describe
                    )
                    .await,
                stopped
            );
            owner.stop().await.expect("stop");
        });
    }

    #[test]
    fn borrowed_client_does_not_control_owned_session() {
        runtime().block_on(async {
            let owner = OwnedSession::open(config()).await.expect("start");
            let client = owner.client();
            drop(client.clone());
            assert!(client.snapshot().await.is_ok());
            owner.stop().await.expect("stop");
            assert!(matches!(
                client.snapshot().await,
                Err(SessionError::Stopped)
            ));
            owner.stop().await.expect("repeated stop");
            assert!(matches!(
                client.events().lifecycle().state,
                LifecycleState::Stopped(_)
            ));
        });
    }

    #[test]
    fn all_stop_waiters_observe_join_completion() {
        runtime().block_on(async {
            let owner = OwnedSession::open(config()).await.expect("start");
            let second = Arc::clone(&owner);
            let first = tokio::spawn(async move { second.stop().await });
            owner.stop().await.expect("owner stop");
            first.await.expect("stop task").expect("second stop");
            assert!(owner.client().events().lifecycle().state.is_terminal());
        });
    }

    #[test]
    fn dropping_owner_releases_stop_authority_without_joining_caller() {
        runtime().block_on(async {
            let owner = OwnedSession::open(config()).await.expect("start");
            let client = owner.client();
            let events = client.events();
            let mut completion = client.shared.stopped.subscribe();
            let stream = events
                .claim_stream(ConsumerLane::ApplicationEvents)
                .expect("claim");
            drop(owner);
            assert!(matches!(client.identity_hash(), Err(SessionError::Stopped)));
            while completion.borrow().is_none() {
                completion.changed().await.expect("stop completion");
            }
            assert!(completion.borrow().as_ref().expect("completed").is_ok());
            assert!(matches!(
                stream.ready().await,
                Err(crate::session::HostRuntimeStreamError::Stopped)
            ));
        });
    }
}

#[cfg(all(test, unix))]
#[path = "owner/pairing_discovery_tests.rs"]
mod pairing_discovery_tests;
