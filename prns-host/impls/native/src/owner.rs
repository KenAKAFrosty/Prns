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

use crate::session::NativeSessionEvents;
use crate::{
    NativeHost, NativeSnapshotError, NativeStartError, NativeStopError, NativeSubmitError,
};

const MAX_LIVE_SESSIONS: usize = 32;
static LIVE_SESSIONS: AtomicUsize = AtomicUsize::new(0);
type LifecycleJob = Box<dyn FnOnce() + Send>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionError {
    Busy,
    Stopped,
    Start(NativeStartError),
    Stop(NativeStopError),
    LifecycleUnavailable,
    Snapshot(NativeSnapshotError),
}

/// The sole stop authority. Keep this owner in native composition for a retained host.
pub struct OwnedSession {
    shared: Arc<SessionState>,
}

/// A borrowed command/query client. Releasing it cannot stop a retained owner.
#[derive(Clone)]
pub struct HostClient {
    shared: Arc<SessionState>,
}

struct SessionState {
    running: Mutex<Option<RunningHost>>,
    events: NativeSessionEvents,
    lifecycle: mpsc::Sender<LifecycleJob>,
    stopped: watch::Sender<Option<Result<(), SessionError>>>,
}

/// Every admitted startup reserves one slot through completion of shutdown.
struct SessionSlot;

impl SessionSlot {
    fn acquire() -> Result<Self, SessionError> {
        LIVE_SESSIONS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_LIVE_SESSIONS).then_some(count + 1)
            })
            .map(|_| Self)
            .map_err(|_| SessionError::Busy)
    }
}

impl Drop for SessionSlot {
    fn drop(&mut self) {
        LIVE_SESSIONS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct RunningHost {
    host: Arc<NativeHost>,
    slot: SessionSlot,
    _native_consumer: Option<crate::NativeEventStream>,
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
        Self::open_with_embedding(config, crate::NativeEmbedding::default()).await
    }

    pub async fn open_with_embedding(
        config: HostConfig,
        embedding: crate::NativeEmbedding,
    ) -> Result<Arc<Self>, SessionError> {
        let slot = SessionSlot::acquire()?;
        let lifecycle = lifecycle_executor()?;
        let worker = lifecycle.clone();
        let (reply, result) = oneshot::channel();
        lifecycle
            .send(Box::new(move || {
                let events = NativeSessionEvents::new(config.limits, false);
                let native_consumer = if embedding.application_events
                    == crate::ApplicationEventDispatch::NativeCallback
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
                    NativeHost::start_with_embedding(config, Arc::new(events.clone()), embedding)
                        .map_err(SessionError::Start)
                        .map(|host| {
                            let (stopped, _) = watch::channel(None);
                            Arc::new(Self {
                                shared: Arc::new(SessionState {
                                    running: Mutex::new(Some(RunningHost {
                                        host: Arc::new(host),
                                        slot,
                                        _native_consumer: native_consumer,
                                    })),
                                    events,
                                    lifecycle: worker,
                                    stopped,
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
        }
    }

    pub async fn stop(&self) -> Result<(), SessionError> {
        let mut completion = self.shared.stopped.subscribe();
        self.shared.request_stop();
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

impl Drop for OwnedSession {
    fn drop(&mut self) {
        self.shared.request_stop();
    }
}

impl SessionState {
    fn host(&self) -> Result<Arc<NativeHost>, SessionError> {
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
                _native_consumer,
            } = running;
            let result = host.stop_result().map_err(SessionError::Stop);
            drop(host);
            drop(_native_consumer);
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
    pub async fn execute(
        &self,
        command: HostCommand,
    ) -> Result<Result<CommandOutcome, CommandFailure>, SessionError> {
        let command = self.shared.host()?.submit(command).map_err(submit_error)?;
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

    #[must_use]
    pub fn events(&self) -> NativeSessionEvents {
        self.shared.events.clone()
    }

    pub fn native_services(&self) -> Result<crate::NativeServiceClient, SessionError> {
        self.shared.host()?.service_client().map_err(submit_error)
    }

    /// Reuse an existing public Rust protocol API on the bounded host runtime.
    /// Accepted operations survive waiter cancellation and are accounted for by
    /// joined shutdown. This grants no ownership or stop authority.
    pub async fn protocol_operation<T, F, Fut>(&self, operation: F) -> Result<T, SessionError>
    where
        T: Send + 'static,
        F: FnOnce(personal_rns::PrnsNodeHandle) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send + 'static,
    {
        self.shared
            .host()?
            .on_preview_runtime(operation)
            .await
            .map_err(submit_error)
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
        let services = self.native_services()?;
        Ok(services
            .protocols()
            .destination_identity(DestinationIdentityQuery::Destination(
                personal_rns::wire::DestinationHash::new(destination.into_bytes()),
            ))
            .await
            .map(|snapshot| *snapshot.public.as_bytes()))
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
        crate::NativeRemoteControlError,
    > {
        self.shared
            .host()
            .map_err(|_| crate::NativeRemoteControlError::Admission(NativeSubmitError::Stopped))?
            .remote_control_exchange(
                personal_rns::routing::links::LinkId::new(link_id.into_bytes()),
                request,
            )
            .await
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
        crate::NativeRemoteControlError,
    > {
        self.shared
            .host()
            .map_err(|_| crate::NativeRemoteControlError::Admission(NativeSubmitError::Stopped))?
            .remote_control_target_exchange(
                personal_rns::identity::IdentityHash::new(target.into_bytes()),
                request,
            )
            .await
    }

    pub fn begin_resource_upload(
        &self,
        link_id: LinkId,
        declared_length: u64,
        metadata: Option<Vec<u8>>,
        compression: ResourceCompression,
    ) -> Result<crate::NativeUpload, SessionError> {
        self.shared
            .host()?
            .begin_resource_upload(link_id, declared_length, metadata, compression)
            .map_err(submit_error)
    }
}

fn submit_error(error: NativeSubmitError) -> SessionError {
    match error {
        NativeSubmitError::Busy => SessionError::Busy,
        NativeSubmitError::Stopped => SessionError::Stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_host::{
        ConsumerLane, HostRole, IdentityConfig, LifecycleState, PersistenceConfig, PrnsLimits,
    };

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
                Err(crate::session::NativeStreamError::Stopped)
            ));
        });
    }
}
