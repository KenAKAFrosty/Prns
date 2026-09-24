use super::{durability, fixture::*};
use persistence::{Effect, IoEvent, Storage};
use personal_rns::persistence::SnapshotRegion;
use personal_rns::remote_control::*;
use personal_rns::runtime::{PersistenceIoOperation, RemoteControlControllerGrantControl};
use prns_simulation::{FaultPlan, ManualTaskScheduling, SimulationSeed};

fn authorize(
    lab: &mut Lab<'_>,
    link: personal_rns::routing::links::LinkId,
    index: usize,
    permitted_requests: RemoteControlRequestSet,
) -> RemoteControlResponse {
    lab.exchange(
        CONTROLLER,
        link,
        RemoteControlRequest::AuthorizeController {
            controller: *grant(
                index,
                RemoteControlControllerAuthority::Operator,
                permitted_requests,
            )
            .controller(),
            permitted_requests,
        },
    )
}

#[test]
fn remote_management_commits_revoke_and_regrant_with_exact_discovery_and_silent_denial() {
    for seed in [0, 7, 0x5eed] {
        let run = || {
            let storage = Storage::new();
            with_storage(
                ManualTaskScheduling::Seeded {
                    seed: SimulationSeed::new(seed),
                },
                FaultPlan::none(),
                durability::policy(),
                Some(storage.clone()),
                |lab| {
                    let admin = lab.link(CONTROLLER);
                    let operator = lab.link(OPERATOR);
                    assert_eq!(
                        authorize(lab, admin, OPERATOR, requests()),
                        RemoteControlResponse::AuthorizeController(
                            RemoteControlAuthorizeControllerOutcome::Applied
                        )
                    );
                    let stores = storage
                        .io
                        .events()
                        .iter()
                        .filter(|event| {
                            matches!(
                                event,
                                IoEvent::Started(PersistenceIoOperation::AuthorizationStore(_))
                            )
                        })
                        .count();
                    assert_eq!(
                        stores, 0,
                        "unchanged authorization does not rewrite storage"
                    );
                    let hash = lab.nodes[OPERATOR].identity.identity_hash();
                    assert_eq!(
                        lab.exchange(
                            CONTROLLER,
                            admin,
                            RemoteControlRequest::RevokeController { hash }
                        ),
                        RemoteControlResponse::RevokeController(
                            RemoteControlRevokeControllerOutcome::Applied
                        )
                    );
                    assert_eq!(storage.grants().len(), 1);
                    let before = lab.target_response_count();
                    let task =
                        lab.request(OPERATOR, operator, encoded(RemoteControlRequest::Describe));
                    lab.expect_timeout(task);
                    assert_eq!(lab.target_response_count(), before);
                    assert_eq!(
                        lab.exchange(
                            CONTROLLER,
                            admin,
                            RemoteControlRequest::RevokeController { hash }
                        ),
                        RemoteControlResponse::RevokeController(
                            RemoteControlRevokeControllerOutcome::NotFound
                        )
                    );
                    let limited = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
                    assert_eq!(
                        authorize(lab, admin, OPERATOR, limited),
                        RemoteControlResponse::AuthorizeController(
                            RemoteControlAuthorizeControllerOutcome::Applied
                        )
                    );
                    let RemoteControlResponse::Describe(description) =
                        lab.exchange(OPERATOR, operator, RemoteControlRequest::Describe)
                    else {
                        unreachable!("operator description");
                    };
                    assert_eq!(*description.available_requests(), limited);
                    let before = lab.target_response_count();
                    let task = lab.request(
                        OPERATOR,
                        operator,
                        encoded(RemoteControlRequest::DescribeBuild),
                    );
                    lab.expect_timeout(task);
                    assert_eq!(lab.target_response_count(), before);
                    lab.restart_target(storage.clone());
                    let operator = lab.link(OPERATOR);
                    assert!(matches!(
                        lab.exchange(OPERATOR, operator, RemoteControlRequest::Describe),
                        RemoteControlResponse::Describe(_)
                    ));
                    storage.grants()
                },
            )
        };
        assert_eq!(run(), run(), "durable management replay seed {seed}");
    }
}

