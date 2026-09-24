use super::*;
use crate::clock::{ClockLease, CompletionBudget, EmbassyTasks};
use adapter::{Runtime, PAIRS};
use personal_rns::engine::SendRequestFailure;
use personal_rns::interfaces::bluetooth_auto::BleAddress;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::SendError;
use prns_simulation::ble::{BleMediumConfig, BleWireCapture, VirtualBleLab};
use prns_simulation::{
    ManualMedium, ManualTaskScheduling, ManualTimeDriver, Reachability, TopologyConfig,
};
use std::num::NonZeroUsize;
use std::time::Duration;

pub struct Pair<'a, 'clock> {
    pub tasks: &'a mut EmbassyTasks<'clock>,
    pub lab: &'a VirtualBleLab,
    pub nodes: [node::Node; 2],
    pub storage: [node::Storage; 2],
    pub generations: [u64; 2],
    pub messages: Messages,
    pub link: LinkId,
}
impl Pair<'_, '_> {
    pub fn complete<T: 'static>(
        &mut self,
        future: impl std::future::Future<Output = T> + 'static,
    ) -> T {
        self.tasks.complete_with_budget(
            CompletionBudget {
                deadline: crate::tick(self.tasks.snapshot().tick.get() + 1_000),
                polls_per_tick: NonZeroUsize::new(8192).expect("poll budget"),
            },
            future,
        )
    }
    pub fn exchange(
        &mut self,
        request: RemoteControlRequest,
    ) -> Result<RemoteControlResponse, SendError<SendRequestFailure>> {
        let mut bytes = [0; RemoteControlRequest::MAX_ENCODED_LEN];
        let len = request.write_into(&mut bytes).expect("bounded request");
        let bytes = bytes[..len].to_vec();
        let handle = self.nodes[CONTROLLER].handle.clone();
        let link = self.link;
        self.complete(async move {
            handle
                .raw(link, bytes)
                .await
                .map(|bytes| RemoteControlResponse::parse(&bytes).expect("typed response"))
        })
    }
    pub fn grant(&mut self, requests: RemoteControlRequestSet) {
        let handle = self.nodes[TARGET].handle.clone();
        let grant = RemoteControlControllerGrant::new(
            *secrets(CONTROLLER).identities().controller(),
            RemoteControlControllerAuthority::Administrator,
            requests,
        )
        .expect("explicit authority");
        assert!(self
            .complete(async move { handle.grant(grant).await })
            .is_ok());
    }
    pub fn reconnect(&mut self) {
        self.nodes[CONTROLLER].handle.close(self.link);
        self.tasks.settle();
        // Route evidence uses whole origin seconds; a replacement announcement must be newer than the departed path.
        self.tasks
            .advance(crate::tick(self.tasks.snapshot().tick.get() + 1_000))
            .expect("new discovery evidence clock");
        self.tasks.settle();
        let target = self.nodes[TARGET].handle.clone();
        self.complete(async move { target.announce().await });
        self.tasks.settle();
        let handle = self.nodes[CONTROLLER].handle.clone();
        self.link = self.complete(async move { handle.connect().await });
        self.tasks
            .advance(crate::tick(self.tasks.snapshot().tick.get() + 150))
            .expect("identification clock");
        self.tasks.settle();
    }
    pub fn restart(&mut self, index: usize) {
        self.lab
            .set_reachability(
                BleAddress::new([1; 6]),
                BleAddress::new([2; 6]),
                Reachability::Isolated,
            )
            .expect("remove old radio path");
        self.tasks.cancel(self.nodes[index].task);
        self.tasks.settle();
        self.generations[index] += 1;
        self.nodes[index] = node::start(
            self.tasks,
            self.lab,
            index,
            self.generations[index],
            self.storage[index].clone(),
            self.messages.clone(),
        );
        self.lab
            .set_reachability(
                BleAddress::new([1; 6]),
                BleAddress::new([2; 6]),
                Reachability::Reachable,
            )
            .expect("restore radio path");
        converge(self.tasks, self.lab, &self.nodes);
        self.reconnect();
    }
}
fn converge(tasks: &mut EmbassyTasks<'_>, lab: &VirtualBleLab, nodes: &[node::Node; 2]) {
    let horizon =
        crate::tick(tasks.snapshot().tick.get() + crate::fixture::ADVERTISING_INTERVAL_MS * 3);
    for _ in 0..4096 {
        tasks.settle();
        if lab.active_connection_count() == 1
            && nodes.iter().all(|node| {
                node.inspection.snapshots().iter().any(|snapshot| {
                    matches!(
                        snapshot.membership,
                        personal_rns::interfaces::Membership::FleetMember { .. }
                    ) && snapshot.connection == personal_rns::interfaces::ConnectionState::Connected
                })
            })
        {
            return;
        }
        tasks
            .advance_to_next_wake(horizon)
            .expect("discovery clock");
    }
    panic!(
        "BLE convergence budget at {:?}: {:?}; {:?}",
        tasks.snapshot().tick,
        nodes.each_ref().map(|node| node.inspection.snapshots()),
        lab.trace().events.iter().rev().take(8).collect::<Vec<_>>()
    );
}
pub fn with_pair<T>(
    runtimes: [Runtime; 2],
    scheduling: ManualTaskScheduling,
    scenario: impl FnOnce(&mut Pair<'_, '_>) -> T,
) -> (T, Vec<WireValue>, [serde_json::Value; 2]) {
    with_pair_execution(
        runtimes,
        scheduling,
        std::array::from_fn(|_| personal_rns::runtime::CryptoPoolConfig::Inline),
        scenario,
    )
}
fn with_pair_execution<T>(
    runtimes: [Runtime; 2],
    scheduling: ManualTaskScheduling,
    crypto: [personal_rns::runtime::CryptoPoolConfig; 2],
    scenario: impl FnOnce(&mut Pair<'_, '_>) -> T,
) -> (T, Vec<WireValue>, [serde_json::Value; 2]) {
    let clock = ClockLease::acquire();
    let capture = BleWireCapture::new(NonZeroUsize::new(32768).expect("capture capacity"));
    let lab = VirtualBleLab::with_wire_capture(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: NonZeroUsize::MIN,
            },
            2,
            16,
            16,
            262144,
        )
        .expect("medium"),
        capture.clone(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1))
            .expect("clock");
    let mut tasks = EmbassyTasks::with_limits(
        &mut driver,
        clock,
        NonZeroUsize::new(16).expect("actor budget"),
        NonZeroUsize::new(8192).expect("poll budget"),
        scheduling,
    );
    let storage = runtimes.map(node::Storage::new);
    let messages = Messages(
        Rc::new(RefCell::new(Vec::new())),
        Rc::new(RefCell::new(Vec::new())),
    );
    let mut crypto = crypto.into_iter();
    let nodes = std::array::from_fn(|index| {
        node::start_with_crypto(
            &mut tasks,
            &lab,
            index,
            0,
            storage[index].clone(),
            messages.clone(),
            crypto.next().expect("one crypto configuration per node"),
        )
    });
    lab.set_reachability(
        BleAddress::new([1; 6]),
        BleAddress::new([2; 6]),
        Reachability::Reachable,
    )
    .expect("topology");
    converge(&mut tasks, &lab, &nodes);
    let target = nodes[TARGET].handle.clone();
    let opened = tasks.complete_ready(async move { target.open_pairing().await });
    let opened = pairing::observed_offer(&messages.1, opened);
    let controller = nodes[CONTROLLER].handle.clone();
    tasks.complete_with_budget(
        CompletionBudget {
            deadline: crate::tick(tasks.snapshot().tick.get() + 1_000),
            polls_per_tick: NonZeroUsize::new(8192).expect("pairing poll budget"),
        },
        async move { controller.initiate(opened).await },
    );
    let controller_confirmation = messages
        .1
        .borrow_mut()
        .iter()
        .position(|event| matches!(event, pairing::Observation::Controller(_)))
        .expect("controller callback");
    let pairing::Observation::Controller(controller_confirmation) =
        messages.1.borrow_mut().remove(controller_confirmation)
    else {
        unreachable!("controller confirmation")
    };
    let target_confirmation = messages
        .1
        .borrow_mut()
        .iter()
        .position(|event| matches!(event, pairing::Observation::Target(_)))
        .expect("target callback");
    let pairing::Observation::Target(target_confirmation) =
        messages.1.borrow_mut().remove(target_confirmation)
    else {
        unreachable!("target confirmation")
    };
    assert_eq!(
        controller_confirmation.confirmation(),
        target_confirmation.confirmation()
    );
    let target = nodes[TARGET].handle.clone();
    tasks.complete_ready(async move { target.approve_target(target_confirmation).await });
    let controller = nodes[CONTROLLER].handle.clone();
    tasks.complete_ready(
        async move { controller.approve_controller(controller_confirmation).await },
    );
    assert_eq!(
        *messages.1.borrow(),
        [
            pairing::Observation::TargetPersisted,
            pairing::Observation::ControllerPersisted
        ]
    );
    let target = nodes[TARGET].handle.clone();
    tasks.complete_ready(async move { target.announce().await });
    let controller = nodes[CONTROLLER].handle.clone();
    let link = tasks.complete_ready(async move { controller.connect().await });
    tasks
        .advance(crate::tick(tasks.snapshot().tick.get() + 150))
        .expect("identification clock");
    tasks.settle();
    let mut pair = Pair {
        tasks: &mut tasks,
        lab: &lab,
        nodes,
        storage,
        generations: [0; 2],
        messages,
        link,
    };
    pair.grant(requests());
    let result = scenario(&mut pair);
    let persistence = pair.storage.each_ref().map(node::Storage::trace);
    for node in pair.nodes {
        pair.tasks.cancel(node.task);
    }
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
    let wire = capture.snapshot();
    assert_eq!(wire.discarded_values, 0);
    assert_eq!(lab.trace().discarded_events, 0);
    let mut connections = Vec::new();
    let wire = wire
        .values
        .into_iter()
        .map(|value| {
            let connection = match connections.iter().position(|id| *id == value.connection) {
                Some(index) => index,
                None => {
                    connections.push(value.connection);
                    connections.len() - 1
                }
            };
            WireValue {
                connection,
                from: *value.from.octets(),
                to: *value.to.octets(),
                channel: format!("{:?}", value.channel),
                bytes: value.bytes,
            }
        })
        .collect();
    (result, wire, persistence)
}

