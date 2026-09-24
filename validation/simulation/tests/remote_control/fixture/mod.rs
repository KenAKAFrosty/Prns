use std::cell::RefCell;
use std::future::Future;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::time::Duration;

use personal_rns::engine::SendRequestFailure;
use personal_rns::identity::IdentityHash;
use personal_rns::remote_control::*;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{
    NodeRunError, PrnsNodeHandle, RemoteControlInterfaceWatch, RemoteControlWatchReadError,
    SendError,
};
use prns_simulation::*;

mod control;
mod host;
pub use host::resource_body;
mod node;
pub mod pairing;
pub mod persistence;
mod watch;
pub use control::encoded;
pub use node::Node;
pub use watch::watch_result;
pub mod inventory;

pub const TARGET: usize = 0;
pub const CONTROLLER: usize = 1;
pub const OUTSIDER: usize = 2;
pub const OPERATOR: usize = 3;
const NODE_COUNT: usize = 4;
const MAX_ACTORS: usize = 20;
const POLL_BUDGET: usize = 8192;
pub const REQUEST_TIMEOUT_MS: u64 = 50;
const TRACE_CAPACITY: usize = 32_768;
const RECEIVE_FRAME_CAPACITY: usize = 64;
const PENDING_DELIVERY_CAPACITY: usize = 256;
pub(super) const MAX_APP_INVOCATIONS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInvocation {
    pub controller: IdentityHash,
    pub payload: Vec<u8>,
}

pub enum Event {
    Stopped {
        node: usize,
        result: Result<(), NodeRunError>,
    },
    Linked(LinkId),
    Response(Result<Vec<u8>, SendError<SendRequestFailure>>),
    Done,
    PairingOpened(personal_rns::engine::RemoteControlPairingOpened),
    PairingResponse(
        Result<
            personal_rns::engine::RemoteControlControllerPairingResponseReceived,
            personal_rns::runtime::InitiateRemoteControlControllerPairingError,
        >,
    ),
    WatchOpened(RemoteControlInterfaceWatch),
    WatchRead {
        watch: RemoteControlInterfaceWatch,
        result: Result<RemoteControlStreamEvent, RemoteControlWatchReadError>,
    },
}

pub struct Lab<'a> {
    pub runner: ManualTaskRunner<'a, Event>,
    pub medium: VirtualMedium,
    pub nodes: Vec<Node>,
    pub calls: Rc<RefCell<Vec<AppInvocation>>>,
    boot_generations: [u64; NODE_COUNT],
    app_gates: Rc<RefCell<Vec<host::MessageGate>>>,
    budgets: FixtureBudgets,
    pub pairing: Rc<RefCell<Vec<pairing::PairingObservation>>>,
}

pub fn requests() -> RemoteControlRequestSet {
    let mut requests = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
    for kind in [
        RemoteControlRequestKind::DescribeBuild,
        RemoteControlRequestKind::InventoryInterfaces,
        RemoteControlRequestKind::InventoryInterfaceConfig,
        RemoteControlRequestKind::InventoryInterfacePeers,
        RemoteControlRequestKind::AppMessage,
        RemoteControlRequestKind::WatchInterfaces,
    ] {
        requests.insert(kind);
    }
    requests
}

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("positive fixture capacity")
}
pub fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

pub enum LinkIdentity {
    Controller,
    Unidentified,
}

#[derive(Clone)]
pub enum ControllerPolicy {
    Granted(RemoteControlRequestSet),
    Nobody,
    Grants(Vec<RemoteControlControllerGrant>),
}

