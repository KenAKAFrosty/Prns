use super::*;
use crate::scenario::POLL_BUDGET;
use personal_rns::wire::{WireContext, WirePacketHeader};
use prns_simulation::{DeliveryCopy, EndpointId, ManualTaskPoll};

const CLIENT: usize = 0;
const SERVER: usize = 2;
const CONTROL_PAYLOAD: &[u8] = b"uninterrupted boundary control";

#[derive(Clone, Copy)]
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

    fn sender(self, side: Side) -> usize {
        match (self, side) {
            (Self::Request, Side::Left) => CLIENT,
            (Self::Request, Side::Right) => Side::Left.transport(),
            (Self::Response, Side::Left) => Side::Right.transport(),
            (Self::Response, Side::Right) => SERVER,
        }
    }
}

struct Ingress {
    sender: EndpointId,
    receiver: EndpointId,
}

impl Ingress {
    fn new(nodes: &[LiveNode], side: Side, boundary: Boundary) -> Self {
        let receiver = nodes[side.transport()]
            .ports
            .iter()
            .find(|port| port.port.peer().owner() == boundary.sender(side))
            .unwrap_or_else(|| unreachable!("transport has the requested ingress"));
        Self {
            sender: topology::binding(nodes, receiver.port.peer()).endpoint,
            receiver: receiver.endpoint,
        }
    }
}

fn stop_at_ingress(
    runner: &mut ManualTaskRunner<'_, Completion>,
    medium: &VirtualMedium,
    ingress: &Ingress,
    boundary: Boundary,
    link: LinkId,
    history_start: usize,
) {
    for _ in 0..POLL_BUDGET {
        assert!(matches!(
            runner.poll_next().ok(),
            Some(ManualTaskPoll::Pending { .. })
        ));
        let at = tick(now(runner));
        let reached = medium.inspect_trace(|trace| {
            assert_eq!(trace.discarded_events, 0);
            let accepted = trace.events().skip(history_start).find_map(|event| {
                let MediumEvent::TransmissionAccepted {
                    ordinal,
                    from,
                    frame,
                    ..
                } = event
                else {
                    return None;
                };
                (*from == ingress.sender
                    && WirePacketHeader::parse(frame).is_ok_and(|(header, _)| {
                        header.context == boundary.context() && header.address == link.to_address()
                    }))
                .then_some(*ordinal)
            });
            let Some(ordinal) = accepted else {
                return false;
            };
            assert!(trace.events().skip(history_start).any(|event| {
                *event
                    == MediumEvent::ReceptionQueued {
                        ordinal,
                        to: ingress.receiver,
                        copy: DeliveryCopy::Original,
                        at,
                    }
            }));
            true
        });
        if !reached {
            continue;
        }
        // One actor poll accepted and queued the frame. The receiving node
        // has not been polled again, so cancellation cuts a known boundary.
        return;
    }
    unreachable!("real request/response must reach the selected transport ingress");
}

fn exercise_boundary(side: Side, boundary: Boundary) {
    with_fleet(|runner, medium, nodes| {
        discover(runner, nodes);
        let [first, second] = side.unaffected_pair();
        let survivor = establish(runner, nodes, first, second);
        let mut crossing = establish(runner, nodes, CLIENT, SERVER);
        for _ in 0..RESTARTS {
            let ingress = Ingress::new(nodes, side, boundary);
            let history = medium.inspect_trace(|trace| trace.events().len());
            let control = request(
                runner,
                CLIENT,
                nodes[CLIENT].control.handle.clone(),
                crossing,
                CONTROL_PAYLOAD.to_vec(),
            );
            stop_at_ingress(runner, medium, &ingress, boundary, crossing, history);
            assert_eq!(
                settle(runner),
                [(
                    control,
                    Completion::Response {
                        node: CLIENT,
                        bytes: CONTROL_PAYLOAD.to_vec()
                    }
                )],
                "without teardown the same boundary must complete normally"
            );

            let started = now(runner);
            let history = medium.inspect_trace(|trace| trace.events().len());
            let pending = timeouts(runner, nodes, &[(CLIENT, crossing)]);
            stop_at_ingress(runner, medium, &ingress, boundary, crossing, history);
            let before = runner
                .snapshot()
                .unwrap_or_else(|error| unreachable!("pre-cancellation clock: {error}"));
            assert_eq!(
                runner.cancel(nodes[side.transport()].task).ok(),
                Some(ManualTaskCancellation::Cancelled)
            );
            assert_eq!(runner.snapshot().ok(), Some(before));
            assert!(settle(runner).is_empty());
            assert_eq!(runner.task_count(), NODE_COUNT);
            echo(
                runner,
                nodes,
                first,
                survivor,
                b"survivor during inflight loss",
            );
            expire(runner, started, pending);
            assert_eq!(runner.task_count(), NODE_COUNT - 1);

            rebuild(runner, medium, nodes, side);
            assert_empty_routes(runner, &nodes[side.transport()], side.transport());
            topology::connect_node(medium, nodes, side.transport());
            discover(runner, nodes);
            let fresh = establish(runner, nodes, CLIENT, SERVER);
            assert_ne!(fresh, crossing);
            echo(
                runner,
                nodes,
                CLIENT,
                fresh,
                b"new link after interrupted forwarding",
            );
            echo(
                runner,
                nodes,
                first,
                survivor,
                b"original unaffected link after recovery",
            );
            crossing = fresh;
        }
    });
}

#[test]
fn left_transport_restart_with_request_queued_at_ingress() {
    exercise_boundary(Side::Left, Boundary::Request);
}

#[test]
fn right_transport_restart_with_request_queued_at_ingress() {
    exercise_boundary(Side::Right, Boundary::Request);
}

#[test]
fn left_transport_restart_with_response_queued_at_ingress() {
    exercise_boundary(Side::Left, Boundary::Response);
}

#[test]
fn right_transport_restart_with_response_queued_at_ingress() {
    exercise_boundary(Side::Right, Boundary::Response);
}
