//! Bounded process-owned observations of single canonical exchanges. This owns
//! no runtime, transport or persistence and never retries a request. Embeddings
//! retain this object across UI reloads and may retain it across host replacement.
use std::{
    collections::BTreeMap,
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

use personal_rns::{
    engine::SendRequestFailure,
    remote_control::{RemoteControlRequest, RemoteControlRequestKind},
    runtime::{RemoteControlError, RemoteControlTargetOperationError, SendError},
    PrnsNodeHandle,
};

use super::{
    CloseLinkOnDrop, ExchangeResult, HostRemoteControlError, RemoteControlExchangeSettlement,
    RemoteControlOperationCancellation, RemoteControlOperationDestination,
    RemoteControlOperationRetirement, RemoteControlOperationSnapshot, RemoteControlOperationState,
    RemoteControlRetentionConfigError,
};
use crate::{lock, owner::HostClient, HostSubmitError};

const MAX_OPERATIONS: u32 = 64;

/// Caller-selected bounded retention. Each response is also bounded by the
/// canonical protocol. Full retention rejects admission; no record is evicted.
pub struct RemoteControlOperations {
    entries: Mutex<Entries>,
    capacity: usize,
}

struct Entries {
    next: u64,
    records: BTreeMap<u64, Arc<Operation>>,
}

struct Operation {
    id: u64,
    destination: RemoteControlOperationDestination,
    kind: RemoteControlRequestKind,
    state: Mutex<State>,
}

struct State {
    value: RemoteControlOperationState,
    // One registration per live wait future; cancellation removes it.
    waiters: Vec<(Arc<()>, Waker)>,
}

impl State {
    fn terminal_waiters(&mut self) -> Vec<(Arc<()>, Waker)> {
        if self.value.is_terminal() {
            std::mem::take(&mut self.waiters)
        } else {
            Vec::new()
        }
    }
}

impl RemoteControlOperations {
    /// No filesystem or runtime effects. The maximum retained operation count
    /// is 64; callers may choose any nonzero smaller bound.
    pub fn new(capacity: u32) -> Result<Self, RemoteControlRetentionConfigError> {
        if capacity == 0 || capacity > MAX_OPERATIONS {
            return Err(RemoteControlRetentionConfigError {
                maximum_operations: MAX_OPERATIONS,
            });
        }
        Ok(Self {
            entries: Mutex::new(Entries {
                next: 1,
                records: BTreeMap::new(),
            }),
            capacity: capacity as usize,
        })
    }

    pub fn submit_target(
        &self,
        client: &HostClient,
        target: prns_host::IdentityHash,
        request: RemoteControlRequest,
    ) -> Result<u64, HostSubmitError> {
        self.submit(
            client,
            RemoteControlOperationDestination::Target {
                identity: personal_rns::identity::IdentityHash::new(target.into_bytes()),
            },
            request,
        )
    }

    /// The caller retains ownership of the supplied link, as with the ordinary
    /// shared-host exchange API. Only target exchanges own a temporary link.
    pub fn submit_link(
        &self,
        client: &HostClient,
        link: prns_host::LinkId,
        request: RemoteControlRequest,
    ) -> Result<u64, HostSubmitError> {
        self.submit(
            client,
            RemoteControlOperationDestination::Link {
                link_id: personal_rns::routing::links::LinkId::new(link.into_bytes()),
            },
            request,
        )
    }

    fn submit(
        &self,
        client: &HostClient,
        destination: RemoteControlOperationDestination,
        request: RemoteControlRequest,
    ) -> Result<u64, HostSubmitError> {
        let mut entries = lock(&self.entries);
        if entries.records.len() == self.capacity {
            return Err(HostSubmitError::Busy);
        }
        let id = entries.next;
        entries.next = id.checked_add(1).ok_or(HostSubmitError::Busy)?;
        let operation = Arc::new(Operation {
            id,
            destination,
            kind: request.kind(),
            state: Mutex::new(State {
                value: RemoteControlOperationState::Pending,
                waiters: Vec::new(),
            }),
        });
        entries.records.insert(id, operation.clone());
        // Construct before queue insertion: closing the host can drop a queued
        // closure without polling its future, and still must settle its record.
        let completion = CompletionOnDrop(operation);
        let admitted = client.admit(|host| {
            host.begin_preview_operation(move |handle| async move {
                completion.run(handle, request).await;
            })
        });
        match admitted {
            Ok(receiver) => {
                // Observation belongs to this store, never this throwaway waiter.
                drop(receiver);
                Ok(id)
            }
            Err(error) => {
                entries.records.remove(&id);
                Err(error)
            }
        }
    }

    pub fn operation(&self, id: u64) -> Option<RemoteControlOperationSnapshot> {
        self.find(id).map(|operation| operation.snapshot())
    }

    pub fn operations(&self) -> Vec<RemoteControlOperationSnapshot> {
        lock(&self.entries)
            .records
            .values()
            .map(|operation| operation.snapshot())
            .collect()
    }

    /// Observe repeatedly without consuming a result. Dropping this future has
    /// no effect on submission, cancellation or retention.
    pub async fn wait(&self, id: u64) -> Option<RemoteControlOperationSnapshot> {
        Some(
            Wait {
                operation: self.find(id)?,
                registration: None,
            }
            .await,
        )
    }

    /// Prevent submission, including when connection setup is in progress. The
    /// bounded host task still owns and cleans up any connection setup; this is
    /// not a claim that transport teardown has joined.
    pub fn cancel(&self, id: u64) -> RemoteControlOperationCancellation {
        let Some(operation) = self.find(id) else {
            return RemoteControlOperationCancellation::Missing;
        };
        let mut state = lock(&operation.state);
        let outcome = match state.value {
            RemoteControlOperationState::Pending => {
                state.value = RemoteControlOperationState::CancelledBeforeSubmission;
                RemoteControlOperationCancellation::CancelledBeforeSubmission
            }
            RemoteControlOperationState::Submitted => {
                RemoteControlOperationCancellation::AlreadySubmitted
            }
            _ => RemoteControlOperationCancellation::AlreadySettled,
        };
        let waiters = state.terminal_waiters();
        drop(state);
        wake(waiters);
        outcome
    }

    pub fn retire(&self, id: u64) -> RemoteControlOperationRetirement {
        let mut entries = lock(&self.entries);
        let Some(operation) = entries.records.get(&id) else {
            return RemoteControlOperationRetirement::Missing;
        };
        if !lock(&operation.state).value.is_terminal() {
            return RemoteControlOperationRetirement::Pending;
        }
        entries.records.remove(&id);
        RemoteControlOperationRetirement::Retired
    }

    fn find(&self, id: u64) -> Option<Arc<Operation>> {
        lock(&self.entries).records.get(&id).cloned()
    }
}

impl Operation {
    fn snapshot(&self) -> RemoteControlOperationSnapshot {
        RemoteControlOperationSnapshot {
            operation_id: self.id,
            destination: self.destination,
            request_kind: self.kind,
            state: lock(&self.state).value.clone(),
        }
    }

    fn finish(&self, result: RemoteControlOperationState) {
        let mut state = lock(&self.state);
        if state.value.is_terminal() {
            return;
        }
        state.value = result;
        let waiters = state.terminal_waiters();
        drop(state);
        wake(waiters);
    }

    // Only native synchronous request enqueue executes here. There is no await,
    // foreign callback or host operation result wait under the state mutex.
    fn begin<T>(&self, enqueue: impl FnOnce() -> Result<T, HostRemoteControlError>) -> Option<T> {
        let mut state = lock(&self.state);
        if !matches!(state.value, RemoteControlOperationState::Pending) {
            return None;
        }
        let pending = match enqueue() {
            Ok(pending) => {
                state.value = RemoteControlOperationState::Submitted;
                Some(pending)
            }
            Err(failure) => {
                state.value = RemoteControlOperationState::NotSubmitted { failure };
                None
            }
        };
        let waiters = state.terminal_waiters();
        drop(state);
        wake(waiters);
        pending
    }
}

// A terminal observer is independent of the executor driving it. Do not let a
// failed foreign/native callback unwind the exchange or strand other waiters.
fn wake(waiters: Vec<(Arc<()>, Waker)>) {
    for (_, waker) in waiters {
        let _ = catch_unwind(AssertUnwindSafe(|| waker.wake()));
    }
}

struct Wait {
    operation: Arc<Operation>,
    registration: Option<Arc<()>>,
}

impl Future for Wait {
    type Output = RemoteControlOperationSnapshot;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        // Waker clone/drop/wake may invoke arbitrary executor code.
        let waker = cx.waker().clone();
        let mut state = lock(&this.operation.state);
        if state.value.is_terminal() {
            let result = RemoteControlOperationSnapshot {
                operation_id: this.operation.id,
                destination: this.operation.destination,
                request_kind: this.operation.kind,
                state: state.value.clone(),
            };
            drop(state);
            return Poll::Ready(result);
        }
        let previous = if let Some(registration) = &this.registration {
            let Some((_, registered)) = state
                .waiters
                .iter_mut()
                .find(|(id, _)| Arc::ptr_eq(id, registration))
            else {
                // Only terminal publication drains a live registration.
                return Poll::Pending;
            };
            Some(std::mem::replace(registered, waker))
        } else {
            let registration = Arc::new(());
            state.waiters.push((registration.clone(), waker));
            this.registration = Some(registration);
            None
        };
        drop(state);
        drop(previous);
        Poll::Pending
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        let Some(registration) = self.registration.take() else {
            return;
        };
        let removed = {
            let mut state = lock(&self.operation.state);
            state
                .waiters
                .iter()
                .position(|(id, _)| Arc::ptr_eq(id, &registration))
                .map(|index| state.waiters.swap_remove(index))
        };
        drop(removed);
    }
}

