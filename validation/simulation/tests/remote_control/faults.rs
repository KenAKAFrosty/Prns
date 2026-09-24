use super::fixture::*;
use personal_rns::engine::SendRequestFailure;
use personal_rns::remote_control::*;
use personal_rns::runtime::SendError;
use personal_rns::wire::{WireContext, WirePacketHeader};
use prns_simulation::*;

#[derive(Debug, Clone, Copy)]
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
    fn calls_after_loss(self) -> usize {
        match self {
            Self::Request => 0,
            Self::Response => 1,
        }
    }
}
#[derive(Debug, Clone, Copy)]
enum Fault {
    Lost,
    Delayed,
    Duplicated,
    Late,
}
impl Fault {
    fn rule(self, ordinal: TransmissionOrdinal) -> TransmissionRule {
        match self {
            Self::Lost => TransmissionRule::drop(ordinal),
            Self::Delayed => {
                TransmissionRule::delay(ordinal, SimulationDurationInTicks::from_ticks(10))
            }
            Self::Duplicated => TransmissionRule::duplicate(
                ordinal,
                SimulationDurationInTicks::from_ticks(0),
                SimulationDurationInTicks::from_ticks(10),
            ),
            Self::Late => {
                TransmissionRule::delay(ordinal, SimulationDurationInTicks::from_ticks(60))
            }
        }
    }
}
const SEEDS: [u64; 6] = [0, 1, 7, 42, 0x5eed, u64::MAX];
fn payload() -> RemoteControlAppMessage {
    RemoteControlAppMessage::from_slice(b"fault-subject").expect("bounded message")
}

#[test]
fn generated_request_and_response_faults_preserve_admission_delivery_and_recovery() {
    for seed in SEEDS {
        let scheduling = ManualTaskScheduling::Seeded {
            seed: SimulationSeed::new(seed),
        };
        let (_, baseline) = with_lab(scheduling, FaultPlan::none(), |lab| {
            let link = lab.link(CONTROLLER);
            assert_eq!(
                lab.exchange(
                    CONTROLLER,
                    link,
                    RemoteControlRequest::AppMessage(payload())
                ),
                RemoteControlResponse::AppMessage(payload())
            );
        });
        for boundary in [Boundary::Request, Boundary::Response] {
            let ordinal = baseline
                .iter()
                .find_map(|event| match event {
                    MediumEvent::TransmissionAccepted { ordinal, frame, .. }
                        if WirePacketHeader::parse(frame)
                            .is_ok_and(|(header, _)| header.context == boundary.context()) =>
                    {
                        Some(*ordinal)
                    }
                    _ => None,
                })
                .expect("baseline crosses the selected wire boundary");
            for fault in [Fault::Lost, Fault::Delayed, Fault::Duplicated, Fault::Late] {
                let plan = FaultPlan::new(vec![fault.rule(ordinal)]).expect("one bounded fault");
                let run = || {
                    eprintln!("remote_control_fault_case seed={seed} boundary={boundary:?} fault={fault:?}");
                    with_lab(scheduling, plan.clone(), |lab| {
                        let link = lab.link(CONTROLLER);
                        let task = lab.request(
                            CONTROLLER,
                            link,
                            encoded(RemoteControlRequest::AppMessage(payload())),
                        );
                        let mut completed = lab.settle();
                        match fault {
                            Fault::Lost | Fault::Late => {
                                assert!(
                                    completed.is_empty(),
                                    "seed {seed}, {boundary:?}, {fault:?}"
                                );
                                completed = lab.advance(REQUEST_TIMEOUT_MS);
                                assert_response(
                                    task,
                                    completed,
                                    Err(SendError::Failed(SendRequestFailure::Timeout)),
                                );
                                assert_eq!(
                                    lab.calls.borrow().len(),
                                    boundary.calls_after_loss(),
                                    "seed {seed}, {boundary:?}, {fault:?}"
                                );
                                assert!(
                                    lab.advance(10).is_empty(),
                                    "late responses cannot resurrect a settled request"
                                );
                            }
                            Fault::Delayed => {
                                assert!(completed.is_empty());
                                assert!(lab.advance(9).is_empty());
                                completed = lab.advance(1);
                                assert_response(
                                    task,
                                    completed,
                                    Ok(RemoteControlResponse::AppMessage(payload())),
                                );
                            }
                            Fault::Duplicated => {
                                assert_response(
                                    task,
                                    completed,
                                    Ok(RemoteControlResponse::AppMessage(payload())),
                                );
                                assert!(
                                    lab.advance(10).is_empty(),
                                    "a duplicate cannot complete the request twice"
                                );
                            }
                        }
                        assert!(
                            lab.calls.borrow().len() <= 1,
                            "transport duplication must not repeat application execution"
                        );
                        let recovery = RemoteControlAppMessage::from_slice(b"fresh-after-fault")
                            .expect("bounded");
                        assert_eq!(
                            lab.exchange(
                                CONTROLLER,
                                link,
                                RemoteControlRequest::AppMessage(recovery.clone())
                            ),
                            RemoteControlResponse::AppMessage(recovery)
                        );
                        let controller = lab.nodes[CONTROLLER].identity.identity_hash();
                        let mut expected = match (fault, boundary) {
                            (Fault::Lost, Boundary::Request) => vec![],
                            _ => vec![AppInvocation {
                                controller,
                                payload: payload().as_slice().to_vec(),
                            }],
                        };
                        expected.push(AppInvocation {
                            controller,
                            payload: b"fresh-after-fault".to_vec(),
                        });
                        let calls = lab.calls.borrow().clone();
                        assert_eq!(calls, expected, "seed {seed}, {boundary:?}, {fault:?}");
                        calls
                    })
                };
                let first = run();
                assert_eq!(
                    first,
                    run(),
                    "seed {seed}, {boundary:?}, {fault:?} must replay complete packet traces"
                );
            }
        }
    }
}