pub fn with_lab<R>(
    scheduling: ManualTaskScheduling,
    faults: FaultPlan,
    scenario: impl FnOnce(&mut Lab<'_>) -> R,
) -> (R, Vec<MediumEvent>) {
    with_policy(
        scheduling,
        faults,
        ControllerPolicy::Granted(requests()),
        scenario,
    )
}

pub fn with_policy<R>(
    scheduling: ManualTaskScheduling,
    faults: FaultPlan,
    policy: ControllerPolicy,
    scenario: impl FnOnce(&mut Lab<'_>) -> R,
) -> (R, Vec<MediumEvent>) {
    with_storage(scheduling, faults, policy, None, scenario)
}

pub fn with_storage<R>(
    scheduling: ManualTaskScheduling,
    faults: FaultPlan,
    policy: ControllerPolicy,
    storage: Option<persistence::Storage>,
    scenario: impl FnOnce(&mut Lab<'_>) -> R,
) -> (R, Vec<MediumEvent>) {
    with_budgets(
        scheduling,
        faults,
        policy,
        storage,
        FixtureBudgets::standard(),
        scenario,
    )
}

#[derive(Clone)]
pub struct FixtureBudgets {
    pub actors: usize,
    pub polls: usize,
    pub trace: usize,
    pub receive_frames: usize,
    pub pending_deliveries: usize,
    pub app_invocations: usize,
}

impl FixtureBudgets {
    pub fn standard() -> Self {
        Self {
            actors: MAX_ACTORS,
            polls: POLL_BUDGET,
            trace: TRACE_CAPACITY,
            receive_frames: RECEIVE_FRAME_CAPACITY,
            pending_deliveries: PENDING_DELIVERY_CAPACITY,
            app_invocations: MAX_APP_INVOCATIONS,
        }
    }
    pub fn pressure() -> Self {
        Self {
            actors: 1536,
            polls: 262144,
            trace: 262144,
            receive_frames: 2048,
            pending_deliveries: 4096,
            app_invocations: 1536,
        }
    }
}

pub fn with_budgets<R>(
    scheduling: ManualTaskScheduling,
    faults: FaultPlan,
    policy: ControllerPolicy,
    storage: Option<persistence::Storage>,
    budgets: FixtureBudgets,
    scenario: impl FnOnce(&mut Lab<'_>) -> R,
) -> (R, Vec<MediumEvent>) {
    let medium = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(NODE_COUNT - 1),
            },
            NODE_COUNT,
            budgets.receive_frames,
            budgets.pending_deliveries,
            budgets.trace,
            faults,
        )
        .expect("bounded control medium"),
    );
    let mut clock = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .expect("manual control clock");
    let mut lab = Lab {
        runner: ManualTaskRunner::new_with_scheduling(
            &mut clock,
            nonzero(budgets.actors),
            scheduling,
        ),
        medium,
        nodes: Vec::new(),
        boot_generations: [0; NODE_COUNT],
        app_gates: Rc::new(RefCell::new(Vec::new())),
        budgets,
        calls: Rc::new(RefCell::new(Vec::new())),
        pairing: Rc::new(RefCell::new(Vec::new())),
    };
    for index in 0..NODE_COUNT {
        lab.start_node(
            index,
            match index {
                TARGET => policy.clone(),
                _ => ControllerPolicy::Nobody,
            },
            match index {
                TARGET => storage.clone(),
                _ => None,
            },
        );
    }
    for index in [CONTROLLER, OUTSIDER, OPERATOR] {
        lab.set_reachability(index, Reachability::Reachable);
    }
    if let Some(storage) = storage {
        let handle = lab.nodes[TARGET].handle.clone();
        let directory = storage.directory().to_path_buf();
        let task = lab.insert(async move {
            handle
                .flush_to_store(&mut personal_rns::persistence::FileStore::new(directory))
                .await
                .expect("persist initial provisioned authority");
            Event::Done
        });
        lab.expect_done(task);
    }
    let result = scenario(&mut lab);
    let nodes = std::mem::take(&mut lab.nodes);
    let tasks: Vec<_> = nodes.iter().map(|node| node.task).collect();
    for node in nodes {
        node.shutdown.send(()).expect("live shutdown receiver");
    }
    let mut stopped = lab.settle();
    for _ in 0..2_500 {
        if stopped.len() == NODE_COUNT {
            break;
        }
        stopped.extend(lab.advance(1));
    }
    assert_eq!(stopped.len(), NODE_COUNT);
    for (task, event) in stopped {
        let Event::Stopped { node, result } = event else {
            unreachable!("only node shutdowns remain");
        };
        assert_eq!(task, tasks[node]);
        assert_eq!(result, Ok(()));
    }
    assert_eq!(lab.runner.task_count(), 0);
    assert_eq!(lab.medium.pending_delivery_count(), 0);
    let trace = lab.medium.trace();
    assert_eq!(
        trace.discarded_events, 0,
        "bounded trace must remain complete"
    );
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in &trace.events {
        match event {
            MediumEvent::EndpointAttached { endpoint, .. } => attached.push(*endpoint),
            MediumEvent::EndpointDetached { endpoint } => detached.push(*endpoint),
            MediumEvent::ReceptionDropped {
                reason:
                    ReceptionDropReason::ReceiveQueueFull | ReceptionDropReason::PendingCapacityReached,
                ..
            } => unreachable!("fixture capacity loss"),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached, detached);
    (result, trace.events)
}

impl Lab<'_> {
    pub fn settle(&mut self) -> Vec<(ManualTaskId, Event)> {
        let mut completed = Vec::new();
        for _ in 0..self.budgets.polls {
            match self.runner.poll_next().expect("manual control poll") {
                ManualTaskPoll::Idle => return completed,
                ManualTaskPoll::Pending { .. } => {}
                ManualTaskPoll::Completed { task, output } => completed.push((task, output)),
            }
        }
        unreachable!(
            "control scenario exceeded {} actor polls",
            self.budgets.polls
        );
    }
    pub fn insert(&mut self, future: impl Future<Output = Event> + 'static) -> ManualTaskId {
        self.runner.insert(future).expect("bounded operation actor")
    }
    pub fn advance(&mut self, millis: u64) -> Vec<(ManualTaskId, Event)> {
        let mut completed = Vec::new();
        let end = self.medium.now().get() + millis;
        while self.medium.now().get() < end {
            self.runner
                .advance_to_next_event(tick(self.medium.now().get() + 1))
                .expect("coordinated control time");
            completed.extend(self.settle());
        }
        completed
    }
    pub fn set_reachability(&self, node: usize, reachability: Reachability) {
        assert!(matches!(
            self.medium.set_reachability(
                self.nodes[TARGET].endpoint,
                self.nodes[node].endpoint,
                reachability
            ),
            Ok(TopologyMutation::Applied | TopologyMutation::Unchanged)
        ));
    }
    pub fn expect_done(&mut self, task: ManualTaskId) {
        let completed = self.settle();
        assert_eq!(completed.len(), 1, "one local control completion");
        let (found, event) = &completed[0];
        assert_eq!(*found, task);
        assert!(matches!(event, Event::Done));
    }
}

pub fn grant(
    index: usize,
    authority: RemoteControlControllerAuthority,
    permitted_requests: RemoteControlRequestSet,
) -> RemoteControlControllerGrant {
    RemoteControlControllerGrant::new(
        *node::secrets(index).identities().controller(),
        authority,
        permitted_requests,
    )
    .expect("explicit fixture grant")
}

pub fn management_requests() -> RemoteControlRequestSet {
    let mut allowed = requests();
    for kind in [
        RemoteControlRequestKind::InventoryControllers,
        RemoteControlRequestKind::AuthorizeController,
        RemoteControlRequestKind::RevokeController,
    ] {
        allowed.insert(kind);
    }
    allowed
}

impl Lab<'_> {
    pub fn gate_app(&self, controller: usize, payload: &[u8]) -> tokio::sync::oneshot::Sender<()> {
        let (release, ready) = tokio::sync::oneshot::channel();
        let mut gates = self.app_gates.borrow_mut();
        assert!(
            gates.len() < self.budgets.app_invocations,
            "bounded app gates"
        );
        gates.push(host::MessageGate {
            controller: self.nodes[controller].identity.identity_hash(),
            payload: payload.to_vec(),
            release: ready,
        });
        release
    }
}
