use super::*;
use crate::wire_gate::WireGate;
use personal_rns::engine::{Respond, RespondData, RespondPayload, SendResourceRejection};

const WHOLE_RESPONSE_BYTES: usize = 128;
const FIRST_SEGMENT_WIRE_PARTS: usize = 2;

enum Competition {
    PacketBetweenSegments,
    PacketDuringContinuation,
    WholeResourceBetweenSegments,
}

fn compete(
    tasks: &mut EmbassyTasks<'_>,
    requester: &impl PrnsNodeApi,
    responder: &impl PrnsNodeApi,
    trace: ResponseTrace,
    gate: &WireGate,
    links: [LinkId; 2],
    competition: &Competition,
) -> [(CommandId, Settlement); 2] {
    let [link, other_link] = links;
    let advertisement = buffered_interruption::advertisement_header(link);
    let (loss_header, passes) = match competition {
        Competition::PacketBetweenSegments | Competition::WholeResourceBetweenSegments => {
            (advertisement, 1)
        }
        Competition::PacketDuringContinuation => (
            personal_rns::wire::WirePacketHeader {
                context: personal_rns::wire::WireContext::Resource,
                ..advertisement
            },
            FIRST_SEGMENT_WIRE_PARTS,
        ),
    };
    gate.lose_after(loss_header, passes, NonZeroUsize::new(16).unwrap());
    let started = tasks.snapshot().tick;
    let command = requester
        .issue(PrnsCommand::SendRequest(SendRequest {
            link_id: link,
            path_hash: RequestPathHash::of(files::FILE_PATH),
            data: SendRequestData::from_slice(&(TRANSFER_BYTES as u16).to_be_bytes()).unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Maximum(TRANSFER_BYTES as u64),
        }))
        .unwrap();
    let observer = trace.clone();
    let first = complete(tasks, async move { observer.next().await });
    let ResponseEvent::Segment {
        request,
        index: 1,
        total: 3,
        ..
    } = &first
    else {
        unreachable!("the split response must own the request before competing")
    };
    let request = *request;
    let observer = gate.clone();
    assert_eq!(
        complete(tasks, async move { observer.first_loss().await }),
        loss_header
    );
    assert!(trace.is_empty());
    assert!(responder
        .issue(PrnsCommand::Respond(Respond {
            link_id: link,
            request_id: request,
            payload: match competition {
                Competition::PacketBetweenSegments | Competition::PacketDuringContinuation =>
                    RespondPayload::Packed(RespondData::from_slice(b"competing packet").unwrap()),
                Competition::WholeResourceBetweenSegments => RespondPayload::StaticFile {
                    name: "competitor.bin",
                    bytes: &[0xE7; WHOLE_RESPONSE_BYTES],
                },
            },
        }))
        .is_some());
    tasks.settle();
    assert!(
        trace.is_empty(),
        "a competing response must not publish or settle a split request"
    );
    let other_started = tasks.snapshot().tick;
    let other_command = requester
        .issue(PrnsCommand::SendRequest(SendRequest {
            link_id: other_link,
            path_hash: RequestPathHash::of(files::FILE_PATH),
            data: SendRequestData::from_slice(&(WHOLE_RESPONSE_BYTES as u16).to_be_bytes())
                .unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Maximum(WHOLE_RESPONSE_BYTES as u64),
        }))
        .unwrap();
    let observer = trace.clone();
    let other_events = complete(tasks, async move { observer.completed().await });
    let other_result = Ok(PacketReceiptDelivered {
        rtt: RttMillis::new(tasks.snapshot().tick.get() - other_started.get()),
        evidence: DeliveryEvidence::Response,
    });
    let Some(ResponseEvent::Whole {
        request: other_request,
        ..
    }) = other_events.first()
    else {
        unreachable!("the other link must deliver its independent whole response: {other_events:?}")
    };
    assert_ne!(*other_request, request);
    assert_eq!(
        other_events,
        [
            ResponseEvent::Whole {
                link: other_link,
                request: *other_request,
                bytes: files::FILE_BYTES[..WHOLE_RESPONSE_BYTES].to_vec(),
            },
            ResponseEvent::Settled {
                command: other_command,
                result: other_result
            },
        ]
    );
    assert!(gate.stop_loss() > 0);
    let mut events = vec![first];
    events.extend(complete(tasks, async move { trace.completed().await }));
    let result = Ok(PacketReceiptDelivered {
        rtt: RttMillis::new(tasks.snapshot().tick.get() - started.get()),
        evidence: DeliveryEvidence::Response,
    });
    assert_eq!(events.len(), 4);
    let mut expected = Vec::new();
    let mut offset = 0;
    for (position, event) in events[..3].iter().enumerate() {
        let ResponseEvent::Segment { bytes, .. } = event else {
            unreachable!("the original response must retain its delivery lane")
        };
        let end = offset + bytes.len();
        expected.push(ResponseEvent::Segment {
            link,
            request,
            index: position as u64 + 1,
            total: 3,
            bytes: files::FILE_BYTES[offset..end].to_vec(),
        });
        offset = end;
    }
    assert_eq!(offset, TRANSFER_BYTES);
    expected.push(ResponseEvent::Settled { command, result });
    assert_eq!(events, expected);
    [
        (other_command, Settlement::SendRequest(other_result)),
        (command, Settlement::SendRequest(result)),
    ]
}

fn exercise(competition: Competition) {
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
        scenario_with_capacity::<ORDINARY_RECEIPTS, 2>(
            embedded_endpoint,
            desktop_endpoint,
            TRACE_CAPACITY,
            |tasks, lab, embedded, desktop, links| {
                let spare = establish_pair(tasks, embedded, &desktop.handle);
                compete(
                    tasks,
                    &desktop.handle,
                    &embedded.handle,
                    desktop.responses.clone(),
                    &embedded.wire,
                    [links[1], spare[1]],
                    &competition,
                );
                assert_eq!(
                    response_settlements(embedded),
                    [
                        match competition {
                            Competition::PacketBetweenSegments
                            | Competition::PacketDuringContinuation => Settlement::Respond(Ok(())),
                            Competition::WholeResourceBetweenSegments =>
                                Settlement::Respond(Err(RespondFailure::Resource(
                                    SendResourceFailure::Rejected(SendResourceRejection::LinkBusy)
                                ))),
                        },
                        Settlement::Respond(Ok(())),
                        Settlement::Respond(Ok(()))
                    ]
                );
                let expected = compete(
                    tasks,
                    &embedded.handle,
                    &desktop.handle,
                    embedded.responses.clone(),
                    &desktop.wire,
                    [links[0], spare[0]],
                    &competition,
                );
                assert_eq!(embedded.take_settled(), expected);
                reassemble(tasks, lab, embedded, desktop, links);
            },
        );
    }
}

#[test]
fn packet_responses_cannot_replace_an_active_split_response() {
    exercise(Competition::PacketBetweenSegments);
}

#[test]
fn packet_responses_cannot_replace_a_split_response_during_continuation_reception() {
    exercise(Competition::PacketDuringContinuation);
}

#[test]
fn a_busy_split_sender_refuses_competing_whole_resources_but_serves_other_links() {
    exercise(Competition::WholeResourceBetweenSegments);
}