fn assert_response(
    task: ManualTaskId,
    mut completed: Vec<(ManualTaskId, Event)>,
    expected: Result<RemoteControlResponse, SendError<SendRequestFailure>>,
) {
    assert_eq!(completed.len(), 1);
    let (found, Event::Response(result)) = completed.remove(0) else {
        unreachable!("request completion");
    };
    assert_eq!(found, task);
    assert_eq!(
        result.map(|bytes| RemoteControlResponse::parse(&bytes).expect("canonical response")),
        expected
    );
}

#[test]
fn cancellation_drops_the_local_waiter_without_misattributing_an_inflight_response() {
    let (_, baseline) = with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(CONTROLLER);
        lab.exchange(
            CONTROLLER,
            link,
            RemoteControlRequest::AppMessage(payload()),
        );
    });
    let ordinal = baseline
        .iter()
        .find_map(|event| match event {
            MediumEvent::TransmissionAccepted { ordinal, frame, .. }
                if WirePacketHeader::parse(frame)
                    .is_ok_and(|(header, _)| header.context == WireContext::Request) =>
            {
                Some(*ordinal)
            }
            _ => None,
        })
        .expect("request boundary");
    with_lab(
        ManualTaskScheduling::Cyclic,
        FaultPlan::new(vec![TransmissionRule::delay(
            ordinal,
            SimulationDurationInTicks::from_ticks(10),
        )])
        .expect("delayed request"),
        |lab| {
            let link = lab.link(CONTROLLER);
            let cancelled = lab.request(
                CONTROLLER,
                link,
                encoded(RemoteControlRequest::AppMessage(payload())),
            );
            assert!(lab.settle().is_empty());
            assert_eq!(
                lab.runner
                    .cancel(cancelled)
                    .expect("local waiter cancellation"),
                ManualTaskCancellation::Cancelled
            );
            let fresh =
                RemoteControlAppMessage::from_slice(b"different-live-request").expect("bounded");
            assert_eq!(
                lab.exchange(
                    CONTROLLER,
                    link,
                    RemoteControlRequest::AppMessage(fresh.clone())
                ),
                RemoteControlResponse::AppMessage(fresh)
            );
            assert!(
                lab.advance(REQUEST_TIMEOUT_MS).is_empty(),
                "cancelled waiter never returns a completion"
            );
            assert_eq!(
                *lab.calls.borrow(),
                [
                    AppInvocation {
                        controller: lab.nodes[CONTROLLER].identity.identity_hash(),
                        payload: b"different-live-request".to_vec()
                    },
                    AppInvocation {
                        controller: lab.nodes[CONTROLLER].identity.identity_hash(),
                        payload: payload().as_slice().to_vec()
                    },
                ],
                "cancelling a local wait does not withdraw a request already on the wire"
            );
            assert_eq!(lab.runner.task_count(), lab.nodes.len());
        },
    );
}

#[test]
fn generated_watch_frame_loss_delay_and_duplication_keep_event_sequences_aligned() {
    use personal_rns::runtime::StreamId;
    for seed in SEEDS {
        let scheduling = ManualTaskScheduling::Seeded {
            seed: SimulationSeed::new(seed),
        };
        let (_, baseline) = with_lab(scheduling, FaultPlan::none(), |lab| {
            let link = lab.link(CONTROLLER);
            let watch = lab.watch(link, StreamId::new(3).expect("stream ID"));
            let task = lab.read_watch(watch);
            let completed = lab.settle();
            let (_, initial) = watch_result(task, completed);
            assert_eq!(
                initial.expect("initial"),
                RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
            );
        });
        let ordinal = baseline
            .iter()
            .find_map(|event| match event {
                MediumEvent::TransmissionAccepted { ordinal, frame, .. }
                    if WirePacketHeader::parse(frame)
                        .is_ok_and(|(header, _)| header.context == WireContext::Channel) =>
                {
                    Some(*ordinal)
                }
                _ => None,
            })
            .expect("initial watch channel frame");
        for fault in [Fault::Lost, Fault::Delayed, Fault::Duplicated] {
            let plan = FaultPlan::new(vec![fault.rule(ordinal)]).expect("one watch fault");
            let run = || {
                eprintln!("remote_control_watch_case seed={seed} fault={fault:?}");
                with_lab(scheduling, plan.clone(), |lab| {
                    let link = lab.link(CONTROLLER);
                    let watch = lab.watch(link, StreamId::new(3).expect("stream ID"));
                    let task = lab.read_watch(watch);
                    let mut completed = lab.settle();
                    for _ in 0..500 {
                        if !completed.is_empty() {
                            break;
                        }
                        completed = lab.advance(1);
                    }
                    let (watch, initial) = watch_result(task, completed);
                    assert_eq!(
                        initial.expect("retry delivers one initial event"),
                        RemoteControlStreamEvent::ResyncRequired { sequence: 1 },
                        "seed {seed}, {fault:?}"
                    );
                    let task = lab.read_watch(watch);
                    assert!(lab.settle().is_empty());
                    assert!(
                        lab.advance(20).is_empty(),
                        "late duplicate frame must not become a second event"
                    );
                    let completed = lab.advance(5_500);
                    let (_, heartbeat) = watch_result(task, completed);
                    assert_eq!(
                        heartbeat.expect("heartbeat"),
                        RemoteControlStreamEvent::Heartbeat { sequence: 2 }
                    );
                })
            };
            let first = run();
            assert_eq!(
                first,
                run(),
                "watch fault must replay for seed {seed}, {fault:?}"
            );
        }
    }
}
