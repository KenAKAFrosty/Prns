use super::fixture::*;
use personal_rns::remote_control::*;
use prns_simulation::{FaultPlan, ManualTaskScheduling};

#[test]
fn unlisted_controller_cannot_probe_parser_or_reach_app_handler() {
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(OUTSIDER);
        let before = lab.target_response_count();
        for bytes in [
            encoded(RemoteControlRequest::Describe),
            vec![],
            vec![0xff, 0xff],
            oversized_app_message(),
        ] {
            let task = lab.request(OUTSIDER, link, bytes);
            lab.expect_timeout(task);
            assert_eq!(
                lab.target_response_count(),
                before,
                "admission failures must not emit a response"
            );
            assert!(lab.calls.borrow().is_empty());
        }
        let trusted = lab.link(CONTROLLER);
        assert!(matches!(
            lab.exchange(CONTROLLER, trusted, RemoteControlRequest::DescribeBuild),
            RemoteControlResponse::DescribeBuild(_)
        ));
    });
}

#[test]
fn grant_intersection_limits_discovery_and_dispatch() {
    with_policy(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        ControllerPolicy::Granted(RemoteControlRequestSet::only(
            RemoteControlRequestKind::Describe,
        )),
        |lab| {
            let link = lab.link(CONTROLLER);
            let RemoteControlResponse::Describe(description) =
                lab.exchange(CONTROLLER, link, RemoteControlRequest::Describe)
            else {
                unreachable!("capability description");
            };
            assert_eq!(
                *description.available_requests(),
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe)
            );
            let before = lab.target_response_count();
            for bytes in [
                encoded(RemoteControlRequest::DescribeBuild),
                encoded(RemoteControlRequest::AppMessage(
                    RemoteControlAppMessage::from_slice(b"forbidden").expect("bounded"),
                )),
            ] {
                let task = lab.request(CONTROLLER, link, bytes);
                lab.expect_timeout(task);
                assert_eq!(lab.target_response_count(), before);
            }
            assert!(lab.calls.borrow().is_empty());
        },
    );
}

#[test]
fn nobody_policy_rejects_even_the_pinned_controller() {
    with_policy(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        ControllerPolicy::Nobody,
        |lab| {
            let link = lab.link(CONTROLLER);
            let before = lab.target_response_count();
            let task = lab.request(CONTROLLER, link, encoded(RemoteControlRequest::Describe));
            lab.expect_timeout(task);
            assert_eq!(lab.target_response_count(), before);
            assert!(lab.calls.borrow().is_empty());
        },
    );
}

#[test]
fn authorization_changes_refuse_to_become_ephemeral_without_storage() {
    use personal_rns::runtime::{
        RemoteControlControllerGrantControl, RevokeRemoteControlControllerControlError,
        SetRemoteControlControllerGrantControlError,
    };
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(CONTROLLER);
        let target = lab.nodes[TARGET].handle.clone();
        let controller = lab.nodes[CONTROLLER].identity;
        let outsider = lab.nodes[OUTSIDER].identity;
        let task = lab.insert(async move {
            assert_eq!(
                target.revoke_remote_control_controller(controller).await,
                Err(RevokeRemoteControlControllerControlError::Unavailable)
            );
            assert_eq!(
                target
                    .set_remote_control_controller_grant(
                        RemoteControlControllerGrant::new(
                            outsider,
                            RemoteControlControllerAuthority::Operator,
                            requests()
                        )
                        .expect("explicit grant")
                    )
                    .await,
                Err(SetRemoteControlControllerGrantControlError::Unavailable)
            );
            Event::Done
        });
        lab.expect_done(task);
        assert!(matches!(
            lab.exchange(CONTROLLER, link, RemoteControlRequest::DescribeBuild),
            RemoteControlResponse::DescribeBuild(_)
        ));
        let untrusted = lab.link(OUTSIDER);
        let task = lab.request(OUTSIDER, untrusted, encoded(RemoteControlRequest::Describe));
        lab.expect_timeout(task);
    });
}

pub(super) fn oversized_app_message() -> Vec<u8> {
    let mut bytes = encoded(RemoteControlRequest::AppMessage(
        RemoteControlAppMessage::from_slice(&[0; REMOTE_CONTROL_APP_MESSAGE_CAP])
            .expect("maximum payload"),
    ));
    bytes.push(0);
    bytes
}

