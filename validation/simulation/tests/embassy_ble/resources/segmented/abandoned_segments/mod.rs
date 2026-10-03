use super::*;
use crate::request_probe::RequestProbe;
use crate::respond_probe::{RespondProbe, Responded};
use crate::wire_gate::WireGate;
use personal_rns::routing::links::request::RequestId;

mod calibration;
use calibration::calibrate;

const RETAINED_RECEIPTS: usize = 3;
const LOST_ADVERTISEMENT_BUDGET: usize = 16;
const PRIVATE_COMPLETION_WINDOW: Duration = Duration::from_millis(INTERRUPTION_BUDGET_MS);
const SURVIVOR_BYTES: [&[u8]; 2] = [b"first replacement waiter", b"second replacement waiter"];

#[derive(Clone, Copy)]
enum DropAfter {
    FirstSegment,
    SecondSegment,
}

impl DropAfter {
    fn count(self) -> usize {
        match self {
            Self::FirstSegment => 1,
            Self::SecondSegment => 2,
        }
    }

    fn block_continuation(self, gate: &WireGate, link: LinkId) {
        gate.lose_after(
            buffered_interruption::advertisement_header(link),
            self.count(),
            NonZeroUsize::new(LOST_ADVERTISEMENT_BUDGET).unwrap(),
        );
    }
}

#[derive(Clone, Copy)]
enum Finish {
    Resume,
    Timeout,
}

impl Finish {
    fn responder_result(self) -> Result<(), RespondFailure> {
        match self {
            Self::Resume => Ok(()),
            Self::Timeout => Err(RespondFailure::Resource(SendResourceFailure::Timeout)),
        }
    }
}

struct Abandoned {
    request: RequestId,
    started: tokio::time::Instant,
}

async fn abandon<T>(
    probe: &RequestProbe,
    gate: &WireGate,
    trace: &ResponseTrace,
    link: LinkId,
    after: DropAfter,
    request: impl Future<Output = T>,
) -> Abandoned {
    probe.arm(link, RequestPathHash::of(files::FILE_PATH));
    after.block_continuation(gate, link);
    let started = tokio::time::Instant::now();
    tokio::select! {
        biased;
        _ = request => unreachable!("a blocked partial response cannot finish"),
        header = gate.first_loss() => {
            assert_eq!(header, buffered_interruption::advertisement_header(link));
        },
    }
    // select owns the request future, so the completion-slot guard has already
    // dropped here. The protocol receipt and incomplete assembly remain live.
    assert!(
        trace.is_empty(),
        "the buffered prefix must not reach the application"
    );
    let (observed_link, request) = probe.take().await;
    assert_eq!(observed_link, link);
    Abandoned { request, started }
}

async fn finish_sender(gate: &WireGate, probe: &RespondProbe, finish: Finish) -> Responded {
    probe.arm();
    if let Finish::Resume = finish {
        assert!(gate.stop_loss() > 0);
    }
    let observed = probe.take().await;
    assert_eq!(observed.result, finish.responder_result());
    if let Finish::Timeout = finish {
        assert!(gate.stop_loss() > 0);
    }
    assert!(gate.is_idle());
    observed
}

fn finish_with_budget<T: 'static>(
    tasks: &mut EmbassyTasks<'_>,
    future: impl Future<Output = T> + 'static,
) -> T {
    tasks.complete_with_budget(
        CompletionBudget {
            deadline: tick(tasks.snapshot().tick.get() + INTERRUPTION_BUDGET_MS),
            polls_per_tick: NonZeroUsize::new(TRANSFER_POLLS_PER_TICK).unwrap(),
        },
        future,
    )
}

fn exercise_tokio(
    tasks: &mut EmbassyTasks<'_>,
    embedded: &ResourceNode,
    desktop: &tokio_node::TokioNode,
    link: LinkId,
    after: DropAfter,
    finish: Finish,
) {
    let handle = desktop.handle.clone();
    let trace = desktop.responses.clone();
    let gate = embedded.wire.clone();
    let responder = embedded.responded.clone();
    let (_, calibration) = complete(tasks, async move {
        calibrate(&handle, &trace, &gate, &responder, link, after).await
    });
    assert_eq!(embedded.take_settled(), [calibration]);

    let handle = desktop.handle.clone();
    let probe = embedded.requests.clone();
    let gate = embedded.wire.clone();
    let trace = desktop.responses.clone();
    let old = complete(tasks, async move {
        let data = (TRANSFER_BYTES as u16).to_be_bytes();
        abandon(
            &probe,
            &gate,
            &trace,
            link,
            after,
            handle.request(link, RequestPathHash::of(files::FILE_PATH), &data),
        )
        .await
    });
    assert!(embedded.take_settled().is_empty());

    let handle = desktop.handle.clone();
    let replies = complete(tasks, async move {
        let path = RequestPathHash::of(crate::echo::QUERY_PATH);
        tokio::join!(
            measured(handle.request(link, path, SURVIVOR_BYTES[0])),
            measured(handle.request(link, path, SURVIVOR_BYTES[1])),
        )
    });
    for ((result, elapsed), bytes) in [replies.0, replies.1].into_iter().zip(SURVIVOR_BYTES) {
        assert_eq!(result, Ok((bytes.to_vec(), elapsed)));
    }
    assert_eq!(
        response_settlements(embedded),
        [const { Settlement::Respond(Ok(())) }; 2]
    );
    assert!(desktop.responses.is_empty());

    let gate = embedded.wire.clone();
    let responder = embedded.responded.clone();
    let observed = finish_with_budget(tasks, async move {
        let observed = finish_sender(&gate, &responder, finish).await;
        if let Finish::Timeout = finish {
            // Tokio consumes the orphan's terminal journal privately. Sender
            // timeout alone does not prove the receiver has expired its own
            // between-segment deadline; test reclamation within a fixed window.
            tokio::time::sleep_until(old.started + PRIVATE_COMPLETION_WINDOW).await;
        }
        observed
    });
    assert_eq!(
        embedded.take_settled(),
        [(observed.command, Settlement::Respond(observed.result))]
    );
    assert!(
        desktop.responses.is_empty(),
        "Tokio keeps abandoned buffered completions private"
    );
}

