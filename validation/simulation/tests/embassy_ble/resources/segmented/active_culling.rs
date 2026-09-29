use super::*;
use crate::wire_gate::WireGate;
use personal_rns::wire::{WireContext, WirePacketHeader};

const CONTINUATION_PART_OCCURRENCE: usize = 3;
const RETRY_DRAIN_MS: u64 = 120_000;
const ACTIVE_TRACE_CAPACITY: usize = 524_288;

fn part_header(link: LinkId) -> WirePacketHeader {
    WirePacketHeader {
        context: WireContext::Resource,
        ..buffered_interruption::advertisement_header(link)
    }
}

async fn release_without_pressure<T>(
    gate: WireGate,
    link: LinkId,
    request: impl Future<Output = T>,
) -> T {
    let header = part_header(link);
    gate.arm(
        header,
        NonZeroUsize::new(CONTINUATION_PART_OCCURRENCE).unwrap(),
    );
    let (result, ()) = tokio::join!(request, async {
        assert_eq!(gate.held().await, header);
        gate.release();
    });
    assert!(gate.is_idle());
    result
}

fn no_pressure_control(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    embedded: &ResourceNode,
    desktop: &tokio_node::TokioNode,
    links: [LinkId; 2],
) {
    let handle = desktop.handle.clone();
    let gate = embedded.wire.clone();
    let (result, elapsed) = complete(tasks, async move {
        measured(release_without_pressure(
            gate,
            links[1],
            handle.request(
                links[1],
                RequestPathHash::of(files::FILE_PATH),
                &(TRANSFER_BYTES as u16).to_be_bytes(),
            ),
        ))
        .await
    });
    assert_eq!(
        result,
        Ok((files::FILE_BYTES[..TRANSFER_BYTES].to_vec(), elapsed))
    );
    assert_eq!(
        response_settlements(embedded),
        [Settlement::Respond(Ok(()))]
    );

    let handle = embedded.handle;
    let gate = desktop.wire.clone();
    let (result, elapsed) = complete(tasks, async move {
        measured(release_without_pressure(
            gate,
            links[0],
            handle.request(
                links[0],
                RequestPathHash::of(files::FILE_PATH),
                &(TRANSFER_BYTES as u16).to_be_bytes(),
            ),
        ))
        .await
    });
    assert_eq!(
        result.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
        Ok((files::FILE_BYTES[..TRANSFER_BYTES].to_vec(), elapsed))
    );
    assert!(embedded.take_settled().is_empty());
    assert_eq!(lab.active_connection_count(), 1);
    reassemble(tasks, lab, embedded, desktop, links);
}

enum RequestKind {
    RawCalibration,
    Buffered,
}

enum RetiredRequest {
    Raw(CommandId),
    Buffered,
}

async fn raw_request(
    handle: impl PrnsNodeApi,
    trace: ResponseTrace,
    link: LinkId,
) -> RetiredRequest {
    let command = handle
        .issue(PrnsCommand::SendRequest(SendRequest {
            link_id: link,
            path_hash: RequestPathHash::of(files::FILE_PATH),
            data: SendRequestData::from_slice(&(TRANSFER_BYTES as u16).to_be_bytes()).unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Unlimited,
        }))
        .unwrap();
    let events = trace.completed().await;
    let [ResponseEvent::Segment { request, bytes, .. }, _] = events.as_slice() else {
        unreachable!("one provisional segment precedes the cull: {events:?}");
    };
    assert!(!bytes.is_empty() && bytes.len() <= TRANSFER_WINDOW_BYTES);
    assert_eq!(
        events,
        [
            ResponseEvent::Segment {
                link,
                request: *request,
                index: 1,
                total: 3,
                bytes: files::FILE_BYTES[..bytes.len()].to_vec()
            },
            ResponseEvent::Settled {
                command,
                result: Err(SendRequestFailure::Culled)
            },
        ]
    );
    RetiredRequest::Raw(command)
}

async fn displace_active(
    gate: WireGate,
    header: WirePacketHeader,
    old: impl Future<Output = RetiredRequest>,
    replacement: impl Future<Output = ()>,
) -> RetiredRequest {
    let (culled, retired) = tokio::sync::oneshot::channel();
    let (request, ()) = tokio::join!(
        async {
            let request = old.await;
            culled.send(()).unwrap();
            request
        },
        async {
            assert_eq!(gate.held().await, header);
            tokio::join!(replacement, async {
                retired.await.unwrap();
                gate.release();
            });
        },
    );
    assert!(gate.is_idle());
    // Let the real sender's retry policy settle the abandoned Resource while
    // keepalives continue; no node, link or receiver state is reset.
    tokio::time::sleep(Duration::from_millis(RETRY_DRAIN_MS)).await;
    request
}

