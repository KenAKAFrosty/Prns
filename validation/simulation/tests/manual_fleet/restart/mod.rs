use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use personal_rns::engine::{RequestResponseTimeout, SendRequestFailure};
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::SendError;
use personal_rns::units::DurationMillis;
use personal_rns::wire::{WireContext, WirePacketHeader};
use prns_simulation::{
    DeliveryCopy, FaultPlan, ManualMedium, ManualTaskCancellation, ManualTaskPoll,
    ManualTaskRunner, ManualTimeDriver, MediumEvent, Reachability, ReceptionDropReason,
    SimulationDurationInTicks, SimulationTick, TopologyConfig, TopologyMutation,
    TransmissionOrdinal, TransmissionRule, VirtualMedium, VirtualMediumConfig,
};

use super::scenario::{
    add_node, announce, destination, nonzero, request, settle, Completion, NodeControl, NodeRole,
    NodeSpec, POLL_BUDGET, QUERY_PATH,
};

mod ble;
mod churn;
mod fixture;
mod replay;
mod transport;
use fixture::{connect, destination_hash, echo, link, restart, with_fleet};

const RESTARTED: usize = 1;
const REQUEST_TIMEOUT_MILLIS: u64 = 50;

fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

#[test]
fn delayed_frames_cannot_cross_node_incarnations_with_the_same_interface_identity() {
    const DELAY_MILLIS: u64 = 5;
    let faults = FaultPlan::new(vec![TransmissionRule::delay(
        TransmissionOrdinal::new(0),
        SimulationDurationInTicks::from_ticks(DELAY_MILLIS),
    )])
    .unwrap_or_else(|error| unreachable!("single delayed announce: {error}"));
    with_fleet(faults, |runner, medium, nodes| {
        announce(&nodes[0].control, destination_hash(0));
        assert!(settle(runner).is_empty());
        assert_eq!(medium.pending_delivery_count(), 1);
        assert!(nodes[RESTARTED].heard.borrow().is_empty());
        let old_endpoint = nodes[RESTARTED].endpoint;
        restart(runner, medium, nodes, RESTARTED);
        connect(medium, nodes, 0, RESTARTED);
        assert_eq!(
            medium.pending_delivery_count(),
            1,
            "in-flight traffic keeps its original receiver"
        );

        let survivor = link(runner, nodes, 2, 3);
        echo(
            runner,
            nodes,
            2,
            survivor,
            b"survivor while old frame is pending",
        );
        assert!(runner.advance_to_next_event(tick(DELAY_MILLIS)).is_ok());
        assert!(settle(runner).is_empty());
        assert!(nodes[RESTARTED].heard.borrow().is_empty());
        assert_eq!(medium.pending_delivery_count(), 0);
        let dropped: Vec<_> = medium
            .trace()
            .events
            .into_iter()
            .filter(|event| matches!(event, MediumEvent::ReceptionDropped { .. }))
            .collect();
        assert_eq!(
            dropped,
            [MediumEvent::ReceptionDropped {
                ordinal: TransmissionOrdinal::new(0),
                to: old_endpoint,
                copy: DeliveryCopy::Original,
                at: tick(DELAY_MILLIS),
                intended_for: tick(DELAY_MILLIS),
                reason: ReceptionDropReason::EndpointClosed,
            }]
        );

        announce(&nodes[0].control, destination_hash(0));
        assert!(settle(runner).is_empty());
        assert_eq!(*nodes[RESTARTED].heard.borrow(), [destination_hash(0)]);
        let recovered = link(runner, nodes, 0, RESTARTED);
        echo(runner, nodes, 0, recovered, b"fresh incarnation");
        echo(runner, nodes, 2, survivor, b"same unaffected link");
    });
}

