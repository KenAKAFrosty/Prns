use super::{durability, fixture::*};
use persistence::Storage;
use personal_rns::engine::RequestResponseTimeout;
use personal_rns::remote_control::*;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{RemoteControlControllerGrantControl, StreamId};
use personal_rns::units::DurationMillis;
use personal_rns::wire::{WireContext, WirePacketHeader};
use prns_simulation::*;

fn resource(lab: &mut Lab<'_>, link: personal_rns::routing::links::LinkId) {
    let handle = lab.nodes[OPERATOR].handle.clone();
    let task = lab.insert(async move {
        Event::Response(
            handle
                .request_with_response_timeout(
                    link,
                    RequestPathHash::of("/qualification-resource"),
                    b"",
                    RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT_MS)),
                )
                .await
                .map(|(bytes, _)| bytes),
        )
    });
    let mut completed = lab.settle();
    assert_eq!(
        completed.len(),
        1,
        "controller received the Resource while sender awaits its proof"
    );
    let (found, Event::Response(reply)) = completed.remove(0) else {
        unreachable!("resource")
    };
    assert_eq!(found, task);
    assert_eq!(reply.expect("large app response"), resource_body());
}
#[test]
fn revoked_pending_watch_is_silent_and_its_late_completion_does_not_cancel_same_link_replacement() {
    let (_, baseline) = with_storage(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        Some(Storage::new()),
        |lab| {
            let link = lab.link(OPERATOR);
            resource(lab, link);
        },
    );
    let proof = baseline
        .iter()
        .find_map(|event| match event {
            MediumEvent::TransmissionAccepted { ordinal, frame, .. }
                if WirePacketHeader::parse(frame)
                    .is_ok_and(|(header, _)| header.context == WireContext::ResourceProof) =>
            {
                Some(*ordinal)
            }
            _ => None,
        })
        .expect("Resource sender completion boundary");
    let plan = FaultPlan::new(vec![TransmissionRule::delay(
        proof,
        SimulationDurationInTicks::from_ticks(300),
    )])
    .expect("bounded proof delay");
    with_storage(
        ManualTaskScheduling::Cyclic,
        plan,
        durability::policy(),
        Some(Storage::new()),
        |lab| {
            let link = lab.link(OPERATOR);
            resource(lab, link);
            let id = StreamId::new(4).expect("same stream ID");
            let old = lab.request(
                OPERATOR,
                link,
                encoded(RemoteControlRequest::WatchInterfaces { stream_id: id }),
            );
            assert!(
                lab.settle().is_empty(),
                "watch waits behind Resource response lane"
            );
            let before = lab.target_response_count();
            let target = lab.nodes[TARGET].handle.clone();
            let identity = lab.nodes[OPERATOR].identity;
            let revoke = lab.insert(async move {
                target
                    .revoke_remote_control_controller(identity)
                    .await
                    .expect("revoke pending watch");
                let grant = RemoteControlControllerGrant::new(
                    identity,
                    RemoteControlControllerAuthority::Operator,
                    requests(),
                )
                .expect("regrant");
                target
                    .set_remote_control_controller_grant(grant)
                    .await
                    .expect("restore same controller");
                Event::Done
            });
            lab.expect_done(revoke);
            lab.expect_timeout(old);
            let controller = lab.nodes[OPERATOR].handle.clone();
            let replacement = lab.insert(async move {
                let (watch, _) = controller
                    .remote_control(link)
                    .watch_interfaces(id)
                    .await
                    .expect("replacement watch");
                Event::WatchOpened(watch)
            });
            assert!(lab.settle().is_empty());
            let mut completed = lab.advance(250);
            assert_eq!(
                completed.len(),
                1,
                "only replacement response after delayed proof"
            );
            assert_eq!(
                lab.target_response_count(),
                before + 1,
                "withdrawn pending watch emitted no reply"
            );
            let (found, Event::WatchOpened(watch)) = completed.remove(0) else {
                unreachable!("watch")
            };
            assert_eq!(found, replacement);
            let read = lab.read_watch(watch);
            let (watch, initial) = watch_result(read, lab.settle());
            assert_eq!(
                initial.expect("fresh lease"),
                RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
            );
            let read = lab.read_watch(watch);
            assert!(lab.settle().is_empty());
            let (_, heartbeat) = watch_result(read, lab.advance(5_000));
            assert_eq!(
                heartbeat.expect("late old cleanup did not retire replacement"),
                RemoteControlStreamEvent::Heartbeat { sequence: 2 }
            );
            assert!(lab.nodes[OPERATOR].handle.close_link(link));
            assert!(lab.settle().is_empty());
        },
    );
}
