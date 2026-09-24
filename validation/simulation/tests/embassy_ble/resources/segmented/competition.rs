use super::*;
use crate::request_probe::RequestProbe;
use crate::wire_gate::WireGate;
use personal_rns::engine::{Respond, RespondData, RespondPayload};
use personal_rns::routing::links::request::RequestId;
use personal_rns::wire::{WireContext, WirePacketHeader};
use std::cell::Cell;

#[derive(Clone, Copy)]
pub(super) enum Phase {
    BeforeFirstSegment,
    BetweenSegments,
    DuringContinuation,
}

pub(super) enum ResponseClaim {
    Current,
    Retired(RequestId),
    Abandoned(RequestId),
}

pub(super) struct Competition {
    pub phase: Phase,
    pub claim: ResponseClaim,
}

pub(super) const COMPETING_BYTES: &[u8] = b"not the requested file";
const FIRST_SEGMENT_WIRE_PARTS: usize = 2;

pub(super) async fn compete<T>(
    responder: &impl PrnsNodeApi,
    probe: &RequestProbe,
    gate: &WireGate,
    link: LinkId,
    competition: &Competition,
    request: impl Future<Output = T>,
    barrier: impl Future<Output = ()>,
) -> T {
    let advertisement = buffered_interruption::advertisement_header(link);
    let part = WirePacketHeader {
        context: WireContext::Resource,
        ..advertisement
    };
    let (header, passes) = match competition.phase {
        Phase::BeforeFirstSegment => (part, 0),
        Phase::BetweenSegments => (advertisement, 1),
        Phase::DuringContinuation => (part, FIRST_SEGMENT_WIRE_PARTS),
    };
    probe.arm(link, RequestPathHash::of(files::FILE_PATH));
    gate.lose_after(header, passes, NonZeroUsize::new(16).unwrap());
    let completed = Cell::new(false);
    let (result, ()) = tokio::join!(
        async {
            let result = request.await;
            completed.set(true);
            result
        },
        async {
            let (observed_link, request_id) = probe.take().await;
            assert_eq!(observed_link, link);
            let request_id = match competition.claim {
                ResponseClaim::Current => request_id,
                ResponseClaim::Retired(retired) | ResponseClaim::Abandoned(retired) => {
                    assert_ne!(
                        retired, request_id,
                        "replacement must have a fresh request identity"
                    );
                    retired
                }
            };
            assert_eq!(gate.first_loss().await, header);
            assert!(responder
                .issue(PrnsCommand::Respond(Respond {
                    link_id: link,
                    request_id,
                    payload: RespondPayload::Packed(
                        RespondData::from_slice(COMPETING_BYTES).unwrap()
                    ),
                }))
                .is_some());
            barrier.await;
            assert!(
                !completed.get(),
                "competing packet must not complete the buffered request"
            );
            assert!(gate.stop_loss() > 0);
        },
    );
    assert!(probe.is_idle());
    assert!(gate.is_idle());
    result
}
