use crate::identity::held::HoldIdentityError;
use crate::remote_control::RemoteControlControllerIdentity;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum RemoteControlControllerIdentityConfiguration {
    #[default]
    Unavailable,
    Configured(RemoteControlControllerIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureRemoteControlIdentitiesError {
    AlreadyConfigured,
    Hold(HoldIdentityError),
}