fn exercise(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    embedded: &ResourceNode,
    desktop: &tokio_node::TokioNode,
    links: [LinkId; 2],
    requester: Requester,
    kind: RequestKind,
) {
    let spare = establish_pair(tasks, embedded, &desktop.handle);
    let (link, gate) = match requester {
        Requester::Tokio => (links[1], embedded.wire.clone()),
        Requester::Embassy => (links[0], desktop.wire.clone()),
    };
    let header = part_header(link);
    gate.arm(
        header,
        NonZeroUsize::new(CONTINUATION_PART_OCCURRENCE).unwrap(),
    );
    let budget = CompletionBudget {
        deadline: tick(tasks.snapshot().tick.get() + RETRY_DRAIN_MS + TRANSFER_BUDGET_MS),
        polls_per_tick: NonZeroUsize::new(TRANSFER_POLLS_PER_TICK).unwrap(),
    };
    match requester {
        Requester::Tokio => {
            let old_handle = desktop.handle.clone();
            let replacement = desktop.handle.clone();
            let trace = desktop.responses.clone();
            tasks.complete_with_budget(
                budget,
                displace_active(
                    gate,
                    header,
                    async move {
                        match kind {
                            RequestKind::RawCalibration => {
                                raw_request(old_handle, trace, link).await
                            }
                            RequestKind::Buffered => {
                                assert_eq!(
                                    old_handle
                                        .request(
                                            link,
                                            RequestPathHash::of(files::FILE_PATH),
                                            &(TRANSFER_BYTES as u16).to_be_bytes()
                                        )
                                        .await,
                                    Err(SendError::Failed(SendRequestFailure::Culled))
                                );
                                RetiredRequest::Buffered
                            }
                        }
                    },
                    async move {
                        let (result, elapsed) = measured(replacement.request(
                            link,
                            RequestPathHash::of(crate::echo::QUERY_PATH),
                            b"surviving request",
                        ))
                        .await;
                        assert_eq!(result, Ok((b"surviving request".to_vec(), elapsed)));
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
            let trace = embedded.responses.clone();
            let retired = tasks.complete_with_budget(
                budget,
                displace_active(
                    gate,
                    header,
                    async move {
                        match kind {
                            RequestKind::RawCalibration => raw_request(handle, trace, link).await,
                            RequestKind::Buffered => {
                                assert_eq!(
                                    handle
                                        .request(
                                            link,
                                            RequestPathHash::of(files::FILE_PATH),
                                            &(TRANSFER_BYTES as u16).to_be_bytes()
                                        )
                                        .await,
                                    Err(SendError::Failed(SendRequestFailure::Culled))
                                );
                                RetiredRequest::Buffered
                            }
                        }
                    },
                    async move {
                        let (result, elapsed) = measured(handle.request(
                            link,
                            RequestPathHash::of(crate::echo::QUERY_PATH),
                            b"surviving request",
                        ))
                        .await;
                        assert_eq!(
                            result.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
                            Ok((b"surviving request".to_vec(), elapsed))
                        );
                    },
                ),
            );
            match retired {
                RetiredRequest::Raw(command) => assert_eq!(
                    embedded.take_settled(),
                    [(
                        command,
                        Settlement::SendRequest(Err(SendRequestFailure::Culled))
                    )]
                ),
                RetiredRequest::Buffered => assert!(embedded.take_settled().is_empty()),
            }
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

fn run(embedded: Endpoint, desktop: Endpoint, kind: RequestKind, requester: Requester) {
    scenario_with_receipts::<1>(
        embedded,
        desktop,
        ACTIVE_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, requester, kind)
        },
    );
}

#[test]
fn apple_raw_calibrates_culling_with_a_continuation_part_in_flight() {
    run(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        RequestKind::RawCalibration,
        Requester::Tokio,
    );
}
#[test]
fn esp32_raw_calibrates_culling_with_a_continuation_part_in_flight() {
    run(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        RequestKind::RawCalibration,
        Requester::Embassy,
    );
}
#[test]
fn bluez_raw_calibrates_culling_with_a_continuation_part_in_flight() {
    run(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        RequestKind::RawCalibration,
        Requester::Tokio,
    );
}
#[test]
fn nrf52_raw_calibrates_culling_with_a_continuation_part_in_flight() {
    run(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        RequestKind::RawCalibration,
        Requester::Embassy,
    );
}
#[test]
fn apple_buffered_culling_discards_partial_bytes_and_survives_late_parts() {
    run(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        RequestKind::Buffered,
        Requester::Tokio,
    );
}
#[test]
fn esp32_buffered_culling_discards_partial_bytes_and_survives_late_parts() {
    run(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        RequestKind::Buffered,
        Requester::Embassy,
    );
}
#[test]
fn bluez_buffered_culling_discards_partial_bytes_and_survives_late_parts() {
    run(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        RequestKind::Buffered,
        Requester::Tokio,
    );
}
#[test]
fn nrf52_buffered_culling_discards_partial_bytes_and_survives_late_parts() {
    run(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        RequestKind::Buffered,
        Requester::Embassy,
    );
}

#[test]
fn esp32_and_apple_resume_held_parts_normally_without_pressure() {
    scenario_with_receipts::<1>(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        TRACE_CAPACITY,
        no_pressure_control,
    );
}

#[test]
fn nrf52_and_bluez_resume_held_parts_normally_without_pressure() {
    scenario_with_receipts::<1>(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        TRACE_CAPACITY,
        no_pressure_control,
    );
}
