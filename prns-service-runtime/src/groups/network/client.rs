use super::*;
use crate::groups;
use prns_identities::{announces, owned};
use prns_identities_core::{ApiError, IdentityReference};
use tokio::sync::oneshot;

/// Typed extensions on the existing identity worker, with no extra queue.
// The existing bounded lane accounts for this fixed inline payload; boxing
// its smaller variants would add an allocation per accepted control job.
#[allow(clippy::large_enum_variant)]
pub enum Command {
    Group(groups::Command),
    Announce(announces::Job),
    Attach(Context, oneshot::Sender<Result<(), DriverError>>),
    Detach(oneshot::Sender<Result<(), DriverError>>),
}
struct Submit<J> {
    commands: owned::Commands<J>,
    route: fn(announces::Job) -> J,
}
impl<J: Send + 'static> announces::Submit for Submit<J> {
    fn submit(&self, job: announces::Job) -> Result<(), ApiError> {
        self.commands
            .submit((self.route)(job))
            .map_err(ApiError::from)
    }
}
pub fn announcements<J: Send + 'static>(
    commands: owned::Commands<J>,
    identity: IdentityReference,
    route: fn(announces::Job) -> J,
) -> announces::Client {
    announces::Client::new(identity, Arc::new(Submit { commands, route }))
}
