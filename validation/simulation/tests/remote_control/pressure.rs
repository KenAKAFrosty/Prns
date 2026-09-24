use super::{durability, fixture::*};
use personal_rns::remote_control::*;
use personal_rns::runtime::StreamId;
use prns_simulation::{FaultPlan, ManualTaskScheduling};

const EXPECTED_IN_FLIGHT: usize = 256;
const EXPECTED_QUEUE_DEPTH: usize = 1024;
const REQUEST_COUNT: usize = EXPECTED_IN_FLIGHT + EXPECTED_QUEUE_DEPTH + 2;

#[test]
fn two_verified_controllers_saturate_request_capacity_without_crossing_response_or_identity_ownership(
) {
    with_budgets(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        None,
        FixtureBudgets::pressure(),
        |lab| {
            let admin = lab.link(CONTROLLER);
            let operator = lab.link(OPERATOR);
            let mut pending = Vec::new();
            let mut releases = Vec::new();
            for ordinal in 0..REQUEST_COUNT {
                let (controller, link) = if ordinal.is_multiple_of(2) {
                    (CONTROLLER, admin)
                } else {
                    (OPERATOR, operator)
                };
                let payload = RemoteControlAppMessage::from_slice(&(ordinal as u32).to_be_bytes())
                    .expect("bounded distinct request");
                releases.push(lab.gate_app(controller, payload.as_slice()));
                pending.push((
                    lab.request(
                        controller,
                        link,
                        encoded(RemoteControlRequest::AppMessage(payload.clone())),
                    ),
                    controller,
                    payload,
                ));
            }
            assert!(lab.settle().is_empty());
            assert_eq!(
                lab.calls.borrow().len(),
                EXPECTED_IN_FLIGHT,
                "active handler admission is bounded before releasing any handler"
            );
            for release in releases {
                release.send(()).expect("fixture gate remains owned");
            }
            let completed = lab.settle();
            assert_eq!(completed.len(), EXPECTED_IN_FLIGHT + EXPECTED_QUEUE_DEPTH);
            let mut expected_calls = Vec::new();
            for (task, event) in completed {
                let (_, controller, payload) = pending
                    .iter()
                    .find(|(expected, _, _)| *expected == task)
                    .expect("request owner");
                let Event::Response(response) = event else {
                    unreachable!("application response");
                };
                assert_eq!(
                    RemoteControlResponse::parse(&response.expect("released handler response")),
                    Ok(RemoteControlResponse::AppMessage(payload.clone()))
                );
                expected_calls.push(AppInvocation {
                    controller: lab.nodes[*controller].identity.identity_hash(),
                    payload: payload.as_slice().to_vec(),
                });
            }
            let mut actual = lab.calls.borrow().clone();
            expected_calls.sort_by(|left, right| left.payload.cmp(&right.payload));
            actual.sort_by(|left, right| left.payload.cmp(&right.payload));
            assert_eq!(actual, expected_calls);
            let overflow = lab.advance(REQUEST_TIMEOUT_MS);
            assert_eq!(
                overflow.len(),
                2,
                "requests beyond both router capacities expire without handler execution"
            );
            for (_, event) in overflow {
                let Event::Response(response) = event else {
                    unreachable!("overflow deadline");
                };
                assert_eq!(
                    response,
                    Err(personal_rns::runtime::SendError::Failed(
                        personal_rns::engine::SendRequestFailure::Timeout
                    ))
                );
            }
            for (controller, link) in [(CONTROLLER, admin), (OPERATOR, operator)] {
                assert!(
                    matches!(
                        lab.exchange(controller, link, RemoteControlRequest::DescribeBuild),
                        RemoteControlResponse::DescribeBuild(_)
                    ),
                    "fresh traffic after capacity relief"
                );
            }
            assert_eq!(lab.runner.task_count(), lab.nodes.len());
        },
    );
}

#[test]
fn a_slow_watch_reader_overflows_explicitly_while_another_controller_keeps_receiving() {
    with_budgets(
        ManualTaskScheduling::Cyclic,
        FaultPlan::none(),
        durability::policy(),
        None,
        FixtureBudgets::pressure(),
        |lab| {
            let admin = lab.link(CONTROLLER);
            let operator = lab.link(OPERATOR);
            let healthy = lab.watch_from(CONTROLLER, admin, StreamId::new(1).expect("ID"));
            let slow = lab.watch_from(OPERATOR, operator, StreamId::new(2).expect("ID"));
            let read = lab.read_watch(healthy);
            let completed = lab.settle();
            let (mut healthy, initial) = watch_result(read, completed);
            assert_eq!(
                initial.expect("healthy initial"),
                RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
            );
            for sequence in 2..=35 {
                let read = lab.read_watch(healthy);
                assert!(lab.settle().is_empty());
                let completed = lab.advance(5_000);
                let (next, heartbeat) = watch_result(read, completed);
                healthy = next;
                assert_eq!(
                    heartbeat.expect("healthy heartbeat"),
                    RemoteControlStreamEvent::Heartbeat { sequence }
                );
            }
            let read = lab.read_watch(slow);
            let completed = lab.settle();
            let (_, error) = watch_result(read, completed);
            let Err(personal_rns::runtime::RemoteControlWatchReadError::Io(error)) = error else {
                unreachable!("typed stream overflow");
            };
            assert!(error.get_ref().is_some_and(|error| {
                error.downcast_ref::<personal_rns::manifold::tokio::StreamReceiveFailure>()
                    == Some(&personal_rns::manifold::tokio::StreamReceiveFailure::Overflowed)
            }));
            assert!(lab.nodes[OPERATOR].handle.close_link(operator));
            assert!(lab.settle().is_empty());
            let fresh = lab.link(OPERATOR);
            let replacement = lab.watch_from(
                OPERATOR,
                fresh,
                StreamId::new(2).expect("same ID on new link"),
            );
            let read = lab.read_watch(replacement);
            let completed = lab.settle();
            let (_, initial) = watch_result(read, completed);
            assert_eq!(
                initial.expect("replacement initial"),
                RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
            );
            assert!(
                matches!(
                    lab.exchange(
                        OPERATOR,
                        fresh,
                        RemoteControlRequest::InventoryInterfaces {
                            page: RemoteControlInterfacePage::First
                        }
                    ),
                    RemoteControlResponse::InventoryInterfaces(_)
                ),
                "inventory refetch succeeds after reader overflow and reconnect"
            );
            drop(healthy);
            for (controller, link) in [(CONTROLLER, admin), (OPERATOR, fresh)] {
                assert!(lab.nodes[controller].handle.close_link(link));
            }
            assert!(lab.settle().is_empty());
        },
    );
}
