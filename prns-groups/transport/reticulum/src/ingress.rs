//! Bounded hand-off from the application's shared Reticulum event dispatcher.

use personal_rns::routing::links::request::parse_packed_binary;
use personal_rns::runtime::{Message, PrnsEvent};
use prns_host::{DestinationHash, IdentityHash, LinkId, RequestId, RequestPathHash};
use tokio::sync::mpsc;

use crate::network::{request_path, validate_limits};
use crate::Limits;
use crate::NetworkError;

/// An identified request copied from the host callback. Its data is still
/// untrusted carrier input; the synchronization handler must validate it.
pub struct RequestJob {
    pub(crate) destination: DestinationHash,
    pub(crate) path: RequestPathHash,
    pub(crate) requester: IdentityHash,
    pub(crate) link: LinkId,
    pub(crate) request: RequestId,
    pub(crate) rtt_millis: u64,
    pub(crate) frame: Vec<u8>,
}

impl RequestJob {
    /// Identity verified by Reticulum Link identification, never payload data.
    #[must_use]
    pub const fn requester(&self) -> IdentityHash {
        self.requester
    }

    /// Link carrying this identified request, useful for scoped diagnostics.
    #[must_use]
    pub const fn link(&self) -> LinkId {
        self.link
    }

    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.frame
    }
}

