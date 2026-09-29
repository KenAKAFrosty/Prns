use super::*;
use crate::wire_gate::WireGate;

fn expire_while_send_is_blocked(
    tasks: &mut EmbassyTasks<'_>,
    requester: &impl PrnsNodeApi,
    trace: ResponseTrace,
    gate: &WireGate,
    link: LinkId,
) -> CommandId {
    let header = buffered_interruption::advertisement_header(link);
    gate.arm(header, NonZeroUsize::new(2).unwrap());
    let command = requester
        .issue(PrnsCommand::SendRequest(SendRequest {
            link_id: link,
            path_hash: RequestPathHash::of(files::FILE_PATH),
            data: SendRequestData::from_slice(&(TRANSFER_BYTES as u16).to_be_bytes()).unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Maximum(TRANSFER_BYTES as u64),
        }))
        .unwrap();
    let held = gate.clone();
    assert_eq!(complete(tasks, async move { held.held().await }), header);
    let observer = trace.clone();
    let events = tasks.complete_with_budget(
        CompletionBudget {
            deadline: tick(tasks.snapshot().tick.get() + INTERRUPTION_BUDGET_MS),
            polls_per_tick: NonZeroUsize::new(TRANSFER_POLLS_PER_TICK).unwrap(),
        },
        async move { observer.completed().await },
    );
    let Some(ResponseEvent::Segment { request, bytes, .. }) = events.first() else {
        unreachable!("a verified first segment must precede timeout: {events:?}");
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
                bytes: files::FILE_BYTES[..bytes.len()].to_vec(),
            },
            ResponseEvent::Settled { command, result },
        ]
    );
    tasks.settle();
    assert!(
        trace.is_empty(),
        "timeout must leave no duplicate response or settlement"
    );
    command
}

#[test]
fn blocked_continuation_sends_close_stale_links_and_refresh_request_wakes() {
    exercise(Requester::Tokio);
}

#[test]
fn held_tokio_continuation_arrives_after_embassy_timeout_without_reviving_it() {
    exercise(Requester::Embassy);
}

fn exercise(requester: Requester) {
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
            INTERRUPTION_TRACE_CAPACITY,
            |tasks, lab, embedded, desktop, links| {
                match requester {
                    Requester::Tokio => {
                        expire_while_send_is_blocked(
                            tasks,
                            &desktop.handle,
                            desktop.responses.clone(),
                            &embedded.wire,
                            links[1],
                        );
                        assert!(embedded.wire.is_idle());
                        assert_eq!(
                            response_settlements(embedded),
                            [Settlement::Respond(Err(RespondFailure::Resource(
                                SendResourceFailure::Timeout
                            )))]
                        );
                        let handle = embedded.handle;
                        let result = complete(tasks, async move {
                            handle
                                .request(
                                    links[0],
                                    RequestPathHash::of(crate::echo::QUERY_PATH),
                                    b"pending during stale-link close",
                                )
                                .await
                        });
                        assert_eq!(
                            result,
                            Err(SendError::Failed(SendRequestFailure::LinkClosed))
                        );
                    }
                    Requester::Embassy => {
                        let command = expire_while_send_is_blocked(
                            tasks,
                            &embedded.handle,
                            embedded.responses.clone(),
                            &desktop.wire,
                            links[0],
                        );
                        assert_eq!(
                            embedded.take_settled(),
                            [(
                                command,
                                Settlement::SendRequest(Err(SendRequestFailure::Timeout))
                            )]
                        );
                        let gate = desktop.wire.clone();
                        assert_eq!(
                            complete(tasks, async move { gate.held().await }),
                            buffered_interruption::advertisement_header(links[0])
                        );
                        desktop.wire.release();
                        tasks.settle();
                        assert!(desktop.wire.is_idle());
                        assert!(embedded.responses.is_empty());
                        reuse_both_embassy_request_slots(tasks, embedded, links[0]);
                        reassemble(tasks, lab, embedded, desktop, links);
                        return;
                    }
                }
                converge(tasks, lab, embedded, &desktop.handle);
                let mut expected: Vec<_> = links
                    .into_iter()
                    .map(|link| (link, LinkClosedReason::Timeout))
                    .collect();
                expected.sort_by_key(|(link, _)| *link.as_bytes());
                assert_eq!(embedded.take_closed(), expected);
                assert_eq!(desktop.take_closed(), expected);
                let fresh = establish_pair(tasks, embedded, &desktop.handle);
                for (old, new) in links.into_iter().zip(fresh) {
                    assert_ne!(old, new);
                }
                reassemble(tasks, lab, embedded, desktop, fresh);
            },
        );
    }
}
