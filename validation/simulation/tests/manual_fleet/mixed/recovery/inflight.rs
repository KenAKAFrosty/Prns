use super::*;
use crate::scenario::POLL_BUDGET;
use personal_rns::wire::{WireContext, WirePacketHeader};
use prns_simulation::{
    DeliveryCopy, EndpointId, ManualTaskPoll, ManualTaskScheduling, SimulationSeed,
};

const CONTROL_PAYLOAD: &[u8] = b"mixed ingress boundary control";

#[derive(Clone, Copy, Debug)]
enum Cut {
    FramesOnly,
    BothMedia,
}

enum Settlement {
    Delivered(BTreeMap<ManualTaskId, Completion>),
    TimedOut(BTreeMap<ManualTaskId, Completion>),
}

#[derive(Clone, Copy, Debug)]
enum Boundary {
    Request,
    Response,
}

impl Boundary {
    fn context(self) -> WireContext {
        match self {
            Self::Request => WireContext::Request,
            Self::Response => WireContext::Response,
        }
    }

    fn requester(self) -> usize {
        match self {
            Self::Request => 0,
            Self::Response => BLE_PEER,
        }
    }

    fn responder(self) -> usize {
        match self {
            Self::Request => BLE_PEER,
            Self::Response => 0,
        }
    }
}

struct FrameIngress {
    sender: EndpointId,
    bridge: EndpointId,
    sender_task: ManualTaskId,
}

impl FrameIngress {
    fn inspect(frames: &VirtualMedium, sender_task: ManualTaskId) -> Self {
        frames.inspect_trace(|trace| {
            assert_eq!(trace.discarded_events, 0);
            let endpoints: BTreeMap<_, _> = trace
                .events()
                .filter_map(|event| match event {
                    MediumEvent::EndpointAttached {
                        endpoint,
                        channel_tag,
                    } => Some((channel_tag.as_slice(), *endpoint)),
                    _ => None,
                })
                .collect();
            assert_eq!(endpoints.len(), 2);
            Self {
                sender: endpoints[b"recovery-client".as_slice()],
                bridge: endpoints[b"recovery-bridge".as_slice()],
                sender_task,
            }
        })
    }

    fn set_reachability(&self, frames: &VirtualMedium, state: Reachability) {
        assert_eq!(
            frames.set_reachability(self.sender, self.bridge, state),
            Ok(TopologyMutation::Applied)
        );
    }

    fn stop_before_bridge_poll(
        &self,
        runner: &mut ManualTaskRunner<'_, Completion>,
        frames: &VirtualMedium,
        boundary: Boundary,
        link: LinkId,
        history_start: usize,
    ) {
        let before = runner
            .snapshot()
            .unwrap_or_else(|error| unreachable!("ingress clock: {error}"));
        for _ in 0..POLL_BUDGET {
            let Some(ManualTaskPoll::Pending { task }) = runner.poll_next().ok() else {
                unreachable!("ingress observation requires a pending actor poll");
            };
            let reached = frames.inspect_trace(|trace| {
                assert_eq!(trace.discarded_events, 0);
                let ordinal = trace.events().skip(history_start).find_map(|event| {
                    let MediumEvent::TransmissionAccepted {
                        ordinal,
                        from,
                        frame,
                        ..
                    } = event
                    else {
                        return None;
                    };
                    (*from == self.sender
                        && WirePacketHeader::parse(frame).is_ok_and(|(header, _)| {
                            header.context == boundary.context()
                                && header.address == link.to_address()
                        }))
                    .then_some(*ordinal)
                });
                let Some(ordinal) = ordinal else {
                    return false;
                };
                assert!(trace.events().skip(history_start).any(|event| *event
                    == MediumEvent::ReceptionQueued {
                        ordinal,
                        to: self.bridge,
                        copy: DeliveryCopy::Original,
                        at: before.tick,
                    }));
                true
            });
            if reached {
                // Only the sending node was polled when it queued this frame.
                // The receiving bridge cannot consume it before the next actor poll.
                assert_eq!(task, self.sender_task);
                assert_eq!(runner.snapshot().ok(), Some(before));
                return;
            }
        }
        unreachable!("bounded real request/response must reach bridge frame ingress");
    }
}

fn ble_reachability(ble: &VirtualBleLab, state: Reachability) {
    assert_eq!(
        ble.set_reachability(
            BleAddress::new([BRIDGE as u8; 6]),
            BleAddress::new([BLE_PEER as u8; 6]),
            state
        ),
        Ok(TopologyMutation::Applied)
    );
}