struct CompletionOnDrop(Arc<Operation>);

impl CompletionOnDrop {
    async fn run(self, handle: PrnsNodeHandle, request: RemoteControlRequest) {
        if lock(&self.0.state).value.is_terminal() {
            return;
        }
        let result = match self.0.destination {
            RemoteControlOperationDestination::Link { link_id } => {
                let Some(exchange) = self.0.begin(|| {
                    handle
                        .remote_control(link_id)
                        .begin_exchange(request)
                        .map_err(HostRemoteControlError::Exchange)
                }) else {
                    return;
                };
                exchange
                    .wait()
                    .await
                    .map_err(HostRemoteControlError::Exchange)
            }
            RemoteControlOperationDestination::Target { identity } => {
                let connection = match handle.connect_remote_control_target(identity).await {
                    Ok(connection) => connection,
                    Err(error) => {
                        self.0.finish(RemoteControlOperationState::NotSubmitted {
                            failure: HostRemoteControlError::Connect(error),
                        });
                        return;
                    }
                };
                let _close = CloseLinkOnDrop {
                    handle: handle.clone(),
                    link_id: connection.connection().link_id(),
                };
                let Some(exchange) = self.0.begin(|| {
                    connection
                        .begin_exchange(request)
                        .map_err(HostRemoteControlError::TargetOperation)
                }) else {
                    return;
                };
                exchange
                    .wait()
                    .await
                    .map_err(|error| HostRemoteControlError::TargetOperation(error.into()))
            }
        };
        self.0.finish(classify_submitted(result));
    }
}

