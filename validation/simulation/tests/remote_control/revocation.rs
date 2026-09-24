use super::{durability, fixture::*};
use persistence::Storage;
use personal_rns::persistence::SnapshotRegion;
use personal_rns::remote_control::*;
use personal_rns::runtime::{
    PersistenceIoOperation, RemoteControlControllerGrantControl, StreamId,
};
use prns_simulation::{FaultPlan, ManualTaskScheduling, SimulationSeed};

#[test]
fn admitted_app_work_finishes_after_revocation_but_later_work_is_silent() {
    for seed in [0, 7, 0x5eed] {
        let run = || {
            let storage = Storage::new();
            with_storage(
                ManualTaskScheduling::Seeded {
                    seed: SimulationSeed::new(seed),
                },
                FaultPlan::none(),
                durability::policy(),
                Some(storage),
                |lab| {
                    let link = lab.link(OPERATOR);
                    let payload =
                        RemoteControlAppMessage::from_slice(b"admitted-before-revocation")
                            .expect("bounded");
                    let release = lab.gate_app(OPERATOR, payload.as_slice());
                    let admitted = lab.request(
                        OPERATOR,
                        link,
                        encoded(RemoteControlRequest::AppMessage(payload.clone())),
                    );
                    assert!(lab.settle().is_empty());
                    assert_eq!(
                        *lab.calls.borrow(),
                        [AppInvocation {
                            controller: lab.nodes[OPERATOR].identity.identity_hash(),
                            payload: payload.as_slice().to_vec()
                        }]
                    );
                    let target = lab.nodes[TARGET].handle.clone();
                    let controller = lab.nodes[OPERATOR].identity;
                    let revoke = lab.insert(async move {
                        assert!(matches!(
                            target.revoke_remote_control_controller(controller).await,
                            Ok(RevokeRemoteControlControllerOutcome::Revoked { .. })
                        ));
                        Event::Done
                    });
                    lab.expect_done(revoke);
                    release.send(()).expect("admitted app operation");
                    let completed = lab.settle();
                    assert_eq!(completed.len(), 1);
                    let (task, Event::Response(response)) = &completed[0] else {
                        unreachable!("admitted response");
                    };
                    assert_eq!(*task, admitted);
                    assert_eq!(
                        RemoteControlResponse::parse(
                            response.as_ref().expect("admitted operation may finish")
                        ),
                        Ok(RemoteControlResponse::AppMessage(payload))
                    );
                    let before = lab.target_response_count();
                    let denied = lab.request(
                        OPERATOR,
                        link,
                        encoded(RemoteControlRequest::AppMessage(
                            RemoteControlAppMessage::from_slice(b"after-revocation")
                                .expect("bounded"),
                        )),
                    );
                    lab.expect_timeout(denied);
                    assert_eq!(lab.target_response_count(), before);
                    assert_eq!(lab.calls.borrow().len(), 1);
                    lab.calls.borrow().clone()
                },
            )
        };
        assert_eq!(run(), run(), "admission boundary seed {seed}");
    }
}

#[test]
fn removing_only_watch_permission_ends_subscription_and_regrant_reuses_capacity_safely() {
    let storage = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(storage),
        |lab| {
            let link = lab.link(OPERATOR);
            let id = StreamId::new(3).expect("stream ID");
            let watch = lab.watch_from(OPERATOR, link, id);
            let read = lab.read_watch(watch);
            let completed = lab.settle();
            let (watch, event) = watch_result(read, completed);
            assert_eq!(
                event.expect("resync frame"),
                RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
            );
            let mut permissions = RemoteControlRequestSet::empty();
            for kind in requests()
                .iter()
                .filter(|kind| *kind != RemoteControlRequestKind::WatchInterfaces)
            {
                permissions.insert(kind);
            }
            let desired = grant(
                OPERATOR,
                RemoteControlControllerAuthority::Operator,
                permissions,
            );
            let target = lab.nodes[TARGET].handle.clone();
            let mutation = lab.insert(async move {
                assert!(target
                    .set_remote_control_controller_grant(desired)
                    .await
                    .is_ok());
                Event::Done
            });
            lab.expect_done(mutation);
            let read = lab.read_watch(watch);
            let completed = lab.settle();
            let (_, ended) = watch_result(read, completed);
            assert!(ended.is_err(), "permission removal sends stream EOF");
            assert!(
                matches!(
                    lab.exchange(OPERATOR, link, RemoteControlRequest::DescribeBuild),
                    RemoteControlResponse::DescribeBuild(_)
                ),
                "inspection remains authorized"
            );
            let before = lab.target_response_count();
            let denied = lab.request(
                OPERATOR,
                link,
                encoded(RemoteControlRequest::WatchInterfaces { stream_id: id }),
            );
            lab.expect_timeout(denied);
            assert_eq!(lab.target_response_count(), before);
            let target = lab.nodes[TARGET].handle.clone();
            let restored = grant(
                OPERATOR,
                RemoteControlControllerAuthority::Operator,
                requests(),
            );
            let mutation = lab.insert(async move {
                assert!(target
                    .set_remote_control_controller_grant(restored)
                    .await
                    .is_ok());
                Event::Done
            });
            lab.expect_done(mutation);
            assert!(lab.nodes[OPERATOR].handle.close_link(link));
            assert!(lab.settle().is_empty());
            let link = lab.link(OPERATOR);
            let watch = lab.watch_from(OPERATOR, link, id);
            let read = lab.read_watch(watch);
            let completed = lab.settle();
            let (_, event) = watch_result(read, completed);
            assert_eq!(
                event.expect("resync frame"),
                RemoteControlStreamEvent::ResyncRequired { sequence: 1 },
                "replacement starts independently"
            );
            assert!(lab.nodes[OPERATOR].handle.close_link(link));
            assert!(lab.settle().is_empty());
        },
    );
}

