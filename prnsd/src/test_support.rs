use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::remote_control::{
    RemoteControlControllerIdentitySecret, RemoteControlInitialAccess,
    RemoteControlNodeIdentitySecrets, RemoteControlPublicAppData, RemoteControlSelfAnnouncement,
    RemoteControlService, RemoteControlTargetIdentitySecret,
};

const CONTROLLER_SECRET_BYTE: u8 = 0x43;
const TARGET_SECRET_BYTE: u8 = 0x54;

pub(crate) fn remote_control_service() -> RemoteControlService<'static> {
    let identity_secrets = RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(Zeroizing::new(
            [CONTROLLER_SECRET_BYTE; IDENTITY_SECRET_KEY_LEN],
        )),
        RemoteControlTargetIdentitySecret::from(Zeroizing::new(
            [TARGET_SECRET_BYTE; IDENTITY_SECRET_KEY_LEN],
        )),
    )
    .expect("test controller and target identities must remain distinct");
    RemoteControlService::new(
        identity_secrets,
        RemoteControlPublicAppData::empty(),
        RemoteControlInitialAccess::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    )
}
