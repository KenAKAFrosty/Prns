#[cfg(feature = "tokio-host")]
mod tokio;
#[cfg(feature = "tokio-host")]
pub(crate) use tokio::{
    apply_destination_identity_retention_command, settle_destination_identity_retention,
    DestinationIdentityRetentionHostCommand,
};
