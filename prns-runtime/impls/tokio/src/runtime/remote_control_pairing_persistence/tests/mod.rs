mod target_accesses;
mod target_grants;

use crate::engine::EngineState;
use crate::remote_control::{
    RemoteControlControllerGrantTable, RemoteControlRequestKind,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
};
use crate::storage::GrowableHeap;

use super::*;

fn remote_control() -> AssembledRemoteControl {
    let mut engine = EngineState::<GrowableHeap>::default();
    crate::runtime::configure_remote_control_service(
        &mut engine,
        super::super::node_facade::test_remote_control_service(),
    )
    .expect("RemoteControl fits growable storage")
}

#[test]
fn controller_grant_prepare_is_inert_until_activation_and_exactly_reversible() {
    let mut remote_control = remote_control();
    let grant =
        super::super::node_facade::test_remote_control_grant(RemoteControlRequestKind::Describe);

    let prepared = prepare_controller_grant_set(&mut remote_control, grant).unwrap();
    assert_eq!(
        prepared.set_outcome(),
        Some(SetRemoteControlControllerGrantOutcome::Added)
    );
    assert!(remote_control.controller_grants().unwrap().is_empty());
    let (mutation, projected, rollback) = prepared.into_parts();
    assert_ne!(projected, rollback);

    activate_controller_grant_change(&mut remote_control, mutation).unwrap();
    assert_eq!(
        remote_control
            .controller_grants()
            .unwrap()
            .grants_in_identity_hash_order(),
        &[grant]
    );
    rollback_controller_grant(&mut remote_control, mutation).unwrap();
    assert!(remote_control.controller_grants().unwrap().is_empty());
}

#[test]
fn controller_revocation_prepare_is_inert_until_activation_and_exactly_reversible() {
    let mut remote_control = remote_control();
    let grant =
        super::super::node_facade::test_remote_control_grant(RemoteControlRequestKind::Describe);
    assert_eq!(
        remote_control.set_controller_grant(grant),
        Ok(SetRemoteControlControllerGrantOutcome::Added)
    );

    let prepared = prepare_controller_revocation(&mut remote_control, *grant.controller())
        .expect("revocation can be prepared");
    assert_eq!(
        prepared.revoke_outcome(),
        Some(RevokeRemoteControlControllerOutcome::Revoked { grant })
    );
    assert_eq!(
        remote_control
            .controller_grants()
            .unwrap()
            .grants_in_identity_hash_order(),
        &[grant]
    );
    let (mutation, projected, rollback) = prepared.into_parts();
    assert_ne!(projected, rollback);

    activate_controller_grant_change(&mut remote_control, mutation).unwrap();
    assert!(remote_control.controller_grants().unwrap().is_empty());
    rollback_controller_grant(&mut remote_control, mutation).unwrap();
    assert_eq!(
        remote_control
            .controller_grants()
            .unwrap()
            .grants_in_identity_hash_order(),
        &[grant]
    );
}

#[tokio::test]
async fn failed_completion_delivery_keeps_committed_live_authority() {
    use crate::engine::{
        RemoteControlPairingResponseDispatchFailure,
        SettleRemoteControlTargetPairingAuthorizationFailure as Failure,
    };
    use crate::routing::links::LinkId;
    let attempt_id = RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([0x71; 32]);
    for failure in [
        Failure::CompletionDispatchFailed {
            attempt_id,
            failure: RemoteControlPairingResponseDispatchFailure::Write(
                crate::routing::links::request::LinkRequestWriteError::LinkVanished,
            ),
        },
        Failure::CompletionRetentionExpired {
            attempt_id,
            retired_link: LinkId::new([0x72; 16]),
        },
    ] {
        let mut remote = remote_control();
        let prior = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let candidate = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::AnnounceSelf,
        );
        remote.set_controller_grant(prior).unwrap();
        let (mutation, _, _) = prepare_controller_grant_set(&mut remote, candidate)
            .unwrap()
            .into_parts();
        activate_controller_grant_change(&mut remote, mutation).unwrap();
        assert_eq!(
            finalize_controller_grant(Some(Err(failure))),
            Err(
                RemoteControlAuthorizationPersistenceFailure::CommittedCompletionDelivery {
                    failure
                }
            )
        );
        assert_eq!(
            remote
                .controller_grants()
                .unwrap()
                .grants_in_identity_hash_order(),
            &[candidate]
        );
    }
}
