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
pub use remote_control::{RemoteControlHandle, RemoteControlTargetHandle};

#[cfg(test)]
pub(crate) fn test_remote_control_service(
) -> prns_core::remote_control::RemoteControlService<'static> {
    use prns_core::identity::vault::IdentitySecretKey;
    use prns_core::remote_control::{
        RemoteControlControllerIdentitySecret, RemoteControlInitialControllerGrants,
        RemoteControlNodeIdentitySecrets, RemoteControlSelfAnnouncement, RemoteControlService,
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
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    )
}

#[cfg(test)]
pub(crate) fn test_remote_control_grant(
    request: prns_core::remote_control::RemoteControlRequestKind,
) -> prns_core::remote_control::RemoteControlControllerGrant {
    let identities = test_remote_control_service()
        .configuration()
        .unwrap()
        .identity_secrets()
        .identities();
    prns_core::remote_control::RemoteControlControllerGrant::new(
        *identities.controller(),
        prns_core::remote_control::RemoteControlRequestSet::only(request),
    )
    .unwrap()
}

#[cfg(test)]
pub(crate) fn test_remote_control_pairing_attempt(
    endpoint_fill: u8,
) -> prns_core::remote_control::RemoteControlPairingAttemptId {
    use prns_core::identity::in_memory::InMemoryNodeIdentity;
    use prns_core::identity::vault::IdentitySecretKey;
    use prns_core::identity::IdentityHash;
    use prns_core::remote_control::{
        RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin, RemoteControlPairingContext,
        RemoteControlPairingIdentity, RemoteControlPairingInvitationCode,
        RemoteControlPairingPermissions, RemoteControlPairingPreparedOffer,
        RemoteControlRequestKind, RemoteControlRequestSet,
    };
    use prns_core::routing::links::LinkId;
    use prns_core::units::DurationMillis;
    use prns_core::wire::TRUNCATED_HASH_BYTE_LEN;

    let target_signer = InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
        [0x52; crate::identity::IDENTITY_SECRET_KEY_LEN],
    ));
    let controller = *test_remote_control_service()
        .configuration()
        .unwrap()
        .identity_secrets()
        .identities()
        .controller();
    let context = RemoteControlPairingContext::new(
        RemoteControlPairingIdentity::new(IdentityHash::new(
            [endpoint_fill; TRUNCATED_HASH_BYTE_LEN],
        ))
        .endpoint(),
        LinkId::new([0x84; TRUNCATED_HASH_BYTE_LEN]),
    );
    let begin = RemoteControlPairingBegin::new(
        controller,
        context.endpoint(),
        RemoteControlPairingInvitationCode::from_value(0x1234_ABCD),
    );
    let prepared = RemoteControlPairingPreparedOffer::new(
        &target_signer,
        context,
        &begin,
        RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
            RemoteControlRequestKind::Describe,
        ))
        .unwrap(),
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(30_000)).unwrap(),
    );
    let (_, transcript) = prepared.into_parts();
    (&transcript).into()
}
