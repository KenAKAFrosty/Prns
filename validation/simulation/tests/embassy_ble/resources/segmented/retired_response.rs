use super::competition::{compete, Competition, Phase, ResponseClaim};
use super::*;
use crate::request_probe::RequestProbe;
use crate::wire_gate::WireGate;
use personal_rns::routing::links::request::RequestId;
use personal_rns::units::DurationMillis;
use personal_rns::wire::{WireContext, WirePacketHeader};

#[derive(Clone, Copy)]
enum EndedBy {
    Success,
    Timeout,
}

const OLD_BYTES: &[u8] = b"previous buffered request";
const SURVIVOR_BYTES: &[u8] = b"unrelated request still works";
const OLD_TIMEOUT: RequestResponseTimeout = RequestResponseTimeout::Exact(DurationMillis(1_000));

async fn finish_old<T>(
    probe: &RequestProbe,
    gate: &WireGate,
    link: LinkId,
    ended_by: EndedBy,
    request: impl Future<Output = T>,
) -> (T, RequestId) {
    probe.arm(link, RequestPathHash::of(crate::echo::QUERY_PATH));
    if let EndedBy::Timeout = ended_by {
        gate.lose_after(
            WirePacketHeader {
                context: WireContext::Response,
                ..buffered_interruption::advertisement_header(link)
            },
            0,
            NonZeroUsize::MIN,
        );
    }
    let (result, (observed_link, id)) = tokio::join!(request, probe.take());
    assert_eq!(observed_link, link);
    if let EndedBy::Timeout = ended_by {
        assert_eq!(
            gate.stop_loss(),
            1,
            "the real echo response must have been lost"
        );
    }
    assert!(probe.is_idle());
    assert!(gate.is_idle());
    (result, id)
}

fn exercise(ended_by: EndedBy, requester: Requester) {
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
            scenario(
                embedded_endpoint,
                desktop_endpoint,
                TRACE_CAPACITY,
                |tasks, lab, embedded, desktop, links| {
                    match requester {
                        Requester::Tokio => {
                            let handle = desktop.handle.clone();
                            let probe = embedded.requests.clone();
                            let gate = embedded.wire.clone();
                            let ((result, old_id), elapsed) = complete(tasks, async move {
                                measured(finish_old(
                                    &probe,
                                    &gate,
                                    links[1],
                                    ended_by,
                                    handle.request_with_options(
                                        links[1],
                                        RequestPathHash::of(crate::echo::QUERY_PATH),
                                        OLD_BYTES,
                                        RequestOptions {
                                            response_timeout: OLD_TIMEOUT,
                                            maximum_response_bytes: ByteLimit::Maximum(
                                                OLD_BYTES.len() as u64,
                                            ),
                                        },
                                    ),
                                ))
                                .await
                            });
                            assert_eq!(
                                result,
                                match ended_by {
                                    EndedBy::Success => Ok((OLD_BYTES.to_vec(), elapsed)),
                                    EndedBy::Timeout =>
                                        Err(SendError::Failed(SendRequestFailure::Timeout)),
                                }
                            );
                            if let EndedBy::Timeout = ended_by {
                                assert_eq!(elapsed, RttMillis::new(1_000));
                            }
                            assert!(desktop.responses.is_empty());
                            assert_eq!(
                                response_settlements(embedded),
                                [Settlement::Respond(Ok(()))]
                            );

                            let handle = desktop.handle.clone();
                            let responder = embedded.handle;
                            let probe = embedded.requests.clone();
                            let gate = embedded.wire.clone();
                            let (result, elapsed) = complete(tasks, async move {
                                let data = (TRANSFER_BYTES as u16).to_be_bytes();
                                measured(compete(
                                    &responder,
                                    &probe,
                                    &gate,
                                    links[1],
                                    &Competition {
                                        phase,
                                        claim: ResponseClaim::Retired(old_id),
                                    },
                                    handle.request_with_options(
                                        links[1],
                                        RequestPathHash::of(files::FILE_PATH),
                                        &data,
                                        RequestOptions {
                                            response_timeout: RequestResponseTimeout::LinkDefault,
                                            maximum_response_bytes: ByteLimit::Maximum(
                                                TRANSFER_BYTES as u64,
                                            ),
                                        },
                                    ),
                                    async {
                                        let (result, elapsed) = measured(handle.request(
                                            links[1],
                                            RequestPathHash::of(crate::echo::QUERY_PATH),
                                            SURVIVOR_BYTES,
                                        ))
                                        .await;
                                        assert_eq!(result, Ok((SURVIVOR_BYTES.to_vec(), elapsed)));
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
                            let ((result, old_id), elapsed) = complete(tasks, async move {
                                measured(finish_old(
                                    &probe,
                                    &gate,
                                    links[0],
                                    ended_by,
                                    handle.request_with_response_timeout(
                                        links[0],
                                        RequestPathHash::of(crate::echo::QUERY_PATH),
                                        OLD_BYTES,
                                        OLD_TIMEOUT,
                                    ),
                                ))
                                .await
                            });
                            assert_eq!(
                                result.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
                                match ended_by {
                                    EndedBy::Success => Ok((OLD_BYTES.to_vec(), elapsed)),
                                    EndedBy::Timeout =>
                                        Err(SendError::Failed(SendRequestFailure::Timeout)),
                                }
                            );
                            if let EndedBy::Timeout = ended_by {
                                assert_eq!(elapsed, RttMillis::new(1_000));
                            }
                            assert!(embedded.responses.is_empty());
                            assert!(embedded.take_settled().is_empty());

                            let responder = desktop.handle.clone();
                            let probe = desktop.requests.clone();
                            let gate = desktop.wire.clone();
                            let (result, elapsed) = complete(tasks, async move {
                                let data = (TRANSFER_BYTES as u16).to_be_bytes();
                                measured(compete(
                                    &responder,
                                    &probe,
                                    &gate,
                                    links[0],
                                    &Competition {
                                        phase,
                                        claim: ResponseClaim::Retired(old_id),
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
                                            SURVIVOR_BYTES,
                                        ))
                                        .await;
                                        assert_eq!(
                                            result.map(|(bytes, rtt)| (
                                                bytes.as_slice().to_vec(),
                                                rtt
                                            )),
                                            Ok((SURVIVOR_BYTES.to_vec(), elapsed))
                                        );
                                    },
                                ))
                                .await
                            });
                            assert_eq!(
                                result.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
                                Ok((files::FILE_BYTES[..TRANSFER_BYTES].to_vec(), elapsed))
                            );
                            assert!(embedded.responses.is_empty());
                            assert!(embedded.take_settled().is_empty());
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
fn tokio_retired_response_after_success_preserves_replacement() {
    exercise(EndedBy::Success, Requester::Tokio);
}

#[test]
fn embassy_retired_response_after_success_preserves_replacement() {
    exercise(EndedBy::Success, Requester::Embassy);
}

#[test]
fn tokio_retired_response_after_timeout_preserves_replacement() {
    exercise(EndedBy::Timeout, Requester::Tokio);
}

#[test]
fn embassy_retired_response_after_timeout_preserves_replacement() {
    exercise(EndedBy::Timeout, Requester::Embassy);
}
