use personal_rns::interfaces::{wifi_halow::*, BitrateBps, InterfaceId, InterfaceKind};
use personal_rns::{identity::IdentityHash, remote_control::*, runtime::*};
use prns_simulation::{halow::*, *};
use std::{
    cell::RefCell,
    future::Future,
    num::{NonZeroU32, NonZeroU8, NonZeroUsize},
    rc::Rc,
    time::Duration,
};
use tokio::sync::oneshot;

mod node;
pub use node::{controller, page, target};
mod host;
mod operations;
mod persistence;
pub use persistence::{Persistence, RetainedState};
mod device;
pub use device::{Bindings, DeviceControl, Presence};
pub const TARGET: usize = 0;
pub const PRIMARY: usize = 1;
pub const HEALTHY: usize = 2;
pub const OUTSIDER: usize = 3;
pub const NODE_COUNT: usize = 4;
pub const REQUEST_TIMEOUT_MS: u64 = 100;
pub const IDLE_SECONDS: u32 = 10;
const POLLS: usize = 32_768;
const ACTORS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Topology {
    Shared,
    Chain,
    Asymmetric,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInvocation {
    pub controller: IdentityHash,
    pub payload: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceSeen {
    pub node: usize,
    pub destination: personal_rns::wire::DestinationHash,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Measurement {
    ColdRadioBoot {
        wired_boots: u8,
        rebind_ticks: u64,
        open_attempts: u64,
    },
    DeviceRecovery {
        cycles: u8,
        maximum_rebind_ticks: u64,
        open_attempts: u64,
    },
    PeerBurst {
        injected_datagrams: usize,
        delivered_frames: u64,
    },
}

pub enum Event {
    PageConnection(
        Result<
            personal_rns::routing::links::LinkId,
            SendError<personal_rns::engine::EstablishLinkFailure>,
        >,
    ),
    Path(Result<personal_rns::engine::PathFound, RequestPathError>),
    Connection(Result<personal_rns::routing::links::LinkId, ConnectRemoteControlTargetError>),
    Stopped {
        node: usize,
        result: Result<(), NodeRunError>,
    },
    Linked(personal_rns::routing::links::LinkId),
    Response(Result<Vec<u8>, SendError<personal_rns::engine::SendRequestFailure>>),
    Done,
}
pub struct Node {
    pub handle: PrnsNodeHandle,
    pub radio: RadioId,
    pub adapter: AdapterState,
    task: ManualTaskId,
    shutdown: oneshot::Sender<()>,
}
pub enum AdapterState {
    Attached(AttachedSupervisor),
    Detached,
}
impl Node {
    pub fn adapter_id(&self) -> InterfaceId {
        match &self.adapter {
            AdapterState::Attached(attached) => attached.id(),
            AdapterState::Detached => panic!("detached adapter"),
        }
    }
}
pub struct Lab<'a> {
    pub runner: ManualTaskRunner<'a, Event>,
    pub medium: VirtualHaLowMedium,
    pub nodes: Vec<Node>,
    pub calls: Rc<RefCell<Vec<AppInvocation>>>,
    pub announces: Rc<RefCell<Vec<AnnounceSeen>>>,
    pub measurements: Vec<Measurement>,
    pub wired: Vec<AttachedInterface>,
    generations: [u64; NODE_COUNT],
    persistence: Persistence,
    bindings: Bindings,
}
pub fn nz(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("nonzero fixture budget")
}
pub fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}
pub fn mac(index: usize) -> PeerMac {
    PeerMac::new(personal_rns::interfaces::MacAddress::new([
        2,
        0,
        0,
        0,
        0,
        index as u8 + 1,
    ]))
    .expect("unicast address")
}
pub fn scope(index: usize) -> InstanceTag {
    InstanceTag::new(&[b'H', index as u8]).expect("stable scope")
}
pub fn permissions() -> RemoteControlRequestSet {
    let mut requests = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
    for kind in [
        RemoteControlRequestKind::DescribeBuild,
        RemoteControlRequestKind::InventoryInterfaces,
        RemoteControlRequestKind::InventoryInterfaceConfig,
        RemoteControlRequestKind::InventoryInterfacePeers,
        RemoteControlRequestKind::AppMessage,
    ] {
        requests.insert(kind);
    }
    requests
}
pub fn medium() -> VirtualHaLowMedium {
    VirtualHaLowMedium::new(HaLowMediumLimits {
        radios: nz(NODE_COUNT),
        receive_datagrams: nz(256),
        pending_deliveries: nz(512),
        trace_events: nz(65_536),
        armed_faults: nz(16),
    })
}
pub fn with_lab<R>(
    medium: VirtualHaLowMedium,
    seed: u64,
    topology: Topology,
    scenario: impl FnOnce(&mut Lab<'_>) -> R,
) -> R {
    with_persistence(medium, seed, topology, Persistence::Disabled, scenario)
}
pub fn with_persistence<R>(
    medium: VirtualHaLowMedium,
    seed: u64,
    topology: Topology,
    persistence: Persistence,
    scenario: impl FnOnce(&mut Lab<'_>) -> R,
) -> R {
    with_fixture(
        medium,
        seed,
        topology,
        persistence,
        Bindings::Explicit,
        scenario,
    )
}
pub fn with_fixture<R>(
    medium: VirtualHaLowMedium,
    seed: u64,
    topology: Topology,
    persistence: Persistence,
    bindings: Bindings,
    scenario: impl FnOnce(&mut Lab<'_>) -> R,
) -> R {
    let mut clock = ManualTimeDriver::new(
        ManualMedium::HaLow(medium.clone()),
        Duration::from_millis(1),
    )
    .expect("HaLoW clock");
    let mut lab = Lab {
        runner: ManualTaskRunner::new_with_scheduling(
            &mut clock,
            nz(ACTORS),
            ManualTaskScheduling::Seeded {
                seed: SimulationSeed::new(seed),
            },
        ),
        medium,
        nodes: Vec::new(),
        calls: Rc::new(RefCell::new(Vec::new())),
        announces: Rc::new(RefCell::new(Vec::new())),
        measurements: Vec::new(),
        wired: Vec::new(),
        generations: [0; NODE_COUNT],
        persistence,
        bindings,
    };
    for index in 0..NODE_COUNT {
        lab.start_node(index);
    }
    lab.topology(topology);
    let result = scenario(&mut lab);
    for node in std::mem::take(&mut lab.nodes) {
        node.shutdown.send(()).expect("live shutdown receiver");
    }
    let mut stopped = lab.settle();
    for _ in 0..2_500 {
        if stopped.len() == NODE_COUNT {
            break;
        }
        stopped.extend(lab.advance(1));
    }
    assert_eq!(stopped.len(), NODE_COUNT, "all node actors stop");
    for (_, event) in stopped {
        assert!(
            matches!(
                event,
                Event::Stopped {
                    node: 0..NODE_COUNT,
                    result: Ok(())
                }
            ),
            "graceful shutdown"
        );
    }
    assert_eq!(lab.runner.task_count(), 0);
    let final_state = lab.medium.snapshot();
    assert_eq!(
        final_state.retention,
        TraceRetention::Complete,
        "full trace required for qualification"
    );
    assert_eq!(
        (
            final_state.radios,
            final_state.queued,
            final_state.pending,
            final_state.armed_faults
        ),
        (0, 0, 0, 0)
    );
    for event in &final_state.events {
        assert!(
            !matches!(
                event,
                HaLowEvent::Delivery {
                    outcome: DeliveryOutcome::ReceiveQueueFull | DeliveryOutcome::PendingCapacity,
                    ..
                }
            ),
            "unplanned harness capacity loss"
        );
    }
    result
}
impl Lab<'_> {
    pub fn settle(&mut self) -> Vec<(ManualTaskId, Event)> {
        let mut completed = Vec::new();
        for _ in 0..POLLS {
            match self.runner.poll_next().expect("actor poll") {
                ManualTaskPoll::Idle => return completed,
                ManualTaskPoll::Pending { .. } => {}
                ManualTaskPoll::Completed { task, output } => completed.push((task, output)),
            }
        }
        panic!("HaLoW actors exceeded poll budget");
    }
    pub fn insert(&mut self, future: impl Future<Output = Event> + 'static) -> ManualTaskId {
        self.runner.insert(future).expect("bounded actor admission")
    }
    pub fn advance(&mut self, millis: u64) -> Vec<(ManualTaskId, Event)> {
        let end = self
            .medium
            .now()
            .get()
            .checked_add(millis)
            .expect("scenario time budget");
        let mut completed = self.settle();
        while self.medium.now().get() < end {
            self.runner
                .advance_to_next_wake(tick(end))
                .expect("coordinated next wake");
            completed.extend(self.settle());
        }
        completed
    }
    pub fn wait(&mut self, task: ManualTaskId, budget: u64) -> Event {
        let mut completed = self.settle();
        for _ in 0..budget {
            if !completed.is_empty() {
                break;
            }
            completed = self.advance(1);
        }
        assert_eq!(completed.len(), 1, "one operation within {budget} ms");
        let (found, event) = completed.remove(0);
        assert_eq!(found, task);
        event
    }
    pub fn complete(&mut self, future: impl Future<Output = ()> + 'static) {
        let task = self.insert(async move {
            future.await;
            Event::Done
        });
        assert!(matches!(self.wait(task, 10_000), Event::Done));
    }
    pub fn wait_many(&mut self, tasks: &[ManualTaskId], budget: u64) -> Vec<(ManualTaskId, Event)> {
        let mut completed = self.settle();
        for _ in 0..budget {
            if completed.len() == tasks.len() {
                break;
            }
            completed.extend(self.advance(1));
        }
        assert_eq!(
            completed.len(),
            tasks.len(),
            "all concurrent operations complete"
        );
        assert!(completed.iter().all(|(task, _)| tasks.contains(task)));
        completed.sort_by_key(|(task, _)| *task);
        completed
    }
    pub fn topology(&self, topology: Topology) {
        for from in 0..NODE_COUNT {
            for to in 0..NODE_COUNT {
                if from == to {
                    continue;
                }
                let reachable = match topology {
                    Topology::Shared => true,
                    Topology::Chain => from == HEALTHY || to == HEALTHY,
                    Topology::Asymmetric => {
                        (from == TARGET && to == PRIMARY)
                            || (from == PRIMARY && to == HEALTHY)
                            || (from == HEALTHY && to == TARGET)
                            || (from == TARGET && to == HEALTHY)
                            || (from == OUTSIDER && to == TARGET)
                            || (from == TARGET && to == OUTSIDER)
                    }
                };
                self.medium.set_path(
                    self.nodes[from].radio,
                    self.nodes[to].radio,
                    if reachable {
                        PathState::Reachable
                    } else {
                        PathState::Isolated
                    },
                );
            }
        }
    }
    pub fn drained_metrics(&mut self) -> Vec<Vec<(String, String, u64)>> {
        let mut snapshots = Vec::new();
        for index in 0..NODE_COUNT {
            let handle = self.nodes[index].handle.clone();
            let output = Rc::new(RefCell::new(Vec::new()));
            let destination = output.clone();
            self.complete(async move {
                let snapshot = handle
                    .metrics_snapshot()
                    .await
                    .expect("actual runtime metrics");
                assert_eq!(
                    (
                        snapshot.engine.resources.incoming.active_rows,
                        snapshot.engine.resources.outgoing.active_rows,
                        snapshot.engine.resources.pending_depth
                    ),
                    (0, 0, 0),
                    "Resource owners must drain"
                );
                if let Some(crypto) = snapshot.crypto {
                    assert_eq!((crypto.queue_depth, crypto.packet_verdicts_owed), (0, 0));
                }
                destination.borrow_mut().extend(
                    snapshot
                        .reliability
                        .operations
                        .iter()
                        .filter(|(_, _, count)| *count != 0)
                        .map(|(op, outcome, count)| {
                            (format!("{op:?}"), format!("{outcome:?}"), count)
                        }),
                );
            });
            snapshots.push(output.take());
        }
        snapshots
    }
    pub fn peer_ids(&self, index: usize) -> Vec<InterfaceId> {
        let mut ids: Vec<_> = self.nodes[index]
            .handle
            .interfaces()
            .iter()
            .filter(|row| row.id.kind() == Some(InterfaceKind::WifiHaLowPeer))
            .map(|row| row.id)
            .collect();
        ids.sort_by_key(|id| *id.as_bytes());
        ids
    }
}
