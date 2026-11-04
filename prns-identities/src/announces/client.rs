//! Identity-bound controls submitted to the existing embedding owner lane.
use super::{ManualRequest, Settings};
use prns_identities_core::{ApiError, IdentityReference};
use std::sync::Arc;
use tokio::sync::oneshot;

/// The transport selects channels; callers choose only unsolicited scheduling.
#[derive(Clone, Copy, Debug)]
pub enum Request {
    Settings,
    SetSettings(Settings),
    Manual,
}
pub enum Reply {
    Settings(Settings),
    Manual(ManualRequest),
}
/// A fixed-size request. Dropping its observer never cancels accepted work.
pub struct Job {
    identity: IdentityReference,
    request: Request,
    reply: oneshot::Sender<Result<Reply, ApiError>>,
}
impl Job {
    pub fn execute(
        self,
        handler: impl FnOnce(IdentityReference, Request) -> Result<Reply, ApiError>,
    ) {
        let result = handler(self.identity, self.request);
        let _ = self.reply.send(result);
    }
    pub fn fail(self, error: ApiError) {
        let _ = self.reply.send(Err(error));
    }
}
pub trait Submit: Send + Sync {
    fn submit(&self, job: Job) -> Result<(), ApiError>;
}

#[derive(Clone)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Object))]
pub struct IdentityAnnounceClient {
    identity: IdentityReference,
    submit: Arc<dyn Submit>,
}
impl IdentityAnnounceClient {
    pub fn new(identity: IdentityReference, submit: Arc<dyn Submit>) -> Self {
        Self { identity, submit }
    }
    async fn request(&self, request: Request) -> Result<Reply, ApiError> {
        let (reply, result) = oneshot::channel();
        self.submit.submit(Job {
            identity: self.identity,
            request,
            reply,
        })?;
        result.await.map_err(|_| ApiError::WorkerStopped)?
    }
}
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
impl IdentityAnnounceClient {
    pub async fn settings(&self) -> Result<Settings, ApiError> {
        match self.request(Request::Settings).await? {
            Reply::Settings(settings) => Ok(settings),
            Reply::Manual(_) => Err(ApiError::WorkerStopped),
        }
    }
    /// A saved preference edit does not bypass the shared cooldown or request
    /// an immediate announcement. None selects manual-only operation.
    pub async fn set_settings(&self, settings: Settings) -> Result<Settings, ApiError> {
        settings.validate()?;
        match self.request(Request::SetSettings(settings)).await? {
            Reply::Settings(settings) => Ok(settings),
            Reply::Manual(_) => Err(ApiError::WorkerStopped),
        }
    }
    /// Queued/throttled describes a coalesced request, not radio delivery.
    pub async fn announce(&self) -> Result<ManualRequest, ApiError> {
        match self.request(Request::Manual).await? {
            Reply::Manual(result) => Ok(result),
            Reply::Settings(_) => Err(ApiError::WorkerStopped),
        }
    }
}

#[cfg(test)]
mod tests;
