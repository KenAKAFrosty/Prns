use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use personal_rns::engine::{RequestResponseTimeout, SendRequestFailure};
use personal_rns::interfaces::InterfaceId;
use personal_rns::manifold::interface_seam::Interface;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{PrnsNodeHandle, SendError};
use personal_rns::units::DurationMillis;
use personal_rns::wire::DestinationHash;
use prns_simulation::{
    EndpointId, FaultPlan, ManualMedium, ManualTaskCancellation, ManualTaskId, ManualTaskRunner,
    ManualTaskScheduling, ManualTimeDriver, MediumEvent, Reachability, SimulationSeed,
    SimulationTick, TopologyConfig, TopologyMutation, VirtualMedium, VirtualMediumConfig,
};

use crate::scenario::{
    add_node, announce, destination, nonzero, request, settle, Completion, NodeControl, NodeRole,
    NodeSpec, QUERY_PATH,
};

mod abort;
mod evidence;
mod fixture;
mod traffic;

use fixture::{connect, hash, start, LiveNode};
use traffic::{echo_round, expire, links, lost_requests};

const PAIRS: usize = 64;
const NODES: usize = PAIRS * 2;
const RESTARTED_PER_WAVE: usize = PAIRS / 2;
const TIMEOUT_MS: u64 = 50;
const ACTOR_CAPACITY: usize = NODES * 2;
const TRACE_CAPACITY: usize = NODES * 256;

#[derive(Clone, Copy)]
enum Wave {
    EvenPairs,
    OddPairs,
}

impl Wave {
    fn affects(self, pair: usize) -> bool {
        match self {
            Self::EvenPairs => pair.is_multiple_of(2),
            Self::OddPairs => !pair.is_multiple_of(2),
        }
    }

    fn pairs(self) -> impl Iterator<Item = usize> {
        (0..PAIRS).filter(move |&pair| self.affects(pair))
    }
}

fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

fn now(runner: &ManualTaskRunner<'_, Completion>) -> u64 {
    runner
        .snapshot()
        .unwrap_or_else(|error| unreachable!("fleet clock: {error}"))
        .tick
        .get()
}

fn exercise(waves: [Wave; 2], scheduling: ManualTaskScheduling) {
    let medium = fixture::medium();
    let mut driver = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("fleet driver: {error}"));
    let mut runner =
        ManualTaskRunner::new_with_scheduling(&mut driver, nonzero(ACTOR_CAPACITY), scheduling);
    let mut nodes: Vec<_> = (0..NODES)
        .map(|index| start(&mut runner, &medium, index))
        .collect();
    for pair in 0..PAIRS {
        connect(&medium, &nodes, pair);
    }
    let mut current = links(&mut runner, &nodes, 0..PAIRS);
    evidence::capacity(&medium, &nodes);
    echo_round(&mut runner, &nodes, &current, 0, 0..PAIRS);
    evidence::clocks(&mut runner, &nodes);

    for (round, wave) in waves.into_iter().enumerate() {
        let started = now(&runner);
        let old: BTreeMap<_, _> = wave.pairs().map(|pair| (pair, current[&pair])).collect();
        let cancelled: Vec<_> = wave.pairs().map(|pair| nodes[pair * 2 + 1].task).collect();
        for &task in &cancelled {
            assert_eq!(
                runner.cancel(task).ok(),
                Some(ManualTaskCancellation::Cancelled)
            );
        }
        assert!(settle(&mut runner).is_empty());
        assert_eq!(now(&runner), started);
        assert_eq!(runner.task_count(), NODES - RESTARTED_PER_WAVE);

        let pending = lost_requests(&mut runner, &nodes, &old);
        echo_round(
            &mut runner,
            &nodes,
            &current,
            round + 1,
            (0..PAIRS).filter(|&pair| !wave.affects(pair)),
        );
        assert_eq!(runner.task_count(), NODES);
        expire(&mut runner, started, pending);
        assert_eq!(runner.task_count(), NODES - RESTARTED_PER_WAVE);

        for pair in wave.pairs() {
            let index = pair * 2 + 1;
            let fresh = start(&mut runner, &medium, index);
            assert_ne!(fresh.task, nodes[index].task);
            assert_ne!(fresh.endpoint, nodes[index].endpoint);
            assert_eq!(fresh.interface, nodes[index].interface);
            assert!(fresh.heard.borrow().is_empty());
            nodes[index] = fresh;
            connect(&medium, &nodes, pair);
        }
        for task in cancelled {
            assert_eq!(
                runner.cancel(task).ok(),
                Some(ManualTaskCancellation::NotLive)
            );
        }
        assert_eq!(runner.task_count(), NODES);
        assert_eq!(now(&runner), started + TIMEOUT_MS);
        let replacements = links(&mut runner, &nodes, wave.pairs());
        for (&pair, &link) in &replacements {
            assert_ne!(link, old[&pair]);
        }
        current.extend(replacements);
        evidence::capacity(&medium, &nodes);

        let started = now(&runner);
        let obsolete = lost_requests(&mut runner, &nodes, &old);
        echo_round(&mut runner, &nodes, &current, round + 3, 0..PAIRS);
        assert_eq!(runner.task_count(), NODES + RESTARTED_PER_WAVE);
        expire(&mut runner, started, obsolete);
        evidence::clocks(&mut runner, &nodes);
    }
    assert_eq!(now(&runner), 4 * TIMEOUT_MS);
    echo_round(&mut runner, &nodes, &current, 5, 0..PAIRS);
    evidence::shutdown(&mut runner, nodes);
    evidence::trace(&medium, NODES + PAIRS);
}

#[test]
fn full_node_restart_waves_preserve_unaffected_pairs_and_recover_all_members() {
    exercise(
        [Wave::EvenPairs, Wave::OddPairs],
        ManualTaskScheduling::Cyclic,
    );
}

#[test]
fn full_node_restart_waves_recover_with_reversed_cohort_order() {
    exercise(
        [Wave::OddPairs, Wave::EvenPairs],
        ManualTaskScheduling::Cyclic,
    );
}

#[test]
fn full_node_restart_waves_recover_under_seeded_actor_orders() {
    for seed in [0, 7, u64::MAX] {
        for waves in [
            [Wave::EvenPairs, Wave::OddPairs],
            [Wave::OddPairs, Wave::EvenPairs],
        ] {
            exercise(
                waves,
                ManualTaskScheduling::Seeded {
                    seed: SimulationSeed::new(seed),
                },
            );
        }
    }
}
