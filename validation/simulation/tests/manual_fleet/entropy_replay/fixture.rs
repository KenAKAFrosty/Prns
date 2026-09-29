use super::*;
use personal_rns::interfaces::InterfaceId;
use personal_rns::manifold::{interface_seam::Interface, tokio::TokioHost};
use personal_rns::runtime::PrnsNodeHandle;
use prns_simulation::{EndpointId, ManualTaskCancellation, ManualTaskId};
use scenario::{add_node_with_sources, NodeControl, TIMELINE_ORIGIN};
use source::Reseed;

const NODES: usize = 2;
const RESTART_TICK: u64 = 7;

#[derive(Clone, Copy)]
pub(super) enum Scenario {
    Fresh,
    Restart { receiver_seed: u8 },
    ReseedSuccess { fresh_seed: u8 },
    ReseedFailure,
    OwnedInputs { shared_seed: u8, path_seed: u8 },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Transcript {
    pub trace: Vec<MediumEvent>,
    pub checkpoints: Vec<usize>,
    pub responses: Vec<Vec<u8>>,
    pub source_calls: Vec<Read>,
    pub inputs: Vec<inputs::Observation>,
}

struct Node {
    task: ManualTaskId,
    control: NodeControl,
    endpoint: EndpointId,
    interface: InterfaceId,
}

fn start(
    runner: &mut ManualTaskRunner<'_, Completion>,
    medium: &VirtualMedium,
    boot: BootId,
    seed: u8,
    reseed: Reseed,
    calls: Rc<RefCell<Vec<Read>>>,
    inputs: inputs::Inputs,
) -> Node {
    let interface = medium
        .attach(&(boot.node as u64).to_be_bytes())
        .unwrap_or_else(|error| unreachable!("unique replay interface: {error}"));
    let endpoint = interface.endpoint_id();
    let id = interface.descriptor().id;
    let interface = inputs.interface(interface, boot);
    let (task, mut ready) = add_node_with_sources(
        runner,
        NodeSpec {
            index: boot.node,
            role: NodeRole::Endpoint,
            attach_interfaces: move |handle: &PrnsNodeHandle| {
                let _attached = handle.add_interface(interface);
            },
            heard: Rc::new(RefCell::new(Vec::new())),
            heard_capacity: nonzero(1),
        },
        move |origin| {
            (
                TokioHost::with_runtime_entropy(origin, source::stream(boot, seed, reseed, calls)),
                inputs.entropy(boot),
            )
        },
    );
    assert!(settle(runner).is_empty());
    let control = ready
        .try_recv()
        .unwrap_or_else(|error| unreachable!("initialized replay node: {error}"));
    let elapsed = runner
        .snapshot()
        .unwrap_or_else(|error| unreachable!("replay clock: {error}"))
        .runtime_elapsed;
    assert_eq!(
        control.origin.0,
        TIMELINE_ORIGIN.0
            + u64::try_from(elapsed.as_millis())
                .unwrap_or_else(|_| unreachable!("bounded elapsed time"))
    );
    Node {
        task,
        control,
        endpoint,
        interface: id,
    }
}

fn exchange(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[Node; NODES],
    payload: &[u8],
) -> Vec<u8> {
    let remote = destination(1)
        .destination_hash()
        .unwrap_or_else(|error| unreachable!("replay destination: {error:?}"));
    announce(&nodes[1].control, remote);
    assert!(settle(runner).is_empty());
    let handle = nodes[0].control.handle.clone();
    let linking = runner
        .insert(async move {
            Completion::Linked {
                node: 0,
                link: handle
                    .establish_link(remote)
                    .await
                    .unwrap_or_else(|error| unreachable!("replay link: {error:?}")),
            }
        })
        .unwrap_or_else(|error| unreachable!("bounded link actor: {error}"));
    let completed = settle(runner);
    let [(task, Completion::Linked { node: 0, link })] = completed.as_slice() else {
        unreachable!("one link completion: {completed:?}")
    };
    assert_eq!(*task, linking);
    let requested = request(
        runner,
        0,
        nodes[0].control.handle.clone(),
        *link,
        payload.to_vec(),
    );
    let mut completed = settle(runner);
    assert_eq!(completed.len(), 1);
    let (
        task,
        Completion::Response {
            node: 0,
            bytes: response,
        },
    ) = completed.remove(0)
    else {
        unreachable!("one replay response")
    };
    assert_eq!((task, response.as_slice()), (requested, payload));
    response
}

pub(super) fn run(seed: u8, payload: &[u8], scenario: Scenario) -> Transcript {
    let medium = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::FullyConnected,
            NODES,
            16,
            32,
            512,
            FaultPlan::none(),
        )
        .unwrap_or_else(|error| unreachable!("bounded replay medium: {error}")),
    );
    let mut driver = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("replay clock: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(NODES + 1));
    let source_calls = Rc::new(RefCell::new(Vec::new()));
    let reseed = match scenario {
        Scenario::Fresh | Scenario::Restart { .. } | Scenario::OwnedInputs { .. } => {
            Reseed::NotReached
        }
        Scenario::ReseedSuccess { fresh_seed } => Reseed::Succeed(fresh_seed),
        Scenario::ReseedFailure => Reseed::Fail,
    };
    let inputs = match scenario {
        Scenario::OwnedInputs {
            shared_seed,
            path_seed,
        } => inputs::Inputs::new(shared_seed, path_seed),
        Scenario::Fresh
        | Scenario::Restart { .. }
        | Scenario::ReseedSuccess { .. }
        | Scenario::ReseedFailure => inputs::Inputs::new(0x57, 0xA3),
    };
    let mut nodes = [0, 1].map(|node| {
        start(
            &mut runner,
            &medium,
            BootId {
                node,
                generation: 0,
            },
            seed.checked_add(node as u8)
                .unwrap_or_else(|| unreachable!("bounded replay seed")),
            reseed,
            Rc::clone(&source_calls),
            inputs.clone(),
        )
    });
    let mut effective_payload = payload.to_vec();
    if let Scenario::OwnedInputs { .. } = scenario {
        let handle = nodes[0].control.handle.clone();
        effective_payload.extend_from_slice(&inputs.handle_bytes(&handle));
        let remote = destination(1)
            .destination_hash()
            .unwrap_or_else(|error| unreachable!("path destination: {error:?}"));
        let task = runner
            .insert(async move {
                assert_eq!(
                    handle.request_path(remote).await,
                    Ok(personal_rns::engine::PathFound {
                        hops: personal_rns::units::HopCount(1)
                    })
                );
                Completion::RoutesChecked { node: 0 }
            })
            .unwrap_or_else(|error| unreachable!("path actor: {error}"));
        assert_eq!(
            settle(&mut runner),
            [(task, Completion::RoutesChecked { node: 0 })]
        );
    }
    let mut responses = vec![exchange(&mut runner, &nodes, &effective_payload)];
    let mut checkpoints = vec![medium.trace().events.len()];
    let expected_attachments = match scenario {
        Scenario::Restart { receiver_seed } => {
            assert_ne!(
                receiver_seed,
                seed + 1,
                "restart uses distinct source material"
            );
            assert!(runner.advance_to_next_event(tick(RESTART_TICK)).is_ok());
            assert_eq!(medium.now(), tick(RESTART_TICK));
            assert!(settle(&mut runner).is_empty());
            let old = &nodes[1];
            assert_eq!(
                runner.cancel(old.task).ok(),
                Some(ManualTaskCancellation::Cancelled)
            );
            assert_eq!(runner.task_count(), 1);
            assert_eq!(medium.pending_delivery_count(), 0);
            let replacement = start(
                &mut runner,
                &medium,
                BootId {
                    node: 1,
                    generation: 1,
                },
                receiver_seed,
                Reseed::NotReached,
                Rc::clone(&source_calls),
                inputs.clone(),
            );
            assert_ne!(replacement.task, old.task);
            assert_ne!(replacement.endpoint, old.endpoint);
            assert_eq!(replacement.interface, old.interface);
            assert_eq!(
                replacement.control.origin.0,
                TIMELINE_ORIGIN.0 + RESTART_TICK
            );
            nodes[1] = replacement;
            responses.push(exchange(&mut runner, &nodes, payload));
            checkpoints.push(medium.trace().events.len());
            NODES + 1
        }
        Scenario::Fresh
        | Scenario::ReseedSuccess { .. }
        | Scenario::ReseedFailure
        | Scenario::OwnedInputs { .. } => NODES,
    };
    let expected: BTreeMap<_, _> = nodes
        .into_iter()
        .enumerate()
        .map(|(node, live)| {
            assert_eq!(live.control.shutdown.send(()), Ok(()));
            (
                live.task,
                Completion::Stopped {
                    node,
                    result: Ok(()),
                },
            )
        })
        .collect();
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(
        (runner.task_count(), medium.pending_delivery_count()),
        (0, 0)
    );
    let trace = medium.trace();
    assert_eq!(trace.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in &trace.events {
        match event {
            MediumEvent::EndpointAttached { endpoint, .. } => attached.push(*endpoint),
            MediumEvent::EndpointDetached { endpoint } => detached.push(*endpoint),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), expected_attachments);
    assert_eq!(attached, detached);
    let calls = source_calls.borrow().clone();
    if let Scenario::Fresh = scenario {
        assert_eq!(
            calls,
            [
                Read::initial(
                    BootId {
                        node: 0,
                        generation: 0
                    },
                    seed
                ),
                Read::initial(
                    BootId {
                        node: 1,
                        generation: 0
                    },
                    seed + 1
                )
            ]
        );
    }
    Transcript {
        trace: trace.events,
        checkpoints,
        responses,
        source_calls: calls,
        inputs: inputs.snapshot(),
    }
}
