use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::prelude::*;

pub fn remote_control_identity_secrets(
    controller_fill: u8,
    target_fill: u8,
) -> RemoteControlNodeIdentitySecrets {
    RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
            [controller_fill; IDENTITY_SECRET_KEY_LEN],
        )),
        RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
            [target_fill; IDENTITY_SECRET_KEY_LEN],
        )),
    )
    .expect("distinct example RemoteControl identities")
}

#[allow(dead_code)]
pub fn remote_control_service(
    controller_fill: u8,
    target_fill: u8,
) -> RemoteControlService<'static> {
    RemoteControlService::new(
        remote_control_identity_secrets(controller_fill, target_fill),
        RemoteControlPublicAppData::try_from(b"".as_slice()).expect("empty app data"),
        RemoteControlInitialAccess::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    )
}
