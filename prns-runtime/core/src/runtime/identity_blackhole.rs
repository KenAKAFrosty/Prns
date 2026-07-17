use crate::identity::IdentityHash;
use crate::routing::{BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityBlackholeSourceError {
    NodeStopped,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityBlackholeControlError {
    NodeStopped,
    Busy,
    CapacityExhausted,
    ReasonTooLong,
    DurabilityFailed,
}

pub trait IdentityBlackholeSource {
    type Reason: AsRef<str> + Send;
    type Entries: IntoIterator<Item = BlackholedIdentity<Self::Reason>> + Send;

    fn blackholed_identities(
        &self,
    ) -> impl core::future::Future<Output = Result<Self::Entries, IdentityBlackholeSourceError>> + Send;

    fn is_blackholed(
        &self,
        identity: IdentityHash,
    ) -> impl core::future::Future<Output = Result<bool, IdentityBlackholeSourceError>> + Send;
}

pub trait IdentityBlackholeControl {
    fn blackhole_identity<'a>(
        &'a self,
        entry: BlackholedIdentity<&'a str>,
    ) -> impl core::future::Future<
        Output = Result<BlackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send
           + 'a;

    fn unblackhole_identity(
        &self,
        identity: IdentityHash,
    ) -> impl core::future::Future<
        Output = Result<UnblackholeIdentityOutcome, IdentityBlackholeControlError>,
    > + Send;
}

#[cfg(feature = "tokio-host")]
mod tokio;
#[cfg(feature = "tokio-host")]
pub(crate) use tokio::{
    apply_identity_blackhole_command, settle_control, settle_source, IdentityBlackholeHostCommand,
};