impl core::fmt::Debug for RequestJob {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RequestJob")
            .field("requester", &self.requester)
            .field("destination", &self.destination)
            .field("frame_bytes", &self.frame.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngressOutcome {
    Ignored,
    Queued,
    Rejected,
    Full,
}

/// Clones share one bounded queue. The caller owns queue draining and shutdown.
#[derive(Clone)]
pub struct Ingress {
    local_destination: DestinationHash,
    path: RequestPathHash,
    maximum_frame_bytes: usize,
    sender: mpsc::Sender<RequestJob>,
}

impl Ingress {
    pub fn new(
        local_destination: DestinationHash,
        limits: Limits,
    ) -> Result<(Self, mpsc::Receiver<RequestJob>), NetworkError> {
        validate_limits(limits)?;
        Ok(Self::for_path(
            local_destination,
            request_path(),
            limits.max_frame_bytes,
            limits.inbound_jobs,
        ))
    }

    pub(crate) fn for_path(
        local_destination: DestinationHash,
        path: RequestPathHash,
        maximum_frame_bytes: usize,
        inbound_jobs: usize,
    ) -> (Self, mpsc::Receiver<RequestJob>) {
        let (sender, receiver) = mpsc::channel(inbound_jobs);
        (
            Self {
                local_destination,
                path,
                maximum_frame_bytes,
                sender,
            },
            receiver,
        )
    }

    /// Filter before allocation; reserve queue capacity before copying bytes.
    /// An overflow is safe to retry because no group transaction has run.
    pub fn on_event(&self, event: PrnsEvent<'_>) -> IngressOutcome {
        self.on_event_ref(&event)
    }

    /// Borrowed callback dispatch lets other application protocols observe the
    /// same host event without cloning packet payloads.
    pub fn on_event_ref(&self, event: &PrnsEvent<'_>) -> IngressOutcome {
        let PrnsEvent::Message(Message::Request {
            destination,
            link_id,
            request_id,
            requester,
            path_hash,
            rtt,
            data,
            ..
        }) = event
        else {
            return IngressOutcome::Ignored;
        };
        if destination.as_bytes() != self.local_destination.as_bytes()
            || path_hash.as_bytes() != self.path.as_bytes()
        {
            return IngressOutcome::Ignored;
        }
        let Some(requester) = requester else {
            return IngressOutcome::Rejected;
        };
        let Ok(frame) = parse_packed_binary(data) else {
            return IngressOutcome::Rejected;
        };
        if frame.len() > self.maximum_frame_bytes {
            return IngressOutcome::Rejected;
        }
        let permit = match self.sender.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(_)) => return IngressOutcome::Full,
            Err(mpsc::error::TrySendError::Closed(_)) => return IngressOutcome::Rejected,
        };
        let mut copied = Vec::new();
        if copied.try_reserve_exact(frame.len()).is_err() {
            return IngressOutcome::Rejected;
        }
        copied.extend_from_slice(frame);
        permit.send(RequestJob {
            destination: self.local_destination,
            path: self.path,
            requester: IdentityHash::new(*requester.as_bytes()),
            link: LinkId::new(*link_id.as_bytes()),
            request: RequestId::new(*request_id.as_bytes()),
            rtt_millis: rtt.millis(),
            frame: copied,
        });
        IngressOutcome::Queued
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use personal_rns::engine::InstantMillis;
    use personal_rns::routing::links::request::RequestId as EngineRequestId;
    use personal_rns::routing::links::LinkId as EngineLinkId;
    use personal_rns::routing::request_handlers::RequestPathHash as EngineRequestPathHash;
    use personal_rns::units::RttMillis;
    use std::time::Duration;

    fn limits() -> Limits {
        Limits {
            max_frame_bytes: 134,
            max_page_items: 1,
            max_peers: 3,
            inbound_jobs: 1,
            exchange_timeout: Duration::from_secs(3),
        }
    }

    fn request<'a>(data: &'a [u8], destination: u8, path: &str, identified: bool) -> PrnsEvent<'a> {
        PrnsEvent::Message(Message::Request {
            destination: personal_rns::wire::DestinationHash::new([destination; 16]),
            link_id: EngineLinkId::new([2; 16]),
            request_id: EngineRequestId([3; 16]),
            requester: identified.then_some(personal_rns::identity::IdentityHash::new([4; 16])),
            path_hash: EngineRequestPathHash::of(path),
            requested_at: InstantMillis(0),
            rtt: RttMillis::new(7),
            data,
        })
    }

    #[test]
    fn callback_filters_and_bounds_before_queueing() {
        let (ingress, mut jobs) = Ingress::new(DestinationHash::new([1; 16]), limits()).unwrap();
        let encoded = [0xc4, 3, 9, 8, 7];
        assert_eq!(
            ingress.on_event(request(&encoded, 2, crate::REQUEST_PATH, true)),
            IngressOutcome::Ignored
        );
        assert_eq!(
            ingress.on_event(request(&encoded, 1, "/other", true)),
            IngressOutcome::Ignored
        );
        assert_eq!(
            ingress.on_event(request(&encoded, 1, crate::REQUEST_PATH, false)),
            IngressOutcome::Rejected
        );
        assert_eq!(
            ingress.on_event(request(&encoded, 1, crate::REQUEST_PATH, true)),
            IngressOutcome::Queued
        );
        assert_eq!(
            ingress.on_event(request(&encoded, 1, crate::REQUEST_PATH, true)),
            IngressOutcome::Full
        );
        let job = jobs.try_recv().unwrap();
        assert_eq!(job.data(), [9, 8, 7]);
        assert_eq!(job.requester, IdentityHash::new([4; 16]));
        assert_eq!(job.rtt_millis, 7);
        assert_eq!(
            ingress.on_event(request(&encoded, 1, crate::REQUEST_PATH, true)),
            IngressOutcome::Queued
        );
    }

    #[test]
    fn malformed_oversized_and_closed_jobs_are_not_accepted() {
        let (ingress, jobs) = Ingress::new(DestinationHash::new([1; 16]), limits()).unwrap();
        let malformed = [0xc4, 1, 9, 8];
        assert_eq!(
            ingress.on_event(request(&malformed, 1, crate::REQUEST_PATH, true)),
            IngressOutcome::Rejected
        );
        let mut oversized = vec![0xc4, 135];
        oversized.extend_from_slice(&[0; 135]);
        assert_eq!(
            ingress.on_event(request(&oversized, 1, crate::REQUEST_PATH, true)),
            IngressOutcome::Rejected
        );
        drop(jobs);
        assert_eq!(
            ingress.on_event(request(&[0xc4, 0], 1, crate::REQUEST_PATH, true)),
            IngressOutcome::Rejected
        );
    }
}
