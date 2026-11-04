use super::*;
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "ServiceRuntimeNetworkPhase")
)]
pub enum NetworkPhase {
    Offline,
    Starting,
    Ready,
    Stopping,
    Failed,
    Unresolved,
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "ServiceRuntimeNetworkStatus")
)]
pub struct NetworkStatus {
    pub phase: NetworkPhase,
    /// A host generation, never a durable identity or service generation.
    pub generation: u64,
    pub failure: Option<Error>,
}
/// Observation of one retained transition. Losing it never cancels owned work.
#[derive(Clone)]
pub struct NetworkChange {
    result: watch::Receiver<Option<Result<(), Error>>>,
}
impl NetworkChange {
    pub async fn wait(&self) -> Result<(), Error> {
        let mut result = self.result.clone();
        loop {
            let value = result.borrow().clone();
            if let Some(value) = value {
                return value;
            }
            if result.changed().await.is_err() {
                return Err(Error::LifecyclePanicked);
            }
        }
    }
}
pub(super) struct Startup {
    pub host: prns_host::HostConfig,
    pub embedding: HostEmbedding,
}
pub(super) enum RequestKind {
    Start(Box<Startup>),
    Stop,
}
pub(super) struct NetworkRequest {
    pub kind: RequestKind,
    pub start: bool,
    pub generation: u64,
    pub completion: watch::Sender<Option<Result<(), Error>>>,
}
impl Shared {
    pub(super) fn request_network(&self, kind: RequestKind) -> Result<NetworkChange, Error> {
        let (change, retired) = {
            let mut state = self.lock();
            if state.closed {
                return Err(Error::Closing);
            }
            let start = matches!(kind, RequestKind::Start(_));
            if state.status.phase == Phase::Unresolved {
                return Err(state
                    .status
                    .failure
                    .clone()
                    .unwrap_or(Error::LifecyclePanicked));
            }
            if start {
                if let Some(Err(error)) = &state.startup {
                    return Err(error.clone());
                }
            }
            if state.status.cleanup_complete {
                if start {
                    return Err(Error::Closing);
                }
                let (_, result) = watch::channel(Some(Ok(())));
                let change = NetworkChange { result };
                state.network.phase = NetworkPhase::Offline;
                state.network.failure = None;
                state.operation = Some(change.clone());
                return Ok(change);
            }
            let joins = matches!(
                (start, state.network.phase),
                (true, NetworkPhase::Starting | NetworkPhase::Ready)
                    | (false, NetworkPhase::Stopping | NetworkPhase::Offline)
            );
            if joins {
                if let Some(change) = &state.operation {
                    return Ok(change.clone());
                }
            }
            if start
                && matches!(
                    state.network.phase,
                    NetworkPhase::Stopping | NetworkPhase::Unresolved
                )
            {
                return Err(Error::NotReady);
            }
            let generation = if start {
                state
                    .network
                    .generation
                    .checked_add(1)
                    .ok_or(Error::InvalidOptions)?
            } else {
                state.network.generation
            };
            let (completion, result) = watch::channel(None);
            let change = NetworkChange { result };
            state.operation = Some(change.clone());
            state.network = NetworkStatus {
                phase: if start {
                    NetworkPhase::Starting
                } else {
                    NetworkPhase::Stopping
                },
                generation,
                failure: None,
            };
            if !start {
                if let Some(host) = &state.host {
                    host.fence();
                }
            }
            let retired = state.request.replace(NetworkRequest {
                kind,
                start,
                generation,
                completion,
            });
            (change, retired)
        };
        // Start not yet acquired by the worker can be superseded by Stop.
        if let Some(retired) = retired {
            retired
                .completion
                .send_replace(Some(Err(Error::StartupCancelled)));
        }
        self.wake();
        Ok(change)
    }
    pub(super) fn finish_cleanup(&self, outcome: &Result<(), Error>) {
        let pending = {
            let mut state = self.lock();
            state.status.phase = Phase::Closed;
            state.status.cleanup_complete = true;
            state.request.take().map(|request| {
                let result = if request.start {
                    Err(outcome.as_ref().err().cloned().unwrap_or(Error::Closing))
                } else {
                    Ok(())
                };
                if !state.closed {
                    state.network.phase = if result.is_ok() {
                        NetworkPhase::Offline
                    } else {
                        NetworkPhase::Failed
                    };
                    state.network.failure = result.as_ref().err().cloned();
                }
                (request, result)
            })
        };
        // Admission and terminal publication share one lock. A Stop either joins
        // the final drain here or observes its completed cleanup above.
        if let Some((request, result)) = pending {
            request.completion.send_replace(Some(result));
        }
        self.notify_ready();
    }
    pub(super) fn complete_network(
        &self,
        request: NetworkRequest,
        result: Result<(), Error>,
        unresolved: bool,
    ) {
        {
            let mut state = self.lock();
            if state.request.is_none() && !state.closed {
                state.network.phase = match (&result, unresolved, request.start) {
                    (Err(_), true, _) => NetworkPhase::Unresolved,
                    (Err(_), false, _) => NetworkPhase::Failed,
                    (Ok(()), _, true) => NetworkPhase::Ready,
                    (Ok(()), _, false) => NetworkPhase::Offline,
                };
                state.network.failure = result.as_ref().err().cloned();
            }
        }
        request.completion.send_replace(Some(result));
    }
}
impl Owner {
    pub fn network_status(&self) -> NetworkStatus {
        self.shared.lock().network.clone()
    }
    /// Observe the most recently accepted transition without selecting a new host.
    pub async fn network_ready(&self) -> Result<(), Error> {
        let operation = self
            .shared
            .lock()
            .operation
            .clone()
            .ok_or(Error::NotReady)?;
        operation.wait().await
    }
    /// Every start consumes its explicit host policy and fresh platform preparation.
    pub fn start_networking_with_embedding(
        &self,
        host: prns_host::HostConfig,
        embedding: HostEmbedding,
    ) -> Result<NetworkChange, Error> {
        self.shared
            .request_network(RequestKind::Start(Box::new(Startup { host, embedding })))
    }
    pub fn start_networking(&self, host: prns_host::HostConfig) -> Result<NetworkChange, Error> {
        self.start_networking_with_embedding(host, HostEmbedding::default())
    }
    pub fn stop_networking(&self) -> Result<NetworkChange, Error> {
        self.shared.request_network(RequestKind::Stop)
    }
    pub fn announcements(
        &self,
        reference: prns_identities_core::IdentityReference,
    ) -> Result<prns_identities::announces::Client, Error> {
        let local = self.local()?;
        Ok(network::announcements(
            local.commands(),
            reference,
            network::Command::Announce,
        ))
    }
}
