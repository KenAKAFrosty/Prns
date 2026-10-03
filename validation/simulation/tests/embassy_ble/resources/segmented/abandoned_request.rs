use super::competition::{compete, Competition, Phase, ResponseClaim, COMPETING_BYTES};
use super::*;
use crate::request_probe::RequestProbe;
use crate::wire_gate::WireGate;
use personal_rns::routing::links::request::RequestId;
use personal_rns::wire::{WireContext, WirePacketHeader};

const OLD_BYTES: &[u8] = b"caller will stop waiting";
const LIVE_BYTES: &[u8] = b"second completion slot is reusable";
const RETAINED_RECEIPTS: usize = 3;

struct AbandonedRequest {
    id: RequestId,
    started: tokio::time::Instant,
}

async fn abandon<T>(
    probe: &RequestProbe,
    gate: &WireGate,
    link: LinkId,
    request: impl Future<Output = T>,
) -> AbandonedRequest {
    let started = tokio::time::Instant::now();
    probe.arm(link, RequestPathHash::of(crate::echo::QUERY_PATH));
    let header = WirePacketHeader {
        context: WireContext::Response,
        ..buffered_interruption::advertisement_header(link)
    };
    gate.lose_after(header, 0, NonZeroUsize::MIN);
    tokio::select! {
        biased;
        _ = request => unreachable!("the dropped echo cannot have completed"),
        observed = gate.first_loss() => assert_eq!(observed, header),
    }
    let (observed_link, id) = probe.take().await;
    assert_eq!(observed_link, link);
    assert_eq!(gate.stop_loss(), 1);
    AbandonedRequest { id, started }
}

