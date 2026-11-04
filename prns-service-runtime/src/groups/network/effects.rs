//! Retained physical operations on the one supplied service executor.
use super::*;
use prns_host_runtime::service_registration::ServiceLifecycleError;
use prns_identities_runtime::OwnerToken;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll, Waker},
};

#[derive(Debug)]
pub enum Failure {
    Scope(HostServiceScopeError),
    Service(ServiceLifecycleError),
    Group(prns_groups::Error),
    #[cfg(feature = "contact-card")]
    Contact(prns_identities_core::ApiError),
    #[cfg(feature = "contact-card")]
    AnnounceData(prns_runtime::runtime::SetRegisteredAnnounceAppDataError),
    UnexpectedReceiveState,
    WrongDestination,
    TaskCancelled,
    TaskPanicked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Registration,
    Runner,
    Retirement,
    #[cfg(feature = "contact-card")]
    ContactPublication,
}
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub owner: OwnerToken,
    pub operation: Operation,
    pub error: Arc<Failure>,
}
/// Inspectable after owner/client loss. A failure is not proof of drainage, and
/// observing one never changes the identity's desired lifecycle state.
#[derive(Clone, Default)]
pub struct Diagnostics(Arc<Mutex<Option<Diagnostic>>>);
impl Diagnostics {
    pub fn latest(&self) -> Option<Diagnostic> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    fn publish(&self, value: Diagnostic) {
        let old = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(value);
        drop(old);
    }
}

/// Owns one physical attempt and its original result. Polling never restarts it;
/// the scoped driver chooses explicit retries using the same endpoint tokens.
pub(super) struct Task {
    owner: OwnerToken,
    operation: Operation,
    diagnostics: Diagnostics,
    future: Option<tokio::task::JoinHandle<Result<(), Failure>>>,
    result: Option<Result<(), Arc<Failure>>>,
}
impl Task {
    pub fn spawn(
        context: &super::Context,
        owner: OwnerToken,
        operation: Operation,
        future: impl Future<Output = Result<(), Failure>> + Send + 'static,
    ) -> Self {
        Self {
            owner,
            operation,
            diagnostics: context.diagnostics(),
            future: Some(context.executor().spawn(future)),
            result: None,
        }
    }
    #[cfg(test)]
    pub(super) fn completion_probe(&self) -> Option<tokio::task::AbortHandle> {
        self.future
            .as_ref()
            .map(tokio::task::JoinHandle::abort_handle)
    }

    pub fn poll(&mut self, wake: &Waker) -> Option<Result<(), Arc<Failure>>> {
        if self.result.is_none() {
            let future = self.future.as_mut()?;
            let result = match Pin::new(future).poll(&mut TaskContext::from_waker(wake)) {
                Poll::Pending => return None,
                Poll::Ready(Ok(result)) => result,
                Poll::Ready(Err(error)) => Err(if error.is_panic() {
                    Failure::TaskPanicked
                } else {
                    Failure::TaskCancelled
                }),
            };
            let result = result.map_err(Arc::new);
            if let Err(error) = &result {
                self.diagnostics.publish(Diagnostic {
                    owner: self.owner,
                    operation: self.operation,
                    error: error.clone(),
                });
            }
            self.future = None;
            self.result = Some(result);
        }
        self.result.clone()
    }
}