#[test]
fn unverified_link_stays_silent_and_verified_identity_can_then_use_the_same_link() {
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link_with_identity(CONTROLLER, LinkIdentity::Unidentified);
        let before = lab.target_response_count();
        let task = lab.request(CONTROLLER, link, encoded(RemoteControlRequest::Describe));
        lab.expect_timeout(task);
        assert_eq!(lab.target_response_count(), before);
        let handle = lab.nodes[CONTROLLER].handle.clone();
        let controller = lab.nodes[CONTROLLER].identity.identity_hash();
        let task = lab.insert(async move {
            handle
                .identify(link, controller)
                .await
                .expect("authenticated identity");
            Event::Done
        });
        lab.expect_done(task);
        let payload = RemoteControlAppMessage::from_slice(b"identified").expect("bounded");
        assert_eq!(
            lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::AppMessage(payload.clone())
            ),
            RemoteControlResponse::AppMessage(payload)
        );
        assert_eq!(
            *lab.calls.borrow(),
            [AppInvocation {
                controller,
                payload: b"identified".to_vec()
            }]
        );
    });
}

#[test]
fn concurrent_untrusted_and_trusted_requests_never_cross_the_identity_boundary() {
    use prns_simulation::{ManualTaskId, SimulationSeed};
    let run = |seed| {
        with_lab(
            ManualTaskScheduling::Seeded {
                seed: SimulationSeed::new(seed),
            },
            FaultPlan::none(),
            |lab| {
                let trusted = lab.link(CONTROLLER);
                let untrusted = lab.link(OUTSIDER);
                let mut trusted_tasks: Vec<(ManualTaskId, RemoteControlAppMessage)> = Vec::new();
                let mut untrusted_tasks = Vec::new();
                for index in 0u8..6 {
                    let payload =
                        RemoteControlAppMessage::from_slice(&[index]).expect("bounded payload");
                    trusted_tasks.push((
                        lab.request(
                            CONTROLLER,
                            trusted,
                            encoded(RemoteControlRequest::AppMessage(payload.clone())),
                        ),
                        payload.clone(),
                    ));
                    untrusted_tasks.push(lab.request(
                        OUTSIDER,
                        untrusted,
                        encoded(RemoteControlRequest::AppMessage(payload)),
                    ));
                }
                let completed = lab.settle();
                assert_eq!(completed.len(), trusted_tasks.len());
                for (task, event) in completed {
                    let Event::Response(response) = event else {
                        unreachable!("trusted response");
                    };
                    let (_, payload) = trusted_tasks
                        .iter()
                        .find(|(expected, _)| *expected == task)
                        .expect("response belongs to its requester");
                    assert_eq!(
                        RemoteControlResponse::parse(&response.expect("trusted response")),
                        Ok(RemoteControlResponse::AppMessage(payload.clone()))
                    );
                }
                let completed = lab.advance(REQUEST_TIMEOUT_MS);
                assert_eq!(completed.len(), untrusted_tasks.len());
                for (task, event) in completed {
                    assert!(untrusted_tasks.contains(&task));
                    let Event::Response(response) = event else {
                        unreachable!("untrusted deadline");
                    };
                    assert_eq!(
                        response,
                        Err(personal_rns::runtime::SendError::Failed(
                            personal_rns::engine::SendRequestFailure::Timeout
                        ))
                    );
                }
                let mut calls = lab.calls.borrow().clone();
                assert_eq!(calls.len(), 6);
                for call in &calls {
                    assert_eq!(
                        call.controller,
                        lab.nodes[CONTROLLER].identity.identity_hash()
                    );
                }
                calls.sort_by(|left, right| left.payload.cmp(&right.payload));
                calls
            },
        )
    };
    for seed in [0, 7, 0x5eed] {
        let first = run(seed);
        assert_eq!(
            first,
            run(seed),
            "concurrent control packets must replay for seed {seed}"
        );
        assert_eq!(
            first,
            run(seed),
            "concurrent control packets must replay for seed {seed}"
        );
    }
}
