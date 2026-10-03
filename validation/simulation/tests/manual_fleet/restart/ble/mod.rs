use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use personal_rns::engine::{RequestResponseTimeout, SendRequestFailure};
use personal_rns::interfaces::bluetooth_auto::{AppleHost, BleAddress, BlueZHost, Endpoint};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{PrnsNodeHandle, SendError};
use personal_rns::units::DurationMillis;
use prns_simulation::ble::{BleSimulationEvent, VirtualBleLab};
use prns_simulation::{
    ManualTaskCancellation, ManualTaskRunner, Reachability, SimulationTick, TopologyMutation,
};

use crate::ble::{advance_until, member_inventory};
use crate::scenario::{
    add_node, announce, destination, nonzero, request, settle, Completion, NodeControl, NodeRole,
    NodeSpec, QUERY_PATH,
};

mod fixture;
use fixture::{connect, converge, echo, establish, expected_member, start, with_fleet, NODE_COUNT};

const PAYLOAD_BYTES: usize = 256;
const OLD_REQUEST_TIMEOUT_MS: u64 = 100;
const RESTARTS: usize = 3;

fn exercise(restarted: usize) {
    with_fleet(|runner, lab, nodes| {
        let peer = restarted ^ 1;
        let survivor_link = establish(runner, nodes, 2, 3);
        let mut link = establish(runner, nodes, peer, restarted);
        echo(runner, nodes, peer, link, 0x41);
        for cycle in 0..RESTARTS {
            let before = runner.snapshot().ok();
            let old_task = nodes[restarted].task;
            let old_radio = nodes[restarted].radio;
            let old_status = nodes[restarted].status.clone();
            assert_eq!(
                runner.cancel(old_task).ok(),
                Some(ManualTaskCancellation::Cancelled)
            );
            assert!(settle(runner).is_empty());
            assert_eq!(runner.snapshot().ok(), before);
            assert_eq!(runner.task_count(), NODE_COUNT - 1);
            assert_eq!(lab.active_connection_count(), 1);
            assert_eq!(member_inventory(&nodes[peer].control.handle), Vec::new());
            echo(runner, nodes, 2, survivor_link, 0x50 + cycle as u8);

            let replacement = start(runner, lab, restarted);
            assert_ne!(replacement.task, old_task);
            assert_ne!(replacement.radio, old_radio);
            assert_eq!(replacement.status.id(), old_status.id());
            nodes[restarted] = replacement;
            connect(lab, 0, 1);
            converge(runner, lab, nodes);
            assert_eq!(
                member_inventory(&nodes[peer].control.handle),
                expected_member(restarted)
            );
            assert_eq!(
                runner.cancel(old_task).ok(),
                Some(ManualTaskCancellation::NotLive)
            );
            // Retained UI handles belong to their retired supervisor, not to a
            // later supervisor that happens to have the same interface identity.
            old_status.disable();
            assert!(settle(runner).is_empty());
            assert_eq!(
                nodes[restarted].status.connection(),
                ConnectionState::Connected
            );
            assert_eq!(lab.active_connection_count(), 2);

            let fresh = establish(runner, nodes, peer, restarted);
            assert_ne!(fresh, link);
            let started = runner
                .snapshot()
                .unwrap_or_else(|error| unreachable!("clock: {error}"))
                .tick;
            let handle = nodes[peer].control.handle.clone();
            let obsolete = runner
                .insert(async move {
                    assert_eq!(
                        handle
                            .request_with_response_timeout(
                                link,
                                RequestPathHash::of(QUERY_PATH),
                                b"obsolete link",
                                RequestResponseTimeout::Exact(DurationMillis(
                                    OLD_REQUEST_TIMEOUT_MS
                                ))
                            )
                            .await,
                        Err(SendError::Failed(SendRequestFailure::Timeout))
                    );
                    Completion::TimedOut { node: peer }
                })
                .unwrap_or_else(|error| unreachable!("old-link request: {error}"));
            echo(runner, nodes, peer, fresh, 0x60 + cycle as u8);
            echo(runner, nodes, 2, survivor_link, 0x70 + cycle as u8);
            for elapsed in 1..OLD_REQUEST_TIMEOUT_MS {
                assert!(runner
                    .advance_to_next_event(SimulationTick::from_ticks(started.get() + elapsed))
                    .is_ok());
                assert!(settle(runner).is_empty());
            }
            assert!(runner
                .advance_to_next_event(SimulationTick::from_ticks(
                    started.get() + OLD_REQUEST_TIMEOUT_MS
                ))
                .is_ok());
            assert_eq!(
                settle(runner),
                [(obsolete, Completion::TimedOut { node: peer })]
            );
            assert_eq!(
                runner.snapshot().ok().map(|snapshot| snapshot.tick),
                Some(SimulationTick::from_ticks(
                    started.get() + OLD_REQUEST_TIMEOUT_MS
                ))
            );
            echo(runner, nodes, peer, fresh, 0x80 + cycle as u8);
            link = fresh;
        }
    });
}

#[test]
fn apple_node_restart_rebuilds_ble_membership_without_disturbing_the_other_pair() {
    exercise(0);
}

#[test]
fn bluez_node_restart_rebuilds_ble_membership_without_disturbing_the_other_pair() {
    exercise(1);
}