fn exercise_embassy(
    tasks: &mut EmbassyTasks<'_>,
    embedded: &ResourceNode,
    desktop: &tokio_node::TokioNode,
    link: LinkId,
    after: DropAfter,
    finish: Finish,
) {
    let handle = embedded.handle;
    let trace = embedded.responses.clone();
    let gate = desktop.wire.clone();
    let responder = desktop.responded.clone();
    let (calibrated, _) = complete(tasks, async move {
        calibrate(&handle, &trace, &gate, &responder, link, after).await
    });
    assert_eq!(
        embedded.take_settled().as_slice(),
        std::slice::from_ref(&calibrated.settlement)
    );

    let probe = desktop.requests.clone();
    let gate = desktop.wire.clone();
    let trace = embedded.responses.clone();
    let old = complete(tasks, async move {
        let data = (TRANSFER_BYTES as u16).to_be_bytes();
        abandon(
            &probe,
            &gate,
            &trace,
            link,
            after,
            handle.request(link, RequestPathHash::of(files::FILE_PATH), &data),
        )
        .await
    });
    assert!(embedded.take_settled().is_empty());

    let replies = complete(tasks, async move {
        let path = RequestPathHash::of(crate::echo::QUERY_PATH);
        tokio::join!(
            measured(handle.request(link, path, SURVIVOR_BYTES[0])),
            measured(handle.request(link, path, SURVIVOR_BYTES[1])),
        )
    });
    for ((result, elapsed), bytes) in [replies.0, replies.1].into_iter().zip(SURVIVOR_BYTES) {
        assert_eq!(
            result.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
            Ok((bytes.to_vec(), elapsed))
        );
    }
    assert!(embedded.take_settled().is_empty());
    assert!(embedded.responses.is_empty());

    let gate = desktop.wire.clone();
    let responder = desktop.responded.clone();
    let trace = embedded.responses.clone();
    let ((events, elapsed), _) = finish_with_budget(tasks, async move {
        tokio::join!(
            async {
                let events = trace.completed().await;
                let elapsed = RttMillis::new(old.started.elapsed().as_millis().try_into().unwrap());
                (events, elapsed)
            },
            finish_sender(&gate, &responder, finish),
        )
    });
    let Some(ResponseEvent::Settled { command, .. }) = events.last() else {
        unreachable!("the abandoned request must have a terminal journal")
    };
    let command = *command;
    let result = match finish {
        Finish::Resume => Ok(PacketReceiptDelivered {
            rtt: elapsed,
            evidence: DeliveryEvidence::Response,
        }),
        Finish::Timeout => Err(SendRequestFailure::Timeout),
    };
    let mut expected = match finish {
        Finish::Resume => calibrated.suffix(link, old.request, after),
        Finish::Timeout => Vec::new(),
    };
    expected.push(ResponseEvent::Settled { command, result });
    assert_eq!(
        events, expected,
        "only the unconsumed suffix may reach the raw application"
    );
    assert_eq!(
        embedded.take_settled(),
        [(command, Settlement::SendRequest(result))]
    );
    assert!(embedded.responses.is_empty());
}

fn exercise(requester: Requester, finish: Finish) {
    for after in [DropAfter::FirstSegment, DropAfter::SecondSegment] {
        for (embedded_endpoint, desktop_endpoint) in [
            (
                Endpoint::Esp32(Esp32Host::Esp32),
                Endpoint::CoreBluetooth(AppleHost::MacOs),
            ),
            (
                Endpoint::Nrf52(Nrf52Host::Nrf52),
                Endpoint::BlueZ(BlueZHost::Linux),
            ),
        ] {
            scenario_with_receipts::<RETAINED_RECEIPTS>(
                embedded_endpoint,
                desktop_endpoint,
                INTERRUPTION_TRACE_CAPACITY,
                |tasks, lab, embedded, desktop, links| {
                    let spare = establish_pair(tasks, embedded, &desktop.handle);
                    match requester {
                        Requester::Tokio => {
                            exercise_tokio(tasks, embedded, desktop, links[1], after, finish)
                        }
                        Requester::Embassy => {
                            exercise_embassy(tasks, embedded, desktop, links[0], after, finish)
                        }
                    }
                    assert_eq!(lab.active_connection_count(), 1);
                    assert!(embedded.take_closed().is_empty());
                    assert!(desktop.take_closed().is_empty());
                    // A distinct link must acquire the one assembly/transfer slot.
                    // Reusing only the old link could mask a retained keyed row.
                    reassemble(tasks, lab, embedded, desktop, spare);
                    reuse_both_embassy_request_slots(tasks, embedded, links[0]);
                    reassemble(tasks, lab, embedded, desktop, links);
                },
            );
        }
    }
}

#[test]
fn tokio_abandoned_segments_finish_privately_and_release_the_assembly() {
    exercise(Requester::Tokio, Finish::Resume);
}

#[test]
fn embassy_abandoned_segments_deliver_only_the_remaining_suffix() {
    exercise(Requester::Embassy, Finish::Resume);
}

#[test]
fn tokio_abandoned_segments_time_out_and_release_the_assembly() {
    exercise(Requester::Tokio, Finish::Timeout);
}

#[test]
fn embassy_abandoned_segments_time_out_without_redelivering_the_prefix() {
    exercise(Requester::Embassy, Finish::Timeout);
}
