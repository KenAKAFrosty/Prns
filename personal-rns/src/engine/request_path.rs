use crate::engine::commands::{CommandId, RequestPath};
use crate::engine::egress::{
    write_path_request_wire_packet, write_retransmitted_announce_wire_packet, EgressSerializeError,
};
use crate::engine::pending_path_requests::{
    CulledPathRequest, ExpiredPathRequest, PendingPathRequest, SettledPathRequest,
    PATH_REQUEST_TIMEOUT_MS,
};
use crate::engine::{EngineState, InstantMillis};
use crate::routing::storage::EngineStorage;
use crate::wire::DestinationHash;

#[must_use]
pub enum PathRequestWriteOutcome {
    AlreadyReachable {
        hops: u8,
    },
    Written {
        wire_len: usize,
        culled: Option<CulledPathRequest>,
    },
    SerializeFailed(EgressSerializeError),
}

#[must_use]
pub enum CachedPathResponseOutcome {
    Written { wire_len: usize },
    Unavailable,
}

impl<S: EngineStorage> EngineState<S> {
    pub fn write_commanded_path_request(
        &mut self,
        id: CommandId,
        request: &RequestPath,
        now: InstantMillis,
        buf: &mut [u8],
    ) -> PathRequestWriteOutcome {
        if let Some(retained) = self
            .routing_table
            .retained_announce_for(&request.destination)
        {
            return PathRequestWriteOutcome::AlreadyReachable {
                hops: retained.hops,
            };
        }

        let wire_len =
            match write_path_request_wire_packet(request.destination, request.id.as_bytes(), buf) {
                Ok(wire_len) => wire_len,
                Err(error) => return PathRequestWriteOutcome::SerializeFailed(error),
            };

        let culled = self.pending_path_requests.track(PendingPathRequest {
            destination: request.destination,
            command_id: id,
            timeout_at: InstantMillis(now.0.saturating_add(PATH_REQUEST_TIMEOUT_MS)),
        });

        PathRequestWriteOutcome::Written { wire_len, culled }
    }

    /// RNS 1.3.1 `Transport.path_request`'s cached-packet branch
    pub fn write_cached_path_response(
        &self,
        destination: &DestinationHash,
        buf: &mut [u8],
    ) -> CachedPathResponseOutcome {
        let (Some(retained), Some(via)) = (
            self.routing_table.retained_announce_for(destination),
            self.transport_id,
        ) else {
            return CachedPathResponseOutcome::Unavailable;
        };
        match write_retransmitted_announce_wire_packet(&retained.announce, retained.hops, via, buf)
        {
            Ok(wire_len) => CachedPathResponseOutcome::Written { wire_len },
            Err(_) => CachedPathResponseOutcome::Unavailable,
        }
    }

    pub fn pop_settled_path_request(
        &mut self,
        destination: &DestinationHash,
    ) -> Option<SettledPathRequest> {
        self.pending_path_requests.pop_settled_for(destination)
    }

    /// Drain one pending request whose timeout has passed. Call repeatedly until
    /// `None` to fully drain. Every pop is that command's timeout settlement.
    pub fn pop_timed_out_path_request(&mut self, now: InstantMillis) -> Option<ExpiredPathRequest> {
        self.pending_path_requests.pop_expired(now)
    }
}