#[test]
fn selective_node_restart_loses_an_inflight_request_but_preserves_other_nodes_and_clock() {
    with_fleet(FaultPlan::none(), |runner, medium, nodes| {
        let original = link(runner, nodes, 0, RESTARTED);
        let survivor = link(runner, nodes, 2, 3);
        echo(runner, nodes, 0, original, b"before teardown");
        let history_start = medium.inspect_trace(|trace| trace.events().len());
        let handle = nodes[0].control.handle.clone();
        let request_task = runner
            .insert(async move {
                assert_eq!(
                    handle
                        .request_with_response_timeout(
                            original,
                            RequestPathHash::of(QUERY_PATH),
                            b"receiver will be torn down",
                            RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT_MILLIS)),
                        )
                        .await,
                    Err(SendError::Failed(SendRequestFailure::Timeout))
                );
                Completion::TimedOut { node: 0 }
            })
            .unwrap_or_else(|error| unreachable!("pending request actor: {error}"));
        let source = nodes[0].endpoint;
        let mut sent = false;
        for _ in 0..POLL_BUDGET {
            assert!(matches!(
                runner.poll_next().ok(),
                Some(ManualTaskPoll::Pending { .. })
            ));
            sent = medium.inspect_trace(|trace| {
                assert_eq!(trace.discarded_events, 0);
                trace.events().skip(history_start).any(|event| {
                    let MediumEvent::TransmissionAccepted { from, frame, .. } = event else {
                        return false;
                    };
                    *from == source
                        && WirePacketHeader::parse(frame).is_ok_and(|(header, _)| {
                            header.context == WireContext::Request
                                && header.address == original.to_address()
                        })
                })
            });
            if sent {
                break;
            }
        }
        assert!(
            sent,
            "stop at the real send boundary before the receiver's next poll"
        );
        let before = runner.snapshot().ok();
        let old_task = nodes[RESTARTED].task;
        assert_eq!(
            runner.cancel(old_task).ok(),
            Some(ManualTaskCancellation::Cancelled)
        );
        assert_eq!(runner.snapshot().ok(), before);
        assert!(
            settle(runner).is_empty(),
            "teardown must not invent a response or graceful stop"
        );
        echo(
            runner,
            nodes,
            2,
            survivor,
            b"unaffected during receiver downtime",
        );

        for elapsed in 1..REQUEST_TIMEOUT_MILLIS {
            assert!(runner.advance_to_next_event(tick(elapsed)).is_ok());
            assert!(
                settle(runner).is_empty(),
                "no early completion after receiver loss"
            );
        }
        assert!(runner
            .advance_to_next_event(tick(REQUEST_TIMEOUT_MILLIS))
            .is_ok());
        assert_eq!(
            settle(runner),
            [(request_task, Completion::TimedOut { node: 0 })]
        );
        // Rebuild through the same helper after verifying the earlier cancellation
        // is idempotent; no replacement was allowed to answer the old request.
        fixture::replace_cancelled(runner, medium, nodes, RESTARTED);
        connect(medium, nodes, 0, RESTARTED);
        let fresh = link(runner, nodes, 0, RESTARTED);
        assert_ne!(fresh, original);
        echo(runner, nodes, 0, fresh, b"after restart");
        echo(runner, nodes, 2, survivor, b"same survivor after restart");
        let elapsed = runner
            .snapshot()
            .unwrap_or_else(|error| unreachable!("clock: {error}"))
            .runtime_elapsed;
        assert_eq!(elapsed, Duration::from_millis(REQUEST_TIMEOUT_MILLIS));
        let expected: BTreeMap<_, _> = [
            (RESTARTED, DurationMillis(0)),
            (2, DurationMillis(REQUEST_TIMEOUT_MILLIS)),
        ]
        .into_iter()
        .map(|(node, elapsed)| {
            let clock = nodes[node].control.clock.clone();
            let origin = nodes[node].control.origin;
            let task = runner
                .insert(async move {
                    Completion::Clock {
                        node,
                        elapsed: clock.now().duration_since(origin),
                    }
                })
                .unwrap_or_else(|error| unreachable!("clock observation: {error}"));
            (task, Completion::Clock { node, elapsed })
        })
        .collect();
        assert_eq!(
            settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
            expected
        );
        assert!(!medium
            .trace()
            .events
            .iter()
            .any(|event| matches!(event, MediumEvent::ReceptionDropped { .. })));
    });
}
