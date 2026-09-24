use super::{profile::Profile, traffic};
use crate::clock::{ClockLease, CompletionBudget, EmbassyTasks};
use crate::node_events::EventLog;
use crate::remote_control::{
    adapter::{Handle, Runtime},
    node::{self, NodeSettings, Storage},
    pairing::{self, Observation},
    Messages,
};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, PrnsCommand, RequestResponseTimeout,
    SendRequestFailure,
};
use personal_rns::interfaces::{bluetooth_auto::BleAddress, ConnectionState, Membership};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{CryptoPoolConfig, RequestOptions, SendError};
use personal_rns::units::{ByteLimit, DurationMillis};
use prns_runtime_tokio::runtime::ControlledCrypto;
use prns_simulation::ble::{BleMediumConfig, BleWireCapture, VirtualBleLab};
use prns_simulation::{
    ManualMedium, ManualTaskScheduling, ManualTimeDriver, Reachability, SimulationSeed,
    TopologyConfig,
};
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::time::Duration;

pub const PRIMARY: usize = 0;
pub const TARGET: usize = 1;
pub const HEALTHY: usize = 2;
const ACTORS: usize = 24;
const POLLS_PER_TICK: usize = 32768;
const OPERATION_BUDGET_MS: u64 = 10000;
const WIRE_CAPACITY: usize = 65536;
const MEDIUM_TRACE_CAPACITY: usize = 131072;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLayout {
    Common,
    WorkerSized,
}

