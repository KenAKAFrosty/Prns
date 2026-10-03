use super::*;
use crate::wire_gate::WireGate;

const RETRY_DRAIN_MS: u64 = 20_000;
const LOSS_BUDGET: usize = 16;

async fn displace(
    handle: impl PrnsNodeApi,
    trace: ResponseTrace,
    gate: WireGate,
    link: LinkId,
    replacement: impl Future<Output = ()>,
) -> CommandId {
    let command = handle
        .issue(PrnsCommand::SendRequest(SendRequest {
            link_id: link,
            path_hash: RequestPathHash::of(files::FILE_PATH),
            data: SendRequestData::from_slice(&(TRANSFER_BYTES as u16).to_be_bytes()).unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Unlimited,
        }))
        .unwrap();
    let first = trace.next().await;
    let ResponseEvent::Segment { request, bytes, .. } = &first else {
        unreachable!("receipt pressure follows a verified first segment");
    };
    assert!(!bytes.is_empty() && bytes.len() <= TRANSFER_WINDOW_BYTES);
    assert_eq!(
        first,
        ResponseEvent::Segment {
            link,
            request: *request,
            index: 1,
            total: 3,
            bytes: files::FILE_BYTES[..bytes.len()].to_vec(),
        }
    );
    assert!(trace.is_empty());
    gate.lose_after(
        buffered_interruption::advertisement_header(link),
        0,
        NonZeroUsize::new(LOSS_BUDGET).unwrap(),
    );
    replacement.await;
    assert_eq!(
        trace.completed().await,
        [ResponseEvent::Settled {
            command,
            result: Err(SendRequestFailure::Culled),
        }]
    );
    // Keep retries from reaching the retired request; let the real sender's
    // watchdog release its outgoing slot before probing the receiver's slot.
    tokio::time::sleep(Duration::from_millis(RETRY_DRAIN_MS)).await;
    assert!(gate.stop_loss() > 0);
    assert!(trace.is_empty());
    command
}

fn exercise(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    embedded: &ResourceNode,
    desktop: &tokio_node::TokioNode,
    links: [LinkId; 2],
    requester: Requester,
) {
    let spare = establish_pair(tasks, embedded, &desktop.handle);
    let budget = CompletionBudget {
        deadline: tick(tasks.snapshot().tick.get() + RETRY_DRAIN_MS + TRANSFER_BUDGET_MS),
        polls_per_tick: NonZeroUsize::new(TRANSFER_POLLS_PER_TICK).unwrap(),
    };
    match requester {
        Requester::Tokio => {
            let handle = desktop.handle.clone();
            tasks.complete_with_budget(
                budget,
                displace(
                    desktop.handle.clone(),
                    desktop.responses.clone(),
                    embedded.wire.clone(),
                    links[1],
                    async move {
                        let (result, elapsed) = measured(handle.request(
                            links[1],
                            RequestPathHash::of(crate::echo::QUERY_PATH),
                            b"replacement",
                        ))
                        .await;
                        assert_eq!(result, Ok((b"replacement".to_vec(), elapsed)));
                    },
                ),
            );
            assert_eq!(
                response_settlements(embedded),
                [
                    Settlement::Respond(Ok(())),
                    Settlement::Respond(Err(RespondFailure::Resource(
                        SendResourceFailure::Timeout
                    )))
                ]
            );
            journaled_request(
                tasks,
                &desktop.handle,
                desktop.responses.clone(),
                spare[1],
                &AdmissionCase::ThreeSegments,
            );
            assert_eq!(
                response_settlements(embedded),
                [Settlement::Respond(Ok(()))]
            );
        }
        Requester::Embassy => {
            let handle = embedded.handle;
            let command = tasks.complete_with_budget(
                budget,
                displace(
                    embedded.handle,
                    embedded.responses.clone(),
                    desktop.wire.clone(),
                    links[0],
                    async move {
                        let (result, elapsed) = measured(handle.request(
                            links[0],
                            RequestPathHash::of(crate::echo::QUERY_PATH),
                            b"replacement",
                        ))
                        .await;
                        assert_eq!(
                            result.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
                            Ok((b"replacement".to_vec(), elapsed))
                        );
                    },
                ),
            );
            assert_eq!(
                embedded.take_settled(),
                [(
                    command,
                    Settlement::SendRequest(Err(SendRequestFailure::Culled))
                )]
            );
            let expected = journaled_request(
                tasks,
                &embedded.handle,
                embedded.responses.clone(),
                spare[0],
                &AdmissionCase::ThreeSegments,
            );
            assert_eq!(embedded.take_settled(), [expected]);
        }
    }
    assert_eq!(lab.active_connection_count(), 1);
    assert!(embedded.take_closed().is_empty());
    assert!(desktop.take_closed().is_empty());
    reassemble(tasks, lab, embedded, desktop, links);
}

#[test]
fn apple_reclaims_a_response_displaced_by_receipt_pressure() {
    scenario_with_receipts::<1>(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, Requester::Tokio)
        },
    );
}

#[test]
fn esp32_reclaims_a_response_displaced_by_receipt_pressure() {
    scenario_with_receipts::<1>(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, Requester::Embassy)
        },
    );
}

#[test]
fn bluez_reclaims_a_response_displaced_by_receipt_pressure() {
    scenario_with_receipts::<1>(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, Requester::Tokio)
        },
    );
}

#[test]
fn nrf52_reclaims_a_response_displaced_by_receipt_pressure() {
    scenario_with_receipts::<1>(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, Requester::Embassy)
        },
    );
}
