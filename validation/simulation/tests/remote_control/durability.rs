use super::fixture::*;
use persistence::{Effect, IoEvent, Storage};
use personal_rns::persistence::SnapshotRegion;
use personal_rns::remote_control::*;
use personal_rns::runtime::{PersistenceIoOperation, RemoteControlControllerGrantControl};
use prns_simulation::{FaultPlan, ManualTaskId, ManualTaskScheduling};

fn administrator() -> RemoteControlControllerGrant {
    grant(
        CONTROLLER,
        RemoteControlControllerAuthority::Administrator,
        management_requests(),
    )
}

fn operator() -> RemoteControlControllerGrant {
    grant(
        OPERATOR,
        RemoteControlControllerAuthority::Operator,
        requests(),
    )
}

pub(super) fn policy() -> ControllerPolicy {
    ControllerPolicy::Grants(vec![administrator(), operator()])
}

fn sorted(mut grants: Vec<RemoteControlControllerGrant>) -> Vec<RemoteControlControllerGrant> {
    grants.sort_by_key(|grant| *grant.controller().identity_hash().as_bytes());
    grants
}

fn set(lab: &mut Lab<'_>, desired: RemoteControlControllerGrant) -> ManualTaskId {
    let target = lab.nodes[TARGET].handle.clone();
    lab.insert(async move {
        assert!(target
            .set_remote_control_controller_grant(desired)
            .await
            .is_ok());
        Event::Done
    })
}

#[test]
fn authority_activates_after_confirmed_storage_and_reboots_restore_the_committed_table() {
    let run = || {
        let storage = Storage::new();
        with_storage(
            ManualTaskScheduling::Cyclic,
            FaultPlan::none(),
            policy(),
            Some(storage.clone()),
            |lab| {
                let initial = sorted(vec![administrator(), operator()]);
                assert_eq!(storage.grants(), initial);
                let desired = grant(
                    OPERATOR,
                    RemoteControlControllerAuthority::Operator,
                    RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
                );
                let release = storage.io.gate(PersistenceIoOperation::AuthorizationStore(
                    SnapshotRegion::RemoteControlControllerGrants,
                ));
                let task = set(lab, desired);
                assert!(lab.settle().is_empty());
                assert_eq!(
                    storage.grants(),
                    initial,
                    "preparation leaves durable authority unchanged"
                );
                assert!(storage.io.events().contains(&IoEvent::Started(
                    PersistenceIoOperation::AuthorizationStore(
                        SnapshotRegion::RemoteControlControllerGrants
                    )
                )));
                release.send(()).expect("pending storage actor");
                lab.expect_done(task);
                let committed = sorted(vec![administrator(), desired]);
                assert_eq!(storage.grants(), committed);
                for _ in 0..2 {
                    lab.restart_target(storage.clone());
                    let link = lab.link(OPERATOR);
                    assert!(matches!(
                        lab.exchange(OPERATOR, link, RemoteControlRequest::Describe),
                        RemoteControlResponse::Describe(_)
                    ));
                    let before = lab.target_response_count();
                    let task =
                        lab.request(OPERATOR, link, encoded(RemoteControlRequest::DescribeBuild));
                    lab.expect_timeout(task);
                    assert_eq!(lab.target_response_count(), before);
                    assert_eq!(storage.grants(), committed);
                }
                storage.io.events()
            },
        )
    };
    assert_eq!(run(), run());
}

#[test]
fn unconfirmed_publication_retries_without_rewriting_and_canceled_caller_cannot_undo_commit() {
    let storage = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        policy(),
        Some(storage.clone()),
        |lab| {
            let region = SnapshotRegion::RemoteControlControllerGrants;
            let desired = grant(
                OPERATOR,
                RemoteControlControllerAuthority::Operator,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            );
            storage.io.script(
                region,
                [
                    Effect::PublishedUnconfirmed,
                    Effect::ConfirmationFailed,
                    Effect::ConfirmationMissing,
                    Effect::ConfirmationDifferent,
                ],
            );
            let task = set(lab, desired);
            assert!(lab.settle().is_empty());
            assert_eq!(
                storage.grants(),
                sorted(vec![administrator(), desired]),
                "publication precedes confirmation"
            );
            lab.runner.cancel(task).expect("cancel caller only");
            for _ in 0..3 {
                assert!(lab.advance(249).is_empty());
                assert!(lab.advance(1).is_empty());
            }
            assert_eq!(storage.io.remaining_effects(), 0);
            assert!(lab.advance(250).is_empty());
            let events = storage.io.events();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| **event
                        == IoEvent::Started(PersistenceIoOperation::AuthorizationStore(region)))
                    .count(),
                1,
                "confirmation never rewrites a published candidate"
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| **event
                        == IoEvent::Started(PersistenceIoOperation::AuthorizationConfirm(region)))
                    .count(),
                4
            );
            assert!(events.contains(&IoEvent::Finished(
                PersistenceIoOperation::AuthorizationFinish
            )));
            lab.restart_target(storage.clone());
            assert_eq!(storage.grants(), sorted(vec![administrator(), desired]));
            let link = lab.link(OPERATOR);
            assert!(matches!(
                lab.exchange(OPERATOR, link, RemoteControlRequest::Describe),
                RemoteControlResponse::Describe(_)
            ));
        },
    );
}

#[test]
fn failed_write_persists_rollback_before_reporting_failure_and_releases_authority_owner() {
    let storage = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        policy(),
        Some(storage.clone()),
        |lab| {
            let region = SnapshotRegion::RemoteControlControllerGrants;
            storage.io.script(region, [Effect::WriteFailed]);
            let target = lab.nodes[TARGET].handle.clone();
            let desired = grant(
                OPERATOR,
                RemoteControlControllerAuthority::Operator,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            );
            let task = lab.insert(async move {
            assert_eq!(target.set_remote_control_controller_grant(desired).await, Err(personal_rns::runtime::SetRemoteControlControllerGrantControlError::Unavailable));
            Event::Done
        });
            lab.expect_done(task);
            assert_eq!(storage.grants(), sorted(vec![administrator(), operator()]));
            assert_eq!(
                storage
                    .io
                    .events()
                    .iter()
                    .filter(|event| **event
                        == IoEvent::Finished(PersistenceIoOperation::AuthorizationStore(region)))
                    .count(),
                2,
                "failed candidate followed by durable rollback"
            );
            let task = set(lab, desired);
            lab.expect_done(task);
            lab.restart_target(storage.clone());
            assert_eq!(storage.grants(), sorted(vec![administrator(), desired]));
        },
    );
}