#[test]
fn privileged_management_rejects_operator_and_protects_administrators_without_mutation() {
    let storage = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(storage.clone()),
        |lab| {
            let admin = lab.link(CONTROLLER);
            let operator = lab.link(OPERATOR);
            let initial = storage.grants();
            let before = lab.target_response_count();
            let task = lab.request(
                OPERATOR,
                operator,
                encoded(RemoteControlRequest::RevokeController {
                    hash: lab.nodes[CONTROLLER].identity.identity_hash(),
                }),
            );
            lab.expect_timeout(task);
            assert_eq!(
                lab.target_response_count(),
                before,
                "operator cannot enter administrative parser/dispatch"
            );
            assert_eq!(
                lab.exchange(
                    CONTROLLER,
                    admin,
                    RemoteControlRequest::RevokeController {
                        hash: lab.nodes[CONTROLLER].identity.identity_hash()
                    }
                ),
                RemoteControlResponse::RevokeController(
                    RemoteControlRevokeControllerOutcome::Forbidden
                )
            );
            assert_eq!(
                authorize(lab, admin, CONTROLLER, requests()),
                RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::Forbidden
                )
            );
            assert_eq!(storage.grants(), initial);
            assert!(!storage.io.events().iter().any(|event| matches!(
                event,
                IoEvent::Started(PersistenceIoOperation::AuthorizationStore(_))
            )));
        },
    );
}

#[test]
fn remote_capacity_and_unknown_revocation_preserve_the_complete_table() {
    let storage = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(storage.clone()),
        |lab| {
            let admin = lab.link(CONTROLLER);
            for index in 4..4 + DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS - 2 {
                assert_eq!(
                    authorize(lab, admin, index, requests()),
                    RemoteControlResponse::AuthorizeController(
                        RemoteControlAuthorizeControllerOutcome::Applied
                    )
                );
            }
            let complete = storage.grants();
            assert_eq!(complete.len(), DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS);
            assert_eq!(
                authorize(lab, admin, OUTSIDER, requests()),
                RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::CapacityExhausted
                )
            );
            let absent =
                *grant(12, RemoteControlControllerAuthority::Operator, requests()).controller();
            assert_eq!(
                lab.exchange(
                    CONTROLLER,
                    admin,
                    RemoteControlRequest::RevokeController {
                        hash: absent.identity_hash()
                    }
                ),
                RemoteControlResponse::RevokeController(
                    RemoteControlRevokeControllerOutcome::NotFound
                )
            );
            assert_eq!(storage.grants(), complete);
            for _ in 0..2 {
                lab.restart_target(storage.clone());
                assert_eq!(storage.grants(), complete);
            }
        },
    );
}

#[derive(Debug)]
enum CutBoundary {
    Begin,
    Store,
    Confirm,
    Finish,
}

#[test]
fn abrupt_boot_removal_recovers_only_previously_stored_or_complete_candidate_authority() {
    for boundary in [
        CutBoundary::Begin,
        CutBoundary::Store,
        CutBoundary::Confirm,
        CutBoundary::Finish,
    ] {
        let storage = Storage::new();
        with_storage(
            ManualTaskScheduling::Cyclic,
            FaultPlan::none(),
            durability::policy(),
            Some(storage.clone()),
            |lab| {
                let prior = storage.grants();
                let desired = grant(
                    OPERATOR,
                    RemoteControlControllerAuthority::Operator,
                    RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
                );
                let operation = match boundary {
                    CutBoundary::Begin => PersistenceIoOperation::AuthorizationBegin,
                    CutBoundary::Store => PersistenceIoOperation::AuthorizationStore(
                        SnapshotRegion::RemoteControlControllerGrants,
                    ),
                    CutBoundary::Confirm => {
                        storage.io.script(
                            SnapshotRegion::RemoteControlControllerGrants,
                            [Effect::PublishedUnconfirmed],
                        );
                        PersistenceIoOperation::AuthorizationConfirm(
                            SnapshotRegion::RemoteControlControllerGrants,
                        )
                    }
                    CutBoundary::Finish => PersistenceIoOperation::AuthorizationFinish,
                };
                let release = storage.io.gate(operation.clone());
                let target = lab.nodes[TARGET].handle.clone();
                let task = lab.insert(async move {
                    let _outcome = target.set_remote_control_controller_grant(desired).await;
                    Event::Done
                });
                let completed = lab.settle();
                match boundary {
                    CutBoundary::Finish => assert_eq!(
                        completed.len(),
                        1,
                        "caller completion can precede owner release"
                    ),
                    CutBoundary::Begin | CutBoundary::Store | CutBoundary::Confirm => {
                        assert!(completed.is_empty())
                    }
                }
                if matches!(boundary, CutBoundary::Confirm) {
                    assert!(lab.advance(250).is_empty());
                }
                assert!(storage.io.events().contains(&IoEvent::Started(operation)));
                if !matches!(boundary, CutBoundary::Finish) {
                    lab.runner.cancel(task).expect("remove abandoned caller");
                }
                let expected = match boundary {
                    CutBoundary::Begin | CutBoundary::Store => prior,
                    CutBoundary::Confirm | CutBoundary::Finish => {
                        let mut candidate = prior;
                        let old = candidate
                            .iter_mut()
                            .find(|grant| grant.controller() == desired.controller())
                            .expect("existing operator");
                        *old = desired;
                        candidate
                    }
                };
                for _ in 0..2 {
                    lab.restart_target(storage.clone());
                    assert_eq!(storage.grants(), expected, "{boundary:?}");
                }
                assert!(
                    release.send(()).is_err(),
                    "old boot no longer owns I/O continuation"
                );
            },
        );
    }
}