impl Drop for CompletionOnDrop {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        let failure = HostRemoteControlError::Admission(HostSubmitError::Stopped);
        match state.value {
            RemoteControlOperationState::Pending => {
                state.value = RemoteControlOperationState::NotSubmitted { failure }
            }
            RemoteControlOperationState::Submitted => {
                state.value = RemoteControlOperationState::OutcomeUnknown { failure }
            }
            _ => return,
        }
        let waiters = state.terminal_waiters();
        drop(state);
        wake(waiters);
    }
}

fn classify_submitted(result: ExchangeResult) -> RemoteControlOperationState {
    match result {
        Ok(result) => RemoteControlOperationState::Completed {
            settlement: Ok(result).into(),
        },
        Err(failure) => {
            let exchange = match &failure {
                HostRemoteControlError::Exchange(error) => Some(error),
                HostRemoteControlError::TargetOperation(
                    RemoteControlTargetOperationError::Exchange(error),
                ) => Some(error),
                _ => None,
            };
            match exchange {
                Some(RemoteControlError::Remote(_)) => RemoteControlOperationState::Completed {
                    settlement: RemoteControlExchangeSettlement::Failed { failure },
                },
                Some(RemoteControlError::Request(SendError::Failed(
                    SendRequestFailure::Rejected(_),
                ))) => RemoteControlOperationState::NotSubmitted { failure },
                _ => RemoteControlOperationState::OutcomeUnknown { failure },
            }
        }
    }
}

#[cfg(test)]
mod tests;