#[test]
fn whole_node_watch_driver_keeps_heartbeats_live_while_authorization_io_holds_admission() {
    let storage = Storage::new();
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(storage.clone()),
        |lab| {
            let link = lab.link(CONTROLLER);
            let watch = lab.watch(link, StreamId::new(5).expect("stream ID"));
            let read = lab.read_watch(watch);
            let completed = lab.settle();
            let (watch, initial) = watch_result(read, completed);
            assert_eq!(
                initial.expect("initial frame"),
                RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
            );
            let release = storage.io.gate(PersistenceIoOperation::AuthorizationStore(
                SnapshotRegion::RemoteControlControllerGrants,
            ));
            let target = lab.nodes[TARGET].handle.clone();
            let desired = grant(
                OPERATOR,
                RemoteControlControllerAuthority::Operator,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            );
            let mutation = lab.insert(async move {
                assert!(target
                    .set_remote_control_controller_grant(desired)
                    .await
                    .is_ok());
                Event::Done
            });
            assert!(lab.settle().is_empty());
            let waiting = lab.request(
                CONTROLLER,
                link,
                encoded(RemoteControlRequest::DescribeBuild),
            );
            lab.expect_timeout(waiting);
            let read = lab.read_watch(watch);
            assert!(lab.settle().is_empty());
            let completed = lab.advance(5_000 - REQUEST_TIMEOUT_MS);
            let (_, heartbeat) = watch_result(read, completed);
            assert_eq!(
                heartbeat.expect("heartbeat frame"),
                RemoteControlStreamEvent::Heartbeat { sequence: 2 }
            );
            release.send(()).expect("release durable operation");
            lab.expect_done(mutation);
            assert!(
                matches!(
                    lab.exchange(CONTROLLER, link, RemoteControlRequest::DescribeBuild),
                    RemoteControlResponse::DescribeBuild(_)
                ),
                "ordinary admission resumes after settlement"
            );
            assert!(lab.nodes[CONTROLLER].handle.close_link(link));
            assert!(lab.settle().is_empty());
        },
    );
}

#[test]
fn repeated_full_revocation_and_regrant_do_not_let_old_watch_cleanup_retire_fresh_subscriptions() {
    with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(Storage::new()),
        |lab| {
            for _ in 0..3 {
                let link = lab.link(OPERATOR);
                let id = StreamId::new(7).expect("reused watch ID");
                let watch = lab.watch_from(OPERATOR, link, id);
                let read = lab.read_watch(watch);
                let (old, initial) = watch_result(read, lab.settle());
                assert_eq!(
                    initial.expect("initial"),
                    RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
                );
                let target = lab.nodes[TARGET].handle.clone();
                let identity = lab.nodes[OPERATOR].identity;
                let mutation = lab.insert(async move {
                    assert!(matches!(
                        target.revoke_remote_control_controller(identity).await,
                        Ok(RevokeRemoteControlControllerOutcome::Revoked { .. })
                    ));
                    let grant = RemoteControlControllerGrant::new(
                        identity,
                        RemoteControlControllerAuthority::Operator,
                        requests(),
                    )
                    .expect("same identity");
                    target
                        .set_remote_control_controller_grant(grant)
                        .await
                        .expect("immediate regrant");
                    Event::Done
                });
                lab.expect_done(mutation);
                let read = lab.read_watch(old);
                let (_, ended) = watch_result(read, lab.settle());
                assert!(ended.is_err(), "regrant does not revive the revoked lease");
                assert!(lab.nodes[OPERATOR].handle.close_link(link));
                assert!(lab.settle().is_empty());
                let fresh = lab.link(OPERATOR);
                let watch = lab.watch_from(OPERATOR, fresh, id);
                let read = lab.read_watch(watch);
                let (watch, initial) = watch_result(read, lab.settle());
                assert_eq!(
                    initial.expect("replacement"),
                    RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
                );
                let read = lab.read_watch(watch);
                assert!(lab.settle().is_empty());
                let (_, heartbeat) = watch_result(read, lab.advance(5_000));
                assert_eq!(
                    heartbeat.expect("replacement remains live"),
                    RemoteControlStreamEvent::Heartbeat { sequence: 2 }
                );
                assert!(lab.nodes[OPERATOR].handle.close_link(fresh));
                assert!(lab.settle().is_empty());
            }
        },
    );
}
