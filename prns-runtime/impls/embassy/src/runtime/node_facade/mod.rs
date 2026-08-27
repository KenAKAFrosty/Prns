mod command_handle;
mod interface_lifecycle;
mod manifold_lanes;
mod node_lifecycle;
mod remote_control;

pub use command_handle::{CompletionPool, PrnsNodeHandle, RequestResponseData};
pub use interface_lifecycle::{Fleet, InboundDeliveryError, OutboundFrame};
pub use manifold_lanes::{
    minimum_manifold_notification_capacity, InterfaceLane, LaneClaimError, ManifoldLaneSet,
    StaticManifoldLane, SupervisorLane,
};
pub use node_lifecycle::{ManifoldWiring, PrnsNode, RequestRoutingCapacity};
pub use remote_control::RemoteControlHandle;

#[cfg(test)]
pub(crate) fn test_remote_control_service(
) -> prns_core::remote_control::RemoteControlService<'static> {
    use prns_core::identity::vault::IdentitySecretKey;
    use prns_core::remote_control::{
        RemoteControlControllerIdentitySecret, RemoteControlInitialAccess,
        RemoteControlNodeIdentitySecrets, RemoteControlPublicAppData, RemoteControlService,
        RemoteControlTargetIdentitySecret,
    };

    let identity_secrets = RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
            [0x71; crate::identity::IDENTITY_SECRET_KEY_LEN],
        )),
        RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
            [0x72; crate::identity::IDENTITY_SECRET_KEY_LEN],
        )),
    )
    .expect("distinct test identities");
    RemoteControlService::new(
        identity_secrets,
        RemoteControlPublicAppData::try_from(b"".as_slice()).expect("empty app data"),
        RemoteControlInitialAccess::Nobody,
    )
}