fn exercise(boundary: Boundary, cut: Cut, profile: Profile, scheduling: ManualTaskScheduling) {
    with_bridge(
        profile,
        scheduling,
        2,
        |runner, frames, ble, _, nodes, frame_id| {
            converge(runner, ble, nodes);
            routes(runner, nodes, frame_id);
            let ingress = FrameIngress::inspect(frames, nodes[0].task);
            let crossing = link(runner, nodes, boundary.requester(), boundary.responder());
            let frame_local = link(runner, nodes, 0, BRIDGE);
            let ble_local = link(runner, nodes, BLE_PEER, BRIDGE);
            let tasks: Vec<_> = nodes.iter().map(|node| node.task).collect();
            for cycle in 0..CYCLES {
                let history = frames.inspect_trace(|trace| trace.events().len());
                let control = request(
                    runner,
                    boundary.requester(),
                    nodes[boundary.requester()].control.handle.clone(),
                    crossing,
                    CONTROL_PAYLOAD.to_vec(),
                );
                ingress.stop_before_bridge_poll(runner, frames, boundary, crossing, history);
                assert_eq!(
                    settle(runner),
                    [(
                        control,
                        Completion::Response {
                            node: boundary.requester(),
                            bytes: CONTROL_PAYLOAD.to_vec(),
                        }
                    )]
                );

                let started = frames.now();
                let history = frames.inspect_trace(|trace| trace.events().len());
                let pending =
                    match (boundary, cut) {
                        (Boundary::Response, Cut::FramesOnly) => {
                            let bytes = vec![0x30 + cycle as u8; 256];
                            let task = request(
                                runner,
                                boundary.requester(),
                                nodes[boundary.requester()].control.handle.clone(),
                                crossing,
                                bytes.clone(),
                            );
                            Settlement::Delivered(BTreeMap::from([(
                                task,
                                Completion::Response {
                                    node: boundary.requester(),
                                    bytes,
                                },
                            )]))
                        }
                        (Boundary::Request, Cut::FramesOnly | Cut::BothMedia)
                        | (Boundary::Response, Cut::BothMedia) => Settlement::TimedOut(
                            lost_requests(runner, nodes, &[(boundary.requester(), crossing)]),
                        ),
                    };
                ingress.stop_before_bridge_poll(runner, frames, boundary, crossing, history);
                let before = runner
                    .snapshot()
                    .unwrap_or_else(|error| unreachable!("outage clock: {error}"));
                ingress.set_reachability(frames, Reachability::Isolated);
                if let Cut::BothMedia = cut {
                    ble_reachability(ble, Reachability::Isolated);
                }
                let completed = settle(runner);
                assert_eq!(runner.snapshot().ok(), Some(before));
                match cut {
                    Cut::BothMedia => {
                        assert_eq!(ble.active_connection_count(), 0);
                        for node in [BRIDGE, BLE_PEER] {
                            assert!(member_inventory(&nodes[node].control.handle).is_empty());
                        }
                    }
                    Cut::FramesOnly => {
                        assert_eq!(ble.active_connection_count(), 1);
                        echo(runner, nodes, BLE_PEER, ble_local, 0x39 + cycle as u8);
                    }
                }
                match pending {
                    Settlement::Delivered(expected) => {
                        assert_eq!(completed.into_iter().collect::<BTreeMap<_, _>>(), expected);
                    }
                    Settlement::TimedOut(expected) => {
                        assert!(completed.is_empty());
                        expire(runner, frames, ble, started, expected);
                    }
                }
                assert_eq!(runner.task_count(), 3);

                ingress.set_reachability(frames, Reachability::Reachable);
                assert!(settle(runner).is_empty());
                if let Cut::BothMedia = cut {
                    let started = frames.now();
                    let unavailable = lost_requests(
                        runner,
                        nodes,
                        &[(boundary.requester(), crossing), (BLE_PEER, ble_local)],
                    );
                    echo(runner, nodes, 0, frame_local, 0x40 + cycle as u8);
                    expire(runner, frames, ble, started, unavailable);
                    ble_reachability(ble, Reachability::Reachable);
                }
                converge(runner, ble, nodes);
                routes(runner, nodes, frame_id);
                echo(
                    runner,
                    nodes,
                    boundary.requester(),
                    crossing,
                    0x60 + cycle as u8,
                );
                echo(runner, nodes, 0, frame_local, 0x70 + cycle as u8);
                echo(runner, nodes, BLE_PEER, ble_local, 0x80 + cycle as u8);
                assert_eq!(
                    nodes.iter().map(|node| node.task).collect::<Vec<_>>(),
                    tasks
                );
                assert_eq!(runner.task_count(), 3);
                traffic::clocks(runner, nodes, [tick(0); 3], frames, ble);
            }
        },
    );
}

fn matrix(boundary: Boundary, cut: Cut) {
    for profile in [Profile::AppleBridge, Profile::BluezBridge] {
        for scheduling in [
            ManualTaskScheduling::Cyclic,
            ManualTaskScheduling::Seeded {
                seed: SimulationSeed::new(0),
            },
            ManualTaskScheduling::Seeded {
                seed: SimulationSeed::new(7),
            },
            ManualTaskScheduling::Seeded {
                seed: SimulationSeed::new(u64::MAX),
            },
        ] {
            exercise(boundary, cut, profile, scheduling);
        }
    }
}

#[test]
fn mixed_outage_with_request_already_queued_at_bridge_ingress() {
    matrix(Boundary::Request, Cut::BothMedia);
}

#[test]
fn mixed_outage_with_response_already_queued_at_bridge_ingress() {
    matrix(Boundary::Response, Cut::BothMedia);
}

#[test]
fn mixed_outage_with_frame_cut_after_request_ingress_still_loses_return_path() {
    matrix(Boundary::Request, Cut::FramesOnly);
}

#[test]
fn mixed_outage_with_frame_cut_after_response_ingress_preserves_delivery() {
    matrix(Boundary::Response, Cut::FramesOnly);
}
