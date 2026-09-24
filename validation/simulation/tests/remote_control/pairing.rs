use super::{durability, fixture::*};
use persistence::Storage;
use personal_rns::engine::{
    RemoteControlControllerPairingResponseEffect, RemoteControlPairingOpened,
};
use personal_rns::remote_control::*;
use personal_rns::runtime::{
    InitiateRemoteControlControllerPairing, RemoteControlControllerPairingInitiationControl,
    RemoteControlPairingControl,
};
use prns_simulation::{FaultPlan, ManualTaskScheduling};

fn begin(
    lab: &mut Lab<'_>,
    opened: RemoteControlPairingOpened,
    code: RemoteControlPairingInvitationCode,
) -> personal_rns::engine::RemoteControlControllerPairingResponseReceived {
    begin_from(lab, OUTSIDER, opened, code).expect("pairing offer")
}

fn begin_from(
    lab: &mut Lab<'_>,
    controller: usize,
    opened: RemoteControlPairingOpened,
    code: RemoteControlPairingInvitationCode,
) -> Result<
    personal_rns::engine::RemoteControlControllerPairingResponseReceived,
    Box<personal_rns::runtime::InitiateRemoteControlControllerPairingError>,
> {
    let handle = lab.nodes[controller].handle.clone();
    let task = lab.insert(async move {
        if handle
            .destination_identity_hash(opened.endpoint.destination_hash())
            .await
            .is_none()
        {
            handle
                .request_path(opened.endpoint.destination_hash())
                .await
                .expect("discover pairing endpoint after restart");
        }
        Event::PairingResponse(
            handle
                .initiate_remote_control_controller_pairing(
                    InitiateRemoteControlControllerPairing {
                        endpoint: opened.endpoint,
                        invitation_code: code,
                        expires_at: opened.expires_at,
                    },
                )
                .await,
        )
    });
    assert!(lab.settle().is_empty(), "identification propagation grace");
    assert!(lab.advance(149).is_empty());
    let mut completed = lab.advance(1);
    assert_eq!(
        completed.len(),
        1,
        "pairing offer response after identification"
    );
    let (found, Event::PairingResponse(response)) = completed.remove(0) else {
        unreachable!("controller initiation");
    };
    assert_eq!(found, task);
    response.map_err(Box::new)
}

#[test]
fn complete_pairing_commits_both_sides_and_restores_pinned_access_across_independent_restarts() {
    let run = || {
        let target_store = Storage::new();
        let controller_store = Storage::new();
        with_storage(
            ManualTaskScheduling::Cyclic,
            FaultPlan::none(),
            durability::policy(),
            Some(target_store.clone()),
            |lab| {
                lab.restart_node(OUTSIDER, controller_store.clone());
                let opened = lab.open_pairing();
                let code = opened.invitation_code.clone();
                assert_eq!(
                    begin(lab, opened, code).effect,
                    RemoteControlControllerPairingResponseEffect::Advanced
                );
                let controller = lab.controller_confirmation(OUTSIDER);
                let target = lab.target_confirmation();
                assert_eq!(controller.confirmation(), target.confirmation());
                assert_eq!(
                    controller.confirmation().controller(),
                    &lab.nodes[OUTSIDER].identity
                );
                assert_eq!(
                    controller.confirmation().target(),
                    &lab.nodes[TARGET].target
                );
                assert_eq!(
                    *controller.confirmation().permissions().permitted_requests(),
                    requests()
                );
                let attempt = controller.confirmation().attempt_id();
                let target_approval = target.approval();
                let target_handle = lab.nodes[TARGET].handle.clone();
                let approved = lab.insert(async move {
                    target_handle
                        .approve_remote_control_target_pairing(target_approval)
                        .await
                        .expect("target approval");
                    Event::Done
                });
                lab.expect_done(approved);
                let controller_approval = controller.approval();
                let controller_handle = lab.nodes[OUTSIDER].handle.clone();
                let approved = lab.insert(async move {
                    let received = controller_handle
                        .approve_remote_control_controller_pairing(controller_approval)
                        .await
                        .expect("controller approval/completion");
                    assert_eq!(
                        received.effect,
                        RemoteControlControllerPairingResponseEffect::Advanced
                    );
                    Event::Done
                });
                lab.expect_done(approved);
                assert!(lab
                    .pairing
                    .borrow()
                    .iter()
                    .any(|observation| observation.node == TARGET
                        && observation.event == pairing::PairingEvent::TargetPersisted(attempt)));
                assert!(lab
                    .pairing
                    .borrow()
                    .iter()
                    .any(|observation| observation.node == OUTSIDER
                        && observation.event
                            == pairing::PairingEvent::ControllerPersisted(attempt)));
                assert!(target_store
                    .grants()
                    .iter()
                    .any(|grant| grant.controller() == &lab.nodes[OUTSIDER].identity
                        && *grant.permitted_requests() == requests()));
                let accesses = controller_store.accesses();
                assert_eq!(accesses.len(), 1);
                assert_eq!(accesses[0].target(), &lab.nodes[TARGET].target);
                assert_eq!(*accesses[0].permitted_requests(), requests());
                for _ in 0..2 {
                    lab.restart_target(target_store.clone());
                    let link = lab.link_pinned(OUTSIDER);
                    assert!(matches!(
                        lab.exchange(OUTSIDER, link, RemoteControlRequest::DescribeBuild),
                        RemoteControlResponse::DescribeBuild(_)
                    ));
                    lab.restart_node(OUTSIDER, controller_store.clone());
                    let link = lab.link_pinned(OUTSIDER);
                    let payload = RemoteControlAppMessage::from_slice(b"paired-after-reboot")
                        .expect("bounded");
                    assert_eq!(
                        lab.exchange(
                            OUTSIDER,
                            link,
                            RemoteControlRequest::AppMessage(payload.clone())
                        ),
                        RemoteControlResponse::AppMessage(payload)
                    );
                    assert_eq!(controller_store.accesses(), accesses);
                }
                (
                    target_store.grants(),
                    controller_store.accesses(),
                    std::mem::take(&mut *lab.pairing.borrow_mut()),
                )
            },
        )
    };
    assert_eq!(run(), run());
}