#[test]
fn controlled_workers_run_real_remote_control_links_across_runtime_pairings() {
    use personal_rns::runtime::CryptoPoolConfig;
    use prns_runtime_tokio::runtime::{ControlledCrypto, ControlledCryptoEvent};
    for runtimes in PAIRS {
        if runtimes == [Runtime::Embassy, Runtime::Embassy] {
            continue;
        }
        for workers in [1, 4] {
            let run = || {
                let controls = runtimes.each_ref().map(|runtime| match runtime {
                    Runtime::Embassy => None,
                    Runtime::Tokio => Some(ControlledCrypto::new(
                        NonZeroUsize::new(workers).expect("workers"),
                        NonZeroUsize::new(32768).expect("trace"),
                    )),
                });
                let crypto = controls.each_ref().map(|control| match control {
                    None => CryptoPoolConfig::Inline,
                    Some(control) => CryptoPoolConfig::Controlled(control.clone()),
                });
                let result = with_pair_execution(
                    runtimes.clone(),
                    ManualTaskScheduling::Cyclic,
                    crypto,
                    |pair| {
                        let payload =
                            RemoteControlAppMessage::from_slice(b"real-worker").expect("bounded");
                        assert_eq!(
                            pair.exchange(RemoteControlRequest::AppMessage(payload.clone())),
                            Ok(RemoteControlResponse::AppMessage(payload))
                        );
                    },
                );
                let traces = controls.map(|control| {
                    control.map(|control| {
                        let trace = control.trace().expect("worker trace");
                        assert!(trace
                            .iter()
                            .any(|event| matches!(event, ControlledCryptoEvent::Consumed { .. })));
                        assert!(matches!(
                            trace.last(),
                            Some(ControlledCryptoEvent::Retired { .. })
                        ));
                        trace
                    })
                });
                (result, traces)
            };
            assert_eq!(
                run(),
                run(),
                "exact controlled replay {runtimes:?}, {workers} workers"
            );
        }
    }
}