fn exercise(requester: Requester) {
    for phase in [
        Phase::BeforeFirstSegment,
        Phase::BetweenSegments,
        Phase::DuringContinuation,
    ] {
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
            // Keep the abandoned protocol receipt alongside both new requests;
            // receipt pressure must not substitute for dropping the local waiter.
            scenario_with_receipts::<RETAINED_RECEIPTS>(
                embedded_endpoint,
                desktop_endpoint,
                TRACE_CAPACITY,
                |tasks, lab, embedded, desktop, links| {
                    match requester {
                        Requester::Tokio => {
                            let handle = desktop.handle.clone();
                            let probe = embedded.requests.clone();
                            let gate = embedded.wire.clone();
                            let old = complete(tasks, async move {
                                abandon(
                                    &probe,
                                    &gate,
                                    links[1],
                                    handle.request(
                                        links[1],
                                        RequestPathHash::of(crate::echo::QUERY_PATH),
                                        OLD_BYTES,
                                    ),
                                )
                                .await
                            });
                            assert_eq!(
                                response_settlements(embedded),
                                [Settlement::Respond(Ok(()))]
                            );
                            assert!(desktop.responses.is_empty());
                            let handle = desktop.handle.clone();
                            let responder = embedded.handle;
                            let probe = embedded.requests.clone();
                            let gate = embedded.wire.clone();
                            let trace = desktop.responses.clone();
                            let (result, elapsed) = complete(tasks, async move {
                                let data = (TRANSFER_BYTES as u16).to_be_bytes();
                                measured(compete(
                                    &responder,
                                    &probe,
                                    &gate,
                                    links[1],
                                    &Competition {
                                        phase,
                                        claim: ResponseClaim::Abandoned(old.id),
                                    },
                                    handle.request(
                                        links[1],
                                        RequestPathHash::of(files::FILE_PATH),
                                        &data,
                                    ),
                                    async {
                                        let (result, elapsed) = measured(handle.request(
                                            links[1],
                                            RequestPathHash::of(crate::echo::QUERY_PATH),
                                            LIVE_BYTES,
                                        ))
                                        .await;
                                        assert_eq!(result, Ok((LIVE_BYTES.to_vec(), elapsed)));
                                        assert!(
                                            trace.is_empty(),
                                            "Tokio's abandoned completion remains private"
                                        );
                                    },
                                ))
                                .await
                            });
                            assert_eq!(
                                result,
                                Ok((files::FILE_BYTES[..TRANSFER_BYTES].to_vec(), elapsed))
                            );
                            assert_eq!(
                                response_settlements(embedded),
                                [const { Settlement::Respond(Ok(())) }; 3]
                            );
                            assert!(desktop.responses.is_empty());
                        }
                        Requester::Embassy => {
                            let handle = embedded.handle;
                            let probe = desktop.requests.clone();
                            let gate = desktop.wire.clone();
                            let old = complete(tasks, async move {
                                abandon(
                                    &probe,
                                    &gate,
                                    links[0],
                                    handle.request(
                                        links[0],
                                        RequestPathHash::of(crate::echo::QUERY_PATH),
                                        OLD_BYTES,
                                    ),
                                )
                                .await
                            });
                            assert!(embedded.take_settled().is_empty());
                            assert!(embedded.responses.is_empty());
                            let responder = desktop.handle.clone();
                            let probe = desktop.requests.clone();
                            let gate = desktop.wire.clone();
                            let trace = embedded.responses.clone();
                            let (settled, mut settlement) = tokio::sync::oneshot::channel();
                            let (result, elapsed) = complete(tasks, async move {
                                let data = (TRANSFER_BYTES as u16).to_be_bytes();
                                measured(compete(
                                    &responder,
                                    &probe,
                                    &gate,
                                    links[0],
                                    &Competition {
                                        phase,
                                        claim: ResponseClaim::Abandoned(old.id),
                                    },
                                    handle.request(
                                        links[0],
                                        RequestPathHash::of(files::FILE_PATH),
                                        &data,
                                    ),
                                    async {
                                        let (result, elapsed) = measured(handle.request(
                                            links[0],
                                            RequestPathHash::of(crate::echo::QUERY_PATH),
                                            LIVE_BYTES,
                                        ))
                                        .await;
                                        assert_eq!(
                                            result.map(|(bytes, rtt)| (
                                                bytes.as_slice().to_vec(),
                                                rtt
                                            )),
                                            Ok((LIVE_BYTES.to_vec(), elapsed))
                                        );
                                        let events = trace.completed().await;
                                        let Some(ResponseEvent::Settled { command, .. }) =
                                            events.last()
                                        else {
                                            unreachable!(
                                                "abandoned request must settle at the application"
                                            )
                                        };
                                        let command = *command;
                                        let result = Ok(PacketReceiptDelivered {
                                            rtt: RttMillis::new(
                                                old.started
                                                    .elapsed()
                                                    .as_millis()
                                                    .try_into()
                                                    .unwrap(),
                                            ),
                                            evidence: DeliveryEvidence::Response,
                                        });
                                        assert_eq!(
                                            events,
                                            [
                                                ResponseEvent::Whole {
                                                    link: links[0],
                                                    request: old.id,
                                                    bytes: COMPETING_BYTES.to_vec()
                                                },
                                                ResponseEvent::Settled { command, result },
                                            ]
                                        );
                                        settled
                                            .send((command, Settlement::SendRequest(result)))
                                            .unwrap();
                                    },
                                ))
                                .await
                            });
                            assert_eq!(
                                result.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
                                Ok((files::FILE_BYTES[..TRANSFER_BYTES].to_vec(), elapsed))
                            );
                            assert_eq!(embedded.take_settled(), [settlement.try_recv().unwrap()]);
                            assert!(embedded.responses.is_empty());
                        }
                    }
                    reuse_both_embassy_request_slots(tasks, embedded, links[0]);
                    reassemble(tasks, lab, embedded, desktop, links);
                },
            );
        }
    }
}

#[test]
fn tokio_abandoned_waiter_cannot_contaminate_replacement() {
    exercise(Requester::Tokio);
}

#[test]
fn embassy_abandoned_waiter_releases_slots_without_redirecting_late_response() {
    exercise(Requester::Embassy);
}