#[test]
fn invalid_invitation_is_silent_and_does_not_consume_the_pairing_window() {
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(Storage::new()),
        |lab| {
            let opened = lab.open_pairing();
            let wrong =
                RemoteControlPairingInvitationCode::from_value(opened.invitation_code.value() ^ 1);
            let response_count = lab.target_response_count();
            let handle = lab.nodes[OUTSIDER].handle.clone();
            let endpoint = opened.endpoint;
            let expires_at = opened.expires_at;
            let task = lab.insert(async move {
                Event::PairingResponse(
                    handle
                        .initiate_remote_control_controller_pairing(
                            InitiateRemoteControlControllerPairing {
                                endpoint,
                                invitation_code: wrong,
                                expires_at,
                            },
                        )
                        .await,
                )
            });
            assert!(lab.settle().is_empty());
            assert!(lab.advance(151).is_empty(), "invalid proof remains silent");
            assert_eq!(
                lab.runner.cancel(task).expect("cancel unanswered offer"),
                prns_simulation::ManualTaskCancellation::Cancelled
            );
            lab.restart_node(OUTSIDER, Storage::new());
            assert_eq!(
                lab.target_response_count(),
                response_count,
                "invalid proof receives no response"
            );
            assert!(!lab.pairing.borrow().iter().any(|observation| matches!(
                observation.event,
                pairing::PairingEvent::TargetConfirmation(_)
                    | pairing::PairingEvent::ControllerConfirmation(_)
            )));
            let code = opened.invitation_code.clone();
            begin(lab, opened, code);
            let rejection = lab.target_confirmation().rejection();
            let handle = lab.nodes[TARGET].handle.clone();
            let task = lab.insert(async move {
                handle
                    .reject_remote_control_target_pairing(rejection)
                    .await
                    .expect("reject target");
                Event::Done
            });
            lab.expect_done(task);
        },
    );
}

#[derive(Debug)]
enum RejectionSide {
    Controller,
    Target,
    Expiration,
}

#[test]
fn rejected_and_expired_attempts_never_publish_authority_and_stale_approval_cannot_resurrect_them()
{
    for side in [
        RejectionSide::Controller,
        RejectionSide::Target,
        RejectionSide::Expiration,
    ] {
        let target_store = Storage::new();
        let controller_store = Storage::new();
        with_storage(
            ManualTaskScheduling::Cyclic,
            FaultPlan::none(),
            durability::policy(),
            Some(target_store.clone()),
            |lab| {
                lab.restart_node(OUTSIDER, controller_store.clone());
                let prior = target_store.grants();
                let opened = lab.open_pairing();
                let code = opened.invitation_code.clone();
                begin(lab, opened, code);
                let controller = lab.controller_confirmation(OUTSIDER);
                let target = lab.target_confirmation();
                match side {
                    RejectionSide::Controller => {
                        let handle = lab.nodes[OUTSIDER].handle.clone();
                        let reject = controller.rejection();
                        let task = lab.insert(async move {
                            handle
                                .reject_remote_control_controller_pairing(reject)
                                .await
                                .expect("controller rejects");
                            Event::Done
                        });
                        lab.expect_done(task);
                    }
                    RejectionSide::Target => {
                        let handle = lab.nodes[TARGET].handle.clone();
                        let reject = target.rejection();
                        let task = lab.insert(async move {
                            handle
                                .reject_remote_control_target_pairing(reject)
                                .await
                                .expect("target rejects");
                            Event::Done
                        });
                        lab.expect_done(task);
                    }
                    RejectionSide::Expiration => {
                        assert!(lab.advance(10_001).is_empty());
                    }
                }
                assert_eq!(target_store.grants(), prior);
                assert!(controller_store.accesses().is_empty());
                let handle = lab.nodes[OUTSIDER].handle.clone();
                let approve = controller.approval();
                let task = lab.insert(async move {
                    assert!(handle
                        .approve_remote_control_controller_pairing(approve)
                        .await
                        .is_err());
                    Event::Done
                });
                lab.expect_done(task);
                lab.restart_target(target_store.clone());
                lab.restart_node(OUTSIDER, controller_store.clone());
                let link = lab.link(OUTSIDER);
                let denied =
                    lab.request(OUTSIDER, link, encoded(RemoteControlRequest::DescribeBuild));
                lab.expect_timeout(denied);
                assert_eq!(target_store.grants(), prior);
                assert!(controller_store.accesses().is_empty());
            },
        );
    }
}