#[test]
fn inspection_authority_and_recovery_contract_runs_through_all_four_runtime_pairings() {
    for runtimes in PAIRS {
        let run = || {
            with_pair(runtimes.clone(), ManualTaskScheduling::Cyclic, |pair| {
                let RemoteControlResponse::Describe(description) = pair
                    .exchange(RemoteControlRequest::Describe)
                    .expect("description")
                else {
                    unreachable!("describe")
                };
                assert_eq!(
                    description
                        .available_requests()
                        .supports(RemoteControlRequestKind::WatchInterfaces),
                    matches!(runtimes[TARGET], Runtime::Tokio)
                );
                let build = pair
                    .exchange(RemoteControlRequest::DescribeBuild)
                    .expect("build");
                assert_eq!(
                    build,
                    RemoteControlResponse::DescribeBuild(
                        RemoteControlBuildVersion::from_text("simulation-v1").expect("build label")
                    )
                );
                let payload = RemoteControlAppMessage::from_slice(b"verified").expect("bounded");
                assert_eq!(
                    pair.exchange(RemoteControlRequest::AppMessage(payload.clone())),
                    Ok(RemoteControlResponse::AppMessage(payload))
                );
                assert_eq!(
                    pair.messages.0.borrow().as_slice(),
                    [Invocation {
                        identity: secrets(CONTROLLER)
                            .identities()
                            .controller()
                            .identity_hash(),
                        payload: b"verified".to_vec()
                    }]
                );
                pair.grant(RemoteControlRequestSet::only(
                    RemoteControlRequestKind::Describe,
                ));
                let before = pair.tasks.snapshot().tick;
                assert_eq!(
                    pair.exchange(RemoteControlRequest::DescribeBuild),
                    Err(SendError::Failed(SendRequestFailure::Timeout))
                );
                assert_eq!(
                    pair.tasks.snapshot().tick.get() - before.get(),
                    REQUEST_TIMEOUT_MS
                );
                pair.restart(TARGET);
                assert_eq!(
                    pair.exchange(RemoteControlRequest::DescribeBuild),
                    Err(SendError::Failed(SendRequestFailure::Timeout))
                );
                pair.grant(requests());
                pair.restart(CONTROLLER);
                assert_eq!(
                    pair.exchange(RemoteControlRequest::DescribeBuild),
                    Ok(build.clone())
                );
                pair.restart(TARGET);
                assert_eq!(
                    pair.exchange(RemoteControlRequest::DescribeBuild),
                    Ok(build)
                );
            })
        };
        assert_eq!(
            run(),
            run(),
            "byte and native persistence replay {runtimes:?}"
        );
    }
}
