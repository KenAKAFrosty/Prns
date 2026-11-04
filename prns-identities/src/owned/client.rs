//! Typed composition transport. Identity methods live only in crate::api::Client.
use super::*;
pub use prns_identities_core::SubmissionFailure as SubmitFailure;

/// The existing physical worker carries either one opaque canonical identity
/// job or the embedding's typed domain extension. It owns no second dispatcher.
pub struct Job<J = std::convert::Infallible> {
    pub(super) command: Command<J>,
}
// Queue capacity already bounds this inline payload; avoid a second allocation
// for every identity job just to accommodate smaller extension commands.
#[allow(clippy::large_enum_variant)]
pub(super) enum Command<J> {
    Identity(crate::api::IdentityJob),
    Extension(J),
    Runtime(RuntimeCommand),
}
struct Lane<J> {
    lane: prns_storage::Client<Job<J>>,
    control: prns_storage::ControlClient<Job<J>>,
}
fn rejection<J>(error: prns_storage::SubmitError<J>) -> SubmitFailure {
    match error {
        prns_storage::SubmitError::Full(_) => SubmitFailure::Full,
        prns_storage::SubmitError::Closed(_) => SubmitFailure::Closed,
        prns_storage::SubmitError::Reentrant(_) => SubmitFailure::Reentrant,
    }
}
impl<J: Send + 'static> crate::api::Submit for Lane<J> {
    fn submit(&self, identity: crate::api::IdentityJob) -> Result<(), SubmitFailure> {
        let control = identity.uses_control_lane();
        let job = Job {
            command: Command::Identity(identity),
        };
        if control {
            self.control.try_submit(job)
        } else {
            self.lane.try_submit(job)
        }
        .map_err(rejection)
    }
}
impl crate::api::Client {
    /// Bind the actual owner's two queues and exact catalog bounds. This is a
    /// native construction seam; foreign clients cannot select their own lane.
    pub fn from_lane<J: Send + 'static>(
        lane: prns_storage::Client<Job<J>>,
        control: prns_storage::ControlClient<Job<J>>,
        operation_capacity: usize,
        service_capacity: usize,
    ) -> Self {
        Self::new(
            Arc::new(Lane { lane, control }),
            operation_capacity,
            service_capacity,
        )
    }
}
/// Construction-only submission for the selected domain's existing typed jobs.
/// It carries no lifecycle-control capability or caller-selected access class.
pub struct Commands<J> {
    lane: prns_storage::Client<Job<J>>,
}
impl<J> Clone for Commands<J> {
    fn clone(&self) -> Self {
        Self {
            lane: self.lane.clone(),
        }
    }
}
impl<J> Commands<J> {
    pub fn from_lane(lane: prns_storage::Client<Job<J>>) -> Self {
        Self { lane }
    }
    pub fn submit(&self, job: J) -> Result<(), SubmitFailure> {
        self.lane
            .try_submit(Job {
                command: Command::Extension(job),
            })
            .map_err(rejection)
    }
}

/// Construction-only control of the owner's process resources. Suspension drains
/// actual effects without changing durable Enabled/Paused intent or closing data.
pub struct RuntimeControl<J> {
    lane: prns_storage::ControlClient<Job<J>>,
}
impl<J> Clone for RuntimeControl<J> {
    fn clone(&self) -> Self {
        Self {
            lane: self.lane.clone(),
        }
    }
}
pub(super) enum RuntimeCommand {
    Suspend(tokio::sync::oneshot::Sender<Result<(), core::ApiError>>),
    Resume(tokio::sync::oneshot::Sender<Result<(), core::ApiError>>),
}
impl<J: Send + 'static> RuntimeControl<J> {
    pub fn from_lane(lane: prns_storage::ControlClient<Job<J>>) -> Self {
        Self { lane }
    }
    async fn request(
        &self,
        make: impl FnOnce(tokio::sync::oneshot::Sender<Result<(), core::ApiError>>) -> RuntimeCommand,
    ) -> Result<(), core::ApiError> {
        let (send, recv) = tokio::sync::oneshot::channel();
        self.lane
            .try_submit(Job {
                command: Command::Runtime(make(send)),
            })
            .map_err(|error| match rejection(error) {
                SubmitFailure::Full => core::ApiError::Runtime {
                    reason: core::RuntimeFailure::Busy,
                },
                SubmitFailure::Reentrant => core::ApiError::Submission {
                    reason: core::SubmissionFailure::Reentrant,
                },
                SubmitFailure::Closed => core::ApiError::Closing,
                SubmitFailure::Bounds => core::ApiError::InvalidOptions,
            })?;
        recv.await.unwrap_or(Err(core::ApiError::WorkerStopped))
    }
    /// Once submitted, dropping this observer does not cancel the drain.
    pub async fn suspend(&self) -> Result<(), core::ApiError> {
        self.request(RuntimeCommand::Suspend).await
    }
    /// Reopens runtime admission after a completed suspension. Activation remains
    /// explicit through the canonical identity recover operation.
    pub async fn resume(&self) -> Result<(), core::ApiError> {
        self.request(RuntimeCommand::Resume).await
    }
}