#[derive(Debug)]
enum InterruptedOwner {
    TargetUnpublished,
    TargetPublished,
    ControllerUnpublished,
    ControllerPublished,
}

#[test]
fn independently_interrupted_pairing_owners_restore_only_their_published_trust() {
    use personal_rns::persistence::SnapshotRegion;
    use personal_rns::runtime::PersistenceIoOperation;
    for owner in [
        InterruptedOwner::TargetUnpublished,
        InterruptedOwner::TargetPublished,
        InterruptedOwner::ControllerUnpublished,
        InterruptedOwner::ControllerPublished,
    ] {
        let target_store = Storage::new();
        let controller_store = Storage::new();
        with_storage(
            ManualTaskScheduling::Cyclic,
            FaultPlan::none(),
            durability::policy(),
            Some(target_store.clone()),
            |lab| {
                lab.restart_node(OUTSIDER, controller_store.clone());
                let prior = target_store.grants();
                let opened = lab.open_pairing();
                let code = opened.invitation_code.clone();
                begin(lab, opened, code);
                let target = lab.target_confirmation();
                let controller = lab.controller_confirmation(OUTSIDER);
                let handle = lab.nodes[TARGET].handle.clone();
                let approve = target.approval();
                let task = lab.insert(async move {
                    handle
                        .approve_remote_control_target_pairing(approve)
                        .await
                        .expect("target accepts");
                    Event::Done
                });
                lab.expect_done(task);
                let (node, store, operation) = match owner {
                    InterruptedOwner::TargetUnpublished => (
                        TARGET,
                        &target_store,
                        PersistenceIoOperation::AuthorizationStore(
                            SnapshotRegion::RemoteControlControllerGrants,
                        ),
                    ),
                    InterruptedOwner::TargetPublished => (
                        TARGET,
                        &target_store,
                        PersistenceIoOperation::AuthorizationFinish,
                    ),
                    InterruptedOwner::ControllerUnpublished => (
                        OUTSIDER,
                        &controller_store,
                        PersistenceIoOperation::AuthorizationStore(
                            SnapshotRegion::RemoteControlTargetAccesses,
                        ),
                    ),
                    InterruptedOwner::ControllerPublished => (
                        OUTSIDER,
                        &controller_store,
                        PersistenceIoOperation::AuthorizationFinish,
                    ),
                };
                let release = store.io.gate(operation);
                let handle = lab.nodes[OUTSIDER].handle.clone();
                let approve = controller.approval();
                let task = lab.insert(async move {
                    let _ = handle
                        .approve_remote_control_controller_pairing(approve)
                        .await;
                    Event::Done
                });
                let completed = lab.settle();
                if matches!(
                    owner,
                    InterruptedOwner::TargetPublished
                        | InterruptedOwner::ControllerUnpublished
                        | InterruptedOwner::ControllerPublished
                ) {
                    assert_eq!(
                        completed.len(),
                        1,
                        "exchange completes before local persistence event"
                    );
                    assert_eq!(completed[0].0, task);
                } else {
                    assert!(
                        completed.is_empty(),
                        "{owner:?}: completed {} actors",
                        completed.len()
                    );
                    lab.runner.cancel(task).expect("cancel old caller");
                }
                lab.restart_node(node, store.clone());
                assert!(
                    release.send(()).is_err(),
                    "old boot no longer owns persistence work"
                );
                let target_published = !matches!(owner, InterruptedOwner::TargetUnpublished);
                let controller_published = matches!(
                    owner,
                    InterruptedOwner::TargetPublished | InterruptedOwner::ControllerPublished
                );
                if target_published {
                    assert!(target_store
                        .grants()
                        .iter()
                        .any(|grant| grant.controller() == &lab.nodes[OUTSIDER].identity));
                } else {
                    assert_eq!(target_store.grants(), prior);
                }
                assert_eq!(
                    controller_store.accesses().len(),
                    usize::from(controller_published)
                );
                for _ in 0..2 {
                    lab.restart_target(target_store.clone());
                    lab.restart_node(OUTSIDER, controller_store.clone());
                    if controller_published {
                        let link = lab.link_pinned(OUTSIDER);
                        assert!(matches!(
                            lab.exchange(OUTSIDER, link, RemoteControlRequest::DescribeBuild),
                            RemoteControlResponse::DescribeBuild(_)
                        ));
                    }
                    assert_eq!(
                        controller_store.accesses().len(),
                        usize::from(controller_published)
                    );
                }
            },
        );
    }
}

