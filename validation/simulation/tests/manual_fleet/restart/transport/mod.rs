use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use personal_rns::engine::{RequestResponseTimeout, SendRequestFailure};
use personal_rns::interfaces::InterfaceId;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{PrnsNodeHandle, SendError};
use personal_rns::units::DurationMillis;
use personal_rns::wire::DestinationHash;
use prns_simulation::{
    FaultPlan, ManualTaskCancellation, ManualTaskId, ManualTaskRunner, MediumEvent, Reachability,
    SimulationTick, TopologyMutation, VirtualMedium,
};

use crate::scenario::{
    add_node, announce, destination, nonzero, request, settle, Completion, NodeControl, NodeRole,
    NodeSpec, QUERY_PATH,
};

mod fixture;
mod inflight;
mod topology;
mod traffic;
use fixture::{assert_empty_routes, hash, rebuild, with_fleet, LiveNode};
use topology::{Side, LEAVES, NODE_COUNT, PORT_COUNT};
use traffic::{discover, echo, establish, expire, timeouts};

const REQUEST_TIMEOUT_MS: u64 = 50;
const RESTARTS: usize = 2;

fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

fn now(runner: &ManualTaskRunner<'_, Completion>) -> u64 {
    runner
        .snapshot()
        .unwrap_or_else(|error| unreachable!("coordinated transport time: {error}"))
        .tick
        .get()
}

fn exercise(side: Side) {
    with_fleet(|runner, medium, nodes| {
        discover(runner, nodes);
        let [first, second] = side.unaffected_pair();
        let survivor = establish(runner, nodes, first, second);
        let mut crossing = [
            (0, establish(runner, nodes, 0, 2)),
            (2, establish(runner, nodes, 2, 0)),
        ];
        for &(from, link) in &crossing {
            echo(runner, nodes, from, link, b"before restart");
        }
        for _ in 0..RESTARTS {
            let index = side.transport();
            let before = runner.snapshot().ok();
            assert_eq!(
                runner.cancel(nodes[index].task).ok(),
                Some(ManualTaskCancellation::Cancelled)
            );
            assert!(settle(runner).is_empty());
            assert_eq!(runner.snapshot().ok(), before);
            assert_eq!(runner.task_count(), NODE_COUNT - 1);
            let started = now(runner);
            let pending = timeouts(runner, nodes, &crossing);
            echo(
                runner,
                nodes,
                first,
                survivor,
                b"unaffected during downtime",
            );
            expire(runner, started, pending);

            rebuild(runner, medium, nodes, side);
            assert_empty_routes(runner, &nodes[index], index);
            topology::connect_node(medium, nodes, index);
            discover(runner, nodes);
            let fresh = [
                (0, establish(runner, nodes, 0, 2)),
                (2, establish(runner, nodes, 2, 0)),
            ];
            for (old, fresh) in crossing.iter().zip(&fresh) {
                assert_ne!(old.1, fresh.1);
            }
            let started = now(runner);
            let obsolete = timeouts(runner, nodes, &crossing);
            for &(from, link) in &fresh {
                echo(runner, nodes, from, link, b"new forwarding maps");
            }
            echo(
                runner,
                nodes,
                first,
                survivor,
                b"surviving original mapping",
            );
            expire(runner, started, obsolete);
            crossing = fresh;
        }
    });
}

#[test]
fn left_transport_restart_rebuilds_routes_but_not_obsolete_link_forwarding() {
    exercise(Side::Left);
}

#[test]
fn right_transport_restart_rebuilds_routes_but_not_obsolete_link_forwarding() {
    exercise(Side::Right);
}
