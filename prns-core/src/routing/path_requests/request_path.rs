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
    Written {
        wire_len: usize,
        culled: Option<CulledPathRequest>,
    },
    SerializeFailed(EgressSerializeError),
}

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.3.5 `Transport.request_path` emits unconditionally: an existing route never blocks the request, so a suspect path stays refreshable.
    pub fn write_commanded_path_request(
        &mut self,
        id: CommandId,
        request: &RequestPath,
        now: InstantMillis,
        buf: &mut [u8],
    ) -> PathRequestWriteOutcome {
        let wire_len = match write_path_request_wire_packet(
            request.destination,
            self.network_transport_enabled()
                .then(|| self.transport_id())
                .flatten(),
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

    /// Drain one pending request whose timeout has passed. Call repeatedly until `None` to fully drain. Every pop is that command's timeout settlement.
    pub fn pop_timed_out_path_request(&mut self, now: InstantMillis) -> Option<ExpiredPathRequest> {
        self.pending_path_requests.pop_expired(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{AnnounceIngest, IngestPacketOutcome, PathRequestId};
    use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceId};
    use crate::routing::announce::{derive_plain_destination_hash, expand_name};
    use crate::wire::{WirePacketHeader, BROADCAST_MTU};

    #[test]
    fn a_live_route_never_blocks_a_path_request() {
        let mut state: EngineState<TestStorageLayout> = EngineState::new(second_secret_key());
        let mut announce = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let (header, _) = WirePacketHeader::parse(&announce).expect("the announce fixture parses");
        let destination = DestinationHash::from_address(header.address);

        let outcome = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert!(
            matches!(
                outcome,
                IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_))
            ),
            "the announce fixture must take a route first",
        );

        let mut buf = [0u8; BROADCAST_MTU];
        let outcome = state.write_commanded_path_request(
            CommandId(7),
            &RequestPath {
                destination,
                id: PathRequestId::new([0x55; 16]),
            },
            InstantMillis(1_000),
            &mut buf,
        );
        let PathRequestWriteOutcome::Written { wire_len, .. } = outcome else {
            panic!("RNS 1.3.5 Transport.request_path emits unconditionally; a live route must not block a refresh");
        };

        let (header, _) =
            WirePacketHeader::parse(&buf[..wire_len]).expect("the emitted packet parses");
        let path_request_destination = derive_plain_destination_hash(
            &expand_name("rnstransport", &["path", "request"]).expect("the well-known name"),
        );
        assert_eq!(
            DestinationHash::from_address(header.address),
            path_request_destination,
        );
        assert!(state.pending_path_requests.contains(&destination));
    }
}
