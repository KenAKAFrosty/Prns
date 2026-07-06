use crate::engine::{write_path_request_wire_packet, EgressSerializeError};
use crate::engine::{CommandId, RequestPath};
use crate::engine::{EngineState, InstantMillis};
use crate::routing::path_requests::pending::{
    CulledPathRequest, ExpiredPathRequest, PendingPathRequest, SettledPathRequest,
    PATH_REQUEST_TIMEOUT_MS,
};
use crate::storage::StorageLayout;
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

impl<S: StorageLayout> EngineState<S> {
    pub fn write_commanded_path_request(
        &mut self,
        id: CommandId,
        request: &RequestPath,
        now: InstantMillis,
        buf: &mut [u8],
    ) -> PathRequestWriteOutcome {
        if let Some(stored) = self.routing_table.stored_announce_for(&request.destination) {
            return PathRequestWriteOutcome::AlreadyReachable { hops: stored.hops };
        }

        let wire_len = match write_path_request_wire_packet(
            request.destination,
            self.transport_id,
            request.id.as_bytes(),
            buf,
        ) {
            Ok(wire_len) => wire_len,
            Err(error) => return PathRequestWriteOutcome::SerializeFailed(error),
        };

        let culled = self.pending_path_requests.track(PendingPathRequest {
            destination: request.destination,
            command_id: id,
            timeout_at: InstantMillis(now.0.saturating_add(PATH_REQUEST_TIMEOUT_MS)),
        });
        self.recent_path_requests
            .mark_seen_at(request.destination, now);

        PathRequestWriteOutcome::Written { wire_len, culled }
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
