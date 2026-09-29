mod cancellation;

use tokio::sync::{mpsc, oneshot};

use crate::remote_control::{
    RemoteControlControllerGrantTable, RemoteControlRequestKind,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
};

use super::super::node_facade::{test_remote_control_grant, PrnsNodeHandle};
use super::super::{
    RemoteControlControllerGrantControl, RevokeRemoteControlControllerControlError,
    RevokeRemoteControlControllerServiceError, SetRemoteControlControllerGrantControlError,
    SetRemoteControlControllerGrantServiceError,
};
use super::RemoteControlControllerGrantCommand;

#[tokio::test]
async fn unavailable_service_rejects_controller_grant_changes() {
    let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
    let mut engine = crate::engine::EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote_control = crate::runtime::configure_remote_control_service(
        &mut engine,
        crate::remote_control::RemoteControlService::Unavailable,
    )
    .expect("unavailable RemoteControl requires no storage");

    let (completion, settled) = oneshot::channel();
    RemoteControlControllerGrantCommand::SetControllerGrant { grant, completion }
        .apply(&mut remote_control, None)
        .await
        .unwrap();
    assert_eq!(
        settled.await.expect("set completion remains connected"),
        Err(SetRemoteControlControllerGrantServiceError::Unavailable),
    );

    let (completion, settled) = oneshot::channel();
    RemoteControlControllerGrantCommand::RevokeController {
        controller: *grant.controller(),
        completion,
    }
    .apply(&mut remote_control, None)
    .await
    .unwrap();
    assert_eq!(
        settled.await.expect("revoke completion remains connected"),
        Err(RevokeRemoteControlControllerServiceError::Unavailable),
    );
}

#[tokio::test]
async fn controller_grant_lane_preserves_exact_set_and_revoke_outcomes() {
    let (commands, _command_rx) = mpsc::unbounded_channel();
    let (handle, mut controller_grants) =
        PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
    let previous = test_remote_control_grant(RemoteControlRequestKind::Describe);
    let grant = test_remote_control_grant(RemoteControlRequestKind::AnnounceSelf);

    let (set, ()) = tokio::join!(handle.set_remote_control_controller_grant(grant), async {
        let Some(RemoteControlControllerGrantCommand::SetControllerGrant {
            grant: submitted,
            completion,
        }) = controller_grants.receive().await
        else {
            panic!("set controller grant command")
        };
        assert_eq!(submitted, grant);
        assert!(completion
            .send(Ok(SetRemoteControlControllerGrantOutcome::Updated {
                previous,
            }))
            .is_ok());
    },);
    assert_eq!(
        set,
        Ok(SetRemoteControlControllerGrantOutcome::Updated { previous }),
    );

    let (revoke, ()) = tokio::join!(
        handle.revoke_remote_control_controller(*grant.controller()),
        async {
            let Some(RemoteControlControllerGrantCommand::RevokeController {
                controller,
                completion,
            }) = controller_grants.receive().await
            else {
                panic!("revoke controller command")
            };
            assert_eq!(controller, *grant.controller());
            assert!(completion
                .send(Ok(RevokeRemoteControlControllerOutcome::Revoked { grant }))
                .is_ok());
        },
    );
    assert_eq!(
        revoke,
        Ok(RevokeRemoteControlControllerOutcome::Revoked { grant }),
    );
}

#[tokio::test]
async fn controller_grant_lane_distinguishes_busy_capacity_and_stopped() {
    let (commands, _command_rx) = mpsc::unbounded_channel();
    let (handle, mut controller_grants) =
        PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
    let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
    let setting = handle.set_remote_control_controller_grant(grant);
    tokio::pin!(setting);
    tokio::select! {
        biased;
        outcome = &mut setting => panic!("unsettled controller_grants change returned: {outcome:?}"),
        () = tokio::task::yield_now() => {}
    }

    assert_eq!(
        handle.set_remote_control_controller_grant(grant).await,
        Err(SetRemoteControlControllerGrantControlError::Busy),
    );
    assert_eq!(
        handle
            .revoke_remote_control_controller(*grant.controller())
            .await,
        Err(RevokeRemoteControlControllerControlError::Busy),
    );
    let Some(RemoteControlControllerGrantCommand::SetControllerGrant { completion, .. }) =
        controller_grants.receive().await
    else {
        panic!("set controller grant command")
    };
    assert!(completion
        .send(Err(
            SetRemoteControlControllerGrantServiceError::CapacityExhausted,
        ))
        .is_ok());
    assert_eq!(
        setting.await,
        Err(SetRemoteControlControllerGrantControlError::CapacityExhausted),
    );

    drop(controller_grants);
    assert_eq!(
        handle.set_remote_control_controller_grant(grant).await,
        Err(SetRemoteControlControllerGrantControlError::NodeStopped),
    );
}

#[tokio::test]
async fn a_cancelled_received_controller_grant_change_does_not_mutate_the_table() {
    let (commands, _command_rx) = mpsc::unbounded_channel();
    let (handle, mut controller_grants) =
        PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
    let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
    let changing =
        tokio::spawn(async move { handle.set_remote_control_controller_grant(grant).await });
    let Some(command) = controller_grants.receive().await else {
        panic!("set controller grant command")
    };
    changing.abort();
    assert!(changing.await.is_err());

    let service = super::super::node_facade::test_remote_control_service();
    let mut engine = crate::engine::EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote_control = crate::runtime::configure_remote_control_service(&mut engine, service)
        .expect("RemoteControl fits growable storage");
    command.apply(&mut remote_control, None).await.unwrap();
    assert!(remote_control.controller_grants().unwrap().is_empty());
}

#[tokio::test]
async fn an_abandoned_queued_controller_grant_change_holds_the_lane_until_drained() {
    let (commands, _command_rx) = mpsc::unbounded_channel();
    let (handle, mut controller_grants) =
        PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
    let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
    {
        let setting = handle.set_remote_control_controller_grant(grant);
        tokio::pin!(setting);
        tokio::select! {
            biased;
            outcome = &mut setting => panic!("unsettled controller_grants change returned: {outcome:?}"),
            () = tokio::task::yield_now() => {}
        }
    }

    assert_eq!(
        handle
            .revoke_remote_control_controller(*grant.controller())
            .await,
        Err(RevokeRemoteControlControllerControlError::Busy),
    );
    let Some(RemoteControlControllerGrantCommand::SetControllerGrant { completion, .. }) =
        controller_grants.receive().await
    else {
        panic!("set controller grant command")
    };
    assert!(completion.is_closed());

    let (revoke, ()) = tokio::join!(
        handle.revoke_remote_control_controller(*grant.controller()),
        async {
            let Some(RemoteControlControllerGrantCommand::RevokeController { completion, .. }) =
                controller_grants.receive().await
            else {
                panic!("revoke controller command")
            };
            assert!(completion
                .send(Ok(RevokeRemoteControlControllerOutcome::NotFound))
                .is_ok());
        },
    );
    assert_eq!(revoke, Ok(RevokeRemoteControlControllerOutcome::NotFound));
}
