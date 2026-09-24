use super::*;
use crate::wire_gate::WireGate;
use personal_rns::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext,
    WirePacketHeader,
};

enum RequestKind {
    RawCalibration,
    Buffered,
}

pub(super) fn advertisement_header(link: LinkId) -> WirePacketHeader {
    WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: link.to_address(),
        context: WireContext::ResourceAdvertisement,
    }
}

fn cut_before_second_advertisement<T: 'static>(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    gate: &WireGate,
    link: LinkId,
    request: impl Future<Output = T> + 'static,
) -> T {
    let header = advertisement_header(link);
    gate.arm(header, NonZeroUsize::new(2).unwrap());
    let sender = gate.clone();
    let medium = lab.clone();
    let result = tasks.complete_with_budget(
        CompletionBudget {
            deadline: tick(tasks.snapshot().tick.get() + INTERRUPTION_BUDGET_MS),
            polls_per_tick: NonZeroUsize::new(TRANSFER_POLLS_PER_TICK).unwrap(),
        },
        async move {
            let (result, ()) = tokio::join!(request, async move {
                assert_eq!(sender.held().await, header);
                assert_eq!(medium.active_connection_count(), 1);
                assert_eq!(
                    medium.set_reachability(EMBASSY_RADIO, TOKIO_RADIO, Reachability::Isolated),
                    Ok(TopologyMutation::Applied)
                );
                assert_eq!(medium.active_connection_count(), 0);
                sender.release();
            });
            result
        },
    );
    assert_eq!(lab.active_connection_count(), 0);
    assert!(gate.is_idle());
    result
}

#[test]
fn releasing_the_gate_without_an_outage_preserves_buffered_response_bytes() {
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
        scenario(
            embedded_endpoint,
            desktop_endpoint,
            TRACE_CAPACITY,
            |tasks, lab, embedded, desktop, links| {
                let gate = embedded.wire.clone();
                let header = advertisement_header(links[1]);
                gate.arm(header, NonZeroUsize::new(2).unwrap());
                let handle = desktop.handle.clone();
                let ((result, elapsed), ()) = complete(tasks, async move {
                    let requested = (TRANSFER_BYTES as u16).to_be_bytes();
                    tokio::join!(
                        measured(handle.request(
                            links[1],
                            RequestPathHash::of(files::FILE_PATH),
                            &requested
                        )),
                        async move {
                            assert_eq!(gate.held().await, header);
                            gate.release();
                        }
                    )
                });
                assert_eq!(
                    result,
                    Ok((files::FILE_BYTES[..TRANSFER_BYTES].to_vec(), elapsed))
                );
                assert!(embedded.wire.is_idle());
                assert_eq!(lab.active_connection_count(), 1);
                assert_eq!(
                    response_settlements(embedded),
                    [Settlement::Respond(Ok(()))]
                );
                reassemble(tasks, lab, embedded, desktop, links);
            },
        );
    }
}

fn calibrate(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    sender: &WireGate,
    handle: &impl PrnsNodeApi,
    trace: ResponseTrace,
    link: LinkId,
) -> (CommandId, Settlement) {
    assert!(trace.is_empty());
    let command = handle
        .issue(PrnsCommand::SendRequest(SendRequest {
            link_id: link,
            path_hash: RequestPathHash::of(files::FILE_PATH),
            data: SendRequestData::from_slice(&(TRANSFER_BYTES as u16).to_be_bytes()).unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Unlimited,
        }))
        .unwrap();
    let events = cut_before_second_advertisement(tasks, lab, sender, link, async move {
        trace.completed().await
    });
    let Some(ResponseEvent::Segment { request, bytes, .. }) = events.first() else {
        unreachable!("a real verified segment must precede the wire-gated interruption")
    };
    assert!(!bytes.is_empty() && bytes.len() <= TRANSFER_WINDOW_BYTES);
    let result = Err(SendRequestFailure::Timeout);
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
            ResponseEvent::Settled { command, result }
        ]
    );
    (command, Settlement::SendRequest(result))
}

fn exercise(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    embedded: &ResourceNode,
    desktop: &tokio_node::TokioNode,
    mut links: [LinkId; 2],
    kind: RequestKind,
) {
    for requester in [Requester::Tokio, Requester::Embassy] {
        match (&requester, &kind) {
            (Requester::Tokio, RequestKind::RawCalibration) => {
                calibrate(
                    tasks,
                    lab,
                    &embedded.wire,
                    &desktop.handle,
                    desktop.responses.clone(),
                    links[1],
                );
            }
            (Requester::Embassy, RequestKind::RawCalibration) => {
                let expected = calibrate(
                    tasks,
                    lab,
                    &desktop.wire,
                    &embedded.handle,
                    embedded.responses.clone(),
                    links[0],
                );
                assert_eq!(embedded.take_settled(), [expected]);
            }
            (Requester::Tokio, RequestKind::Buffered) => {
                let handle = desktop.handle.clone();
                let result = cut_before_second_advertisement(
                    tasks,
                    lab,
                    &embedded.wire,
                    links[1],
                    async move {
                        handle
                            .request(
                                links[1],
                                RequestPathHash::of(files::FILE_PATH),
                                &(TRANSFER_BYTES as u16).to_be_bytes(),
                            )
                            .await
                    },
                );
                assert_eq!(result, Err(SendError::Failed(SendRequestFailure::Timeout)));
            }
            (Requester::Embassy, RequestKind::Buffered) => {
                let handle = embedded.handle;
                let result = cut_before_second_advertisement(
                    tasks,
                    lab,
                    &desktop.wire,
                    links[0],
                    async move {
                        handle
                            .request(
                                links[0],
                                RequestPathHash::of(files::FILE_PATH),
                                &(TRANSFER_BYTES as u16).to_be_bytes(),
                            )
                            .await
                    },
                );
                assert_eq!(result, Err(SendError::Failed(SendRequestFailure::Timeout)));
            }
        }
        match requester {
            Requester::Tokio => assert_eq!(
                response_settlements(embedded),
                [Settlement::Respond(Err(RespondFailure::Resource(
                    SendResourceFailure::Timeout
                )))]
            ),
            Requester::Embassy => assert!(response_settlements(embedded).is_empty()),
        }
        assert!(embedded.responses.is_empty());
        assert!(desktop.responses.is_empty());
        links = reconnect(tasks, lab, embedded, desktop, links);
        assert!(response_settlements(embedded).is_empty());
        reuse_both_embassy_request_slots(tasks, embedded, links[0]);
        reassemble(tasks, lab, embedded, desktop, links);
    }
}

#[test]
fn esp32_and_apple_wire_cut_has_one_verified_segment() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(
                tasks,
                lab,
                embedded,
                desktop,
                links,
                RequestKind::RawCalibration,
            )
        },
    );
}

#[test]
fn nrf52_and_bluez_wire_cut_has_one_verified_segment() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(
                tasks,
                lab,
                embedded,
                desktop,
                links,
                RequestKind::RawCalibration,
            )
        },
    );
}

#[test]
fn esp32_and_apple_buffered_requests_fail_without_partial_success_and_recover() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, RequestKind::Buffered)
        },
    );
}

#[test]
fn nrf52_and_bluez_buffered_requests_fail_without_partial_success_and_recover() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, RequestKind::Buffered)
        },
    );
}