#[test]
fn lost_remote_success_response_does_not_rollback_committed_authority() {
    use personal_rns::wire::{WireContext, WirePacketHeader};
    use prns_simulation::{MediumEvent, TransmissionRule};
    let baseline_storage = Storage::new();
    let (_, baseline) = with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(baseline_storage),
        |lab| {
            let admin = lab.link(CONTROLLER);
            assert_eq!(
                authorize(lab, admin, OUTSIDER, requests()),
                RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::Applied
                )
            );
        },
    );
    let ordinal = baseline
        .iter()
        .rev()
        .find_map(|event| match event {
            MediumEvent::TransmissionAccepted { ordinal, frame, .. }
                if WirePacketHeader::parse(frame)
                    .is_ok_and(|(header, _)| header.context == WireContext::Response) =>
            {
                Some(*ordinal)
            }
            _ => None,
        })
        .expect("management response boundary");
    let storage = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::new(vec![TransmissionRule::drop(ordinal)]).expect("response loss"),
        durability::policy(),
        Some(storage.clone()),
        |lab| {
            let admin = lab.link(CONTROLLER);
            let controller = lab.nodes[OUTSIDER].identity;
            let task = lab.request(
                CONTROLLER,
                admin,
                encoded(RemoteControlRequest::AuthorizeController {
                    controller,
                    permitted_requests: requests(),
                }),
            );
            lab.expect_timeout(task);
            assert!(storage
                .grants()
                .iter()
                .any(|grant| grant.controller() == &controller));
            for _ in 0..2 {
                lab.restart_target(storage.clone());
                let link = lab.link(OUTSIDER);
                assert!(matches!(
                    lab.exchange(OUTSIDER, link, RemoteControlRequest::DescribeBuild),
                    RemoteControlResponse::DescribeBuild(_)
                ));
            }
        },
    );
}

#[test]
fn rollback_failure_stops_the_target_and_fresh_boot_restores_confirmed_authority() {
    use personal_rns::runtime::{
        NodeRunError, RemoteControlAuthorizationPersistenceFailure,
        SetRemoteControlControllerGrantControlError,
    };
    let storage = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(storage.clone()),
        |lab| {
            let initial = storage.grants();
            storage.io.script(
                SnapshotRegion::RemoteControlControllerGrants,
                [Effect::WriteFailed, Effect::WriteFailed],
            );
            let handle = lab.nodes[TARGET].handle.clone();
            let desired = grant(
                OPERATOR,
                RemoteControlControllerAuthority::Operator,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            );
            let caller = lab.insert(async move {
                assert_eq!(
                    handle.set_remote_control_controller_grant(desired).await,
                    Err(SetRemoteControlControllerGrantControlError::NodeStopped)
                );
                Event::Done
            });
            let completed = lab.settle();
            assert_eq!(completed.len(), 2);
            assert!(completed
                .iter()
                .any(|(task, event)| *task == caller && matches!(event, Event::Done)));
            assert!(completed.iter().any(|(_, event)| matches!(
                event,
                Event::Stopped {
                    node: TARGET,
                    result: Err(NodeRunError::RemoteControlAuthorizationPersistenceFailed(
                        RemoteControlAuthorizationPersistenceFailure::DurableRollback
                    ))
                }
            )));
            assert_eq!(storage.io.remaining_effects(), 0);
            for _ in 0..2 {
                lab.restart_target(storage.clone());
                assert_eq!(storage.grants(), initial);
                let link = lab.link(OPERATOR);
                assert!(matches!(
                    lab.exchange(OPERATOR, link, RemoteControlRequest::DescribeBuild),
                    RemoteControlResponse::DescribeBuild(_)
                ));
            }
        },
    );
}
