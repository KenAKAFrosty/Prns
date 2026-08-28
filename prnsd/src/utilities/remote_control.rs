use personal_rns::remote_control::{
    RemoteControlInitialAccess, RemoteControlNodeIdentityGenerationError,
    RemoteControlNodeIdentitySecrets, RemoteControlPublicAppData, RemoteControlSelfAnnouncement,
    RemoteControlService,
};
use personal_rns::runtime::{fill_os_entropy, OsEntropyError};

pub(crate) type TransientRemoteControlIdentityError =
    RemoteControlNodeIdentityGenerationError<OsEntropyError>;

pub(super) fn transient_remote_control_service(
) -> Result<RemoteControlService<'static>, TransientRemoteControlIdentityError> {
    let identity_secrets = RemoteControlNodeIdentitySecrets::generate(fill_os_entropy)?;
    Ok(RemoteControlService::new(
        identity_secrets,
        RemoteControlPublicAppData::empty(),
        RemoteControlInitialAccess::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    ))
}