pub struct Triple<'a, 'clock> {
    pub tasks: &'a mut EmbassyTasks<'clock>,
    pub lab: &'a VirtualBleLab,
    pub nodes: [node::Node; 3],
    pub storage: [Storage; 3],
    pub profile: Profile,
    pub layout: ResourceLayout,
    pub generations: [u64; 3],
    pub controls: Vec<(usize, u64, ControlledCrypto)>,
    pub messages: Messages,
    pub links: [LinkId; 2],
    pub app_links: [LinkId; 2],
    retired_events: Vec<(usize, u64, EventLog)>,
}
impl Triple<'_, '_> {
    pub fn complete<T: 'static>(
        &mut self,
        future: impl std::future::Future<Output = T> + 'static,
    ) -> T {
        self.tasks.complete_with_budget(
            CompletionBudget {
                deadline: crate::tick(self.tasks.snapshot().tick.get() + OPERATION_BUDGET_MS),
                polls_per_tick: NonZeroUsize::new(POLLS_PER_TICK).expect("poll budget"),
            },
            future,
        )
    }
    pub fn drive_until(&mut self, predicate: impl Fn() -> bool) {
        const BOUNDARY_TICKS: usize = 1000;
        const BOUNDARY_POLLS: usize = 1024;
        for _ in 0..BOUNDARY_TICKS {
            self.tasks
                .poll_turns(NonZeroUsize::new(BOUNDARY_POLLS).expect("boundary polls"));
            if predicate() {
                return;
            }
            let horizon = crate::tick(self.tasks.snapshot().tick.get() + 1);
            self.tasks
                .advance_to_next_wake(horizon)
                .expect("boundary clock");
        }
        panic!("actual work did not reach its controlled boundary");
    }
    pub fn exchange(
        &mut self,
        controller: usize,
        request: personal_rns::remote_control::RemoteControlRequest,
    ) -> Result<personal_rns::remote_control::RemoteControlResponse, SendError<SendRequestFailure>>
    {
        let handle = self.nodes[controller].handle.clone();
        let link = self.links[controller_slot(controller)];
        self.complete(async move { exchange(handle, link, request).await })
    }
    pub fn reconnect(&mut self, controller: usize) {
        let slot = controller_slot(controller);
        self.nodes[controller].handle.close(self.links[slot]);
        self.nodes[controller].handle.close(self.app_links[slot]);
        self.tasks.settle();
        self.advance_for(1000);
        self.discover();
        let handle = self.nodes[controller].handle.clone();
        self.links[slot] = self.complete(async move { handle.connect().await });
        let handle = self.nodes[controller].handle.clone();
        self.app_links[slot] = self.complete(async move { connect_app(handle).await });
        self.advance_for(150);
    }
    pub fn advance_for(&mut self, millis: u64) {
        let horizon = crate::tick(self.tasks.snapshot().tick.get() + millis);
        while self.tasks.snapshot().tick < horizon {
            self.tasks.settle();
            self.tasks
                .advance_to_next_wake(horizon)
                .expect("bounded timer advancement");
        }
        self.tasks.settle();
    }
    fn discover(&mut self) {
        let baseline = route_counts(&self.nodes);
        let target = self.nodes[TARGET].handle.clone();
        self.complete(async move {
            target.announce().await;
            announce_app(target).await;
        });
        self.tasks.settle();
        wait_app_routes(self.tasks, &self.nodes, baseline);
    }
    pub fn restart(&mut self, index: usize) {
        for controller in [PRIMARY, HEALTHY] {
            if index == TARGET || index == controller {
                reach(self.lab, controller, Reachability::Isolated);
            }
        }
        self.retired_events.push((
            index,
            self.generations[index],
            self.nodes[index].events.clone(),
        ));
        self.tasks.cancel(self.nodes[index].task);
        self.tasks.settle();
        self.generations[index] += 1;
        let runtime = runtime(&self.profile, index);
        let crypto = self.profile.crypto(runtime);
        retain_control(&mut self.controls, index, self.generations[index], &crypto);
        self.nodes[index] = start(
            self.tasks,
            self.lab,
            index,
            self.generations[index],
            self.storage[index].clone(),
            self.messages.clone(),
            (crypto, self.layout),
        );
        for controller in [PRIMARY, HEALTHY] {
            reach(self.lab, controller, Reachability::Reachable);
        }
        converge(self.tasks, self.lab, &self.nodes);
        self.advance_for(1000);
        self.discover();
        for controller in [PRIMARY, HEALTHY] {
            if index == TARGET || index == controller {
                let handle = self.nodes[controller].handle.clone();
                self.links[controller_slot(controller)] =
                    self.complete(async move { handle.connect().await });
                let handle = self.nodes[controller].handle.clone();
                self.app_links[controller_slot(controller)] =
                    self.complete(async move { connect_app(handle).await });
            }
        }
        self.advance_for(150);
    }
    pub fn issue(&self, node: usize, command: PrnsCommand) -> CommandId {
        issue(&self.nodes[node].handle, command)
    }
    pub fn events(&self) -> Vec<(usize, u64, Vec<crate::node_events::Event>)> {
        self.retired_events
            .iter()
            .map(|(index, generation, events)| (*index, *generation, events.snapshot()))
            .chain(
                self.nodes
                    .iter()
                    .enumerate()
                    .map(|(index, node)| (index, self.generations[index], node.events.snapshot())),
            )
            .collect()
    }
}
pub fn controller_slot(controller: usize) -> usize {
    match controller {
        PRIMARY => 0,
        HEALTHY => 1,
        _ => unreachable!("controller role required"),
    }
}
pub async fn exchange(
    handle: Handle,
    link: LinkId,
    request: personal_rns::remote_control::RemoteControlRequest,
) -> Result<personal_rns::remote_control::RemoteControlResponse, SendError<SendRequestFailure>> {
    let mut bytes = [0; personal_rns::remote_control::RemoteControlRequest::MAX_ENCODED_LEN];
    let len = request.write_into(&mut bytes).expect("bounded request");
    handle.raw(link, bytes[..len].to_vec()).await.map(|bytes| {
        personal_rns::remote_control::RemoteControlResponse::parse(&bytes).expect("typed response")
    })
}
pub async fn app_request(
    handle: Handle,
    link: LinkId,
    path: &'static str,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, SendError<SendRequestFailure>> {
    let path = RequestPathHash::of(path);
    let timeout = RequestResponseTimeout::Exact(DurationMillis(OPERATION_BUDGET_MS));
    match handle {
        Handle::Tokio(handle) => handle
            .request_with_options(
                link,
                path,
                &bytes,
                RequestOptions {
                    response_timeout: timeout,
                    maximum_response_bytes: ByteLimit::Maximum(
                        traffic::LARGE_RESPONSE_BYTES as u64,
                    ),
                },
            )
            .await
            .map(|(bytes, _)| bytes),
        Handle::Embassy(handle) => handle
            .request_with_response_timeout(link, path, &bytes, timeout)
            .await
            .map(|(bytes, _)| bytes.as_slice().to_vec()),
    }
}
fn issue(handle: &Handle, command: PrnsCommand) -> CommandId {
    match handle {
        Handle::Tokio(handle) => handle.issue(command),
        Handle::Embassy(handle) => handle.issue(command),
    }
    .expect("command admission")
}
#[derive(Debug, PartialEq, Eq)]
pub struct Trace {
    pub wire: serde_json::Value,
    pub persistence: [serde_json::Value; 3],
    pub events: Vec<(usize, u64, Vec<crate::node_events::Event>)>,
    pub crypto: Vec<(
        usize,
        u64,
        Vec<prns_runtime_tokio::runtime::ControlledCryptoEvent>,
    )>,
    pub retained_static: crate::static_storage::Footprint,
}
mod nodes;
use nodes::{announce_app, connect_app, converge, pair, reach, retain_control, runtime, start};
mod bootstrap;
use bootstrap::{route_counts, wait_app_routes};
pub(super) use bootstrap::{with_triple, with_triple_layout};