#[test]
fn changed_keys_do_not_inherit_a_pinned_target_or_controller_grant() {
    use personal_rns::identity::vault::IdentitySecretKey;
    use personal_rns::runtime::{
        ConnectRemoteControlTargetError, ResolveRemoteControlTargetControlError,
    };
    let target_store = Storage::new();
    let controller_store = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(target_store.clone()),
        |lab| {
            let link = lab.link(OPERATOR);
            assert!(matches!(
                lab.exchange(OPERATOR, link, RemoteControlRequest::DescribeBuild),
                RemoteControlResponse::DescribeBuild(_)
            ));
            let original_target = lab.nodes[TARGET].target.identity_hash();
            let replacement = RemoteControlNodeIdentitySecrets::new(
                RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new([0xc1; 64])),
                RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new([0xc2; 64])),
            )
            .expect("replacement keys");
            lab.restart_node_with_secrets(TARGET, target_store.clone(), replacement);
            let changed_target = lab.nodes[TARGET].target.identity_hash();
            assert_ne!(changed_target, original_target);
            let handle = lab.nodes[OPERATOR].handle.clone();
            let task = lab.insert(async move {
                assert_eq!(
                    handle
                        .connect_remote_control_target(changed_target)
                        .await
                        .err(),
                    Some(ConnectRemoteControlTargetError::Resolve(
                        ResolveRemoteControlTargetControlError::TargetNotAuthorized
                    ))
                );
                Event::Done
            });
            lab.expect_done(task);
            lab.restart_target(target_store);
            let replacement = RemoteControlNodeIdentitySecrets::new(
                RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new([0xd1; 64])),
                RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new([0xd2; 64])),
            )
            .expect("replacement controller");
            lab.restart_node_with_secrets(OPERATOR, controller_store, replacement);
            let link = lab.link(OPERATOR);
            let before = lab.target_response_count();
            let denied = lab.request(OPERATOR, link, encoded(RemoteControlRequest::DescribeBuild));
            lab.expect_timeout(denied);
            assert_eq!(
                lab.target_response_count(),
                before,
                "new key does not inherit old identity's grant"
            );
        },
    );
}

#[test]
fn restarting_target_discards_old_app_completion_without_contaminating_fresh_responses() {
    let storage = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(storage.clone()),
        |lab| {
            let link = lab.link(OPERATOR);
            let old_payload = RemoteControlAppMessage::from_slice(b"old-boot").expect("bounded");
            let release = lab.gate_app(OPERATOR, old_payload.as_slice());
            let old = lab.request(
                OPERATOR,
                link,
                encoded(RemoteControlRequest::AppMessage(old_payload)),
            );
            assert!(lab.settle().is_empty());
            lab.restart_target(storage.clone());
            assert!(
                release.send(()).is_err(),
                "old host future was destroyed with its boot"
            );
            lab.expect_timeout(old);
            assert!(lab.nodes[OPERATOR].handle.close_link(link));
            assert!(lab.settle().is_empty());
            let link = lab.link(OPERATOR);
            assert!(
                lab.advance(150).is_empty(),
                "fresh identification propagation"
            );
            let payload = RemoteControlAppMessage::from_slice(b"new-boot").expect("bounded");
            let request = lab.request(
                OPERATOR,
                link,
                encoded(RemoteControlRequest::AppMessage(payload.clone())),
            );
            let mut completed = lab.settle();
            for _ in 0..REQUEST_TIMEOUT_MS {
                if !completed.is_empty() {
                    break;
                }
                completed = lab.advance(1);
            }
            let (found, Event::Response(response)) = completed.remove(0) else {
                unreachable!()
            };
            assert_eq!(found, request);
            assert_eq!(
                RemoteControlResponse::parse(&response.expect("fresh response")),
                Ok(RemoteControlResponse::AppMessage(payload))
            );
            assert_eq!(
                lab.calls
                    .borrow()
                    .iter()
                    .map(|call| call.payload.clone())
                    .collect::<Vec<_>>(),
                [b"old-boot".to_vec(), b"new-boot".to_vec()]
            );
        },
    );
}
