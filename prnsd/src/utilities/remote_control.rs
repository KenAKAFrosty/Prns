use personal_rns::remote_control::{
    RemoteControlInitialControllerGrants, RemoteControlNodeIdentitySecretsError,
    RemoteControlSelfAnnouncement, RemoteControlService,
};
use personal_rns::runtime::{OsEntropyError, OsRuntimeEntropy};

#[derive(Debug)]
pub(crate) enum TransientRemoteControlIdentityError {
    Entropy(OsEntropyError),
    Identity(RemoteControlNodeIdentitySecretsError),
}

impl core::fmt::Display for TransientRemoteControlIdentityError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Entropy(error) => error.fmt(formatter),
            Self::Identity(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TransientRemoteControlIdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Entropy(error) => Some(error),
            Self::Identity(error) => Some(error),
        }
    }
}

pub(super) fn transient_remote_control_service(
) -> Result<RemoteControlService<'static>, TransientRemoteControlIdentityError> {
    let mut entropy =
        OsRuntimeEntropy::try_new().map_err(TransientRemoteControlIdentityError::Entropy)?;
    let identity_secrets = entropy
        .generate_remote_control_identity_secrets()
        .map_err(TransientRemoteControlIdentityError::Identity)?;
    Ok(RemoteControlService::new(
        identity_secrets,
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    ))
}
