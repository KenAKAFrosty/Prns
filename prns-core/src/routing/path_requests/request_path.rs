use crate::engine::{CommandId, RequestPath};
use crate::engine::{EngineState, InstantMillis};
use crate::interfaces::AttachedInterfaces;
use crate::routing::path_requests::pending::{
    CulledPathRequest, ExpiredPathRequest, PendingPathRequest, SettledPathRequest,
};
use crate::routing::timing::{
    path_discovery_timeout_ms, path_request_egress_eligible, slowest_eligible_bitrate,
    PathRequestAudience,
};
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, WireError};

use super::write_path_request_wire_packet;

#[must_use]
pub enum PathRequestWriteOutcome {
    Written {
        wire_bytes: usize,
        culled: Option<CulledPathRequest>,
    },
    SerializeFailed(WireError),
}

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.4.2 `Transport.request_path` emits unconditionally: an existing route never blocks the request, so a suspect path stays refreshable.
    pub fn write_commanded_path_request(
        &mut self,
        id: CommandId,
        request: &RequestPath,
        now: InstantMillis,
        buf: &mut [u8],
    ) -> PathRequestWriteOutcome {
        self.write_commanded_path_request_with_interfaces(
            id,
            request,
            now,
            AttachedInterfaces::new(&[]),
            buf,
        )
    }

    pub fn write_commanded_path_request_with_interfaces(
        &mut self,
        id: CommandId,
        request: &RequestPath,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        buf: &mut [u8],
    ) -> PathRequestWriteOutcome {
        self.write_commanded_path_request_with_timing(id, request, now, interfaces, None, buf)
    }

    pub fn write_commanded_path_request_with_timing(
        &mut self,
        id: CommandId,
        request: &RequestPath,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        path_timeout_floor_ms: Option<u64>,
        buf: &mut [u8],
    ) -> PathRequestWriteOutcome {
        let wire_bytes = match write_path_request_wire_packet(
            request.destination,
            self.network_transport_enabled()
                .then(|| self.transport_id())
                .flatten(),
            request.id.as_bytes(),
            buf,
        ) {
            Ok(wire_bytes) => wire_bytes,
            Err(error) => return PathRequestWriteOutcome::SerializeFailed(error),
        };

        let slowest = slowest_eligible_bitrate(interfaces, |descriptor| {
            path_request_egress_eligible(descriptor, None, PathRequestAudience::Network)
        });
        let timeout_ms = path_timeout_floor_ms.map_or_else(
            || path_discovery_timeout_ms(slowest),
            |floor| path_discovery_timeout_ms(slowest).max(floor),
        );
        let culled = self.pending_path_requests.track(PendingPathRequest {
            destination: request.destination,
            command_id: id,
            timeout_at: InstantMillis(now.0.saturating_add(timeout_ms)),
        });
        self.recent_path_requests
            .mark_seen_at(request.destination, now);

        PathRequestWriteOutcome::Written { wire_bytes, culled }
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
    use crate::interfaces::{
        AttachedInterfaces, BitrateBps, EgressCapability, InboundPacket, InterfaceId, InterfaceKind,
    };
    use crate::routing::announce::{derive_plain_destination_hash, expand_name};
    use crate::wire::{WirePacketHeader, BROADCAST_MTU};

    #[test]
    fn a_live_route_never_blocks_a_path_request() {
        let mut state: EngineState<TestStorageLayout> = EngineState::new(second_secret_key());
        let mut announce = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
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
        let outcome = state.write_commanded_path_request_with_interfaces(
            CommandId(7),
            &RequestPath {
                destination,
                id: PathRequestId::new([0x55; 16]),
            },
            InstantMillis(1_000),
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut buf,
        );
        let PathRequestWriteOutcome::Written { wire_bytes, .. } = outcome else {
            panic!("RNS 1.4.2 Transport.request_path emits unconditionally; a live route must not block a refresh");
        };

        let (header, _) =
            WirePacketHeader::parse(&buf[..wire_bytes]).expect("the emitted packet parses");
        let path_request_destination = derive_plain_destination_hash(
            &expand_name("rnstransport", &["path", "request"]).expect("the well-known name"),
        );
        assert_eq!(
            DestinationHash::from_address(header.address),
            path_request_destination,
        );
        assert!(state.pending_path_requests.contains(&destination));
    }

    #[test]
    fn a_local_path_request_uses_only_eligible_fanout_bitrates() {
        let mut state: EngineState<TestStorageLayout> = EngineState::new(second_secret_key());
        let destination = DestinationHash::new([0x44; 16]);
        let mut slow = routable_descriptor(InterfaceId::new([0xA1; 8]));
        slow.bitrate = BitrateBps::guess(250);
        let mut disabled = routable_descriptor(InterfaceId::new([0xB2; 8]));
        disabled.bitrate = BitrateBps::guess(5);
        disabled.capabilities.egress = EgressCapability::Disabled;
        let mut local = routable_descriptor(InterfaceId::from_channel_tag(
            InterfaceKind::LocalClient,
            b"unrelated-client",
        ));
        local.bitrate = BitrateBps::guess(5);
        let interfaces = [slow, disabled, local];
        let mut buf = [0u8; BROADCAST_MTU];

        let outcome = state.write_commanded_path_request_with_interfaces(
            CommandId(7),
            &RequestPath {
                destination,
                id: PathRequestId::new([0x55; 16]),
            },
            InstantMillis(1_000),
            AttachedInterfaces::new(&interfaces),
            &mut buf,
        );
        assert!(matches!(outcome, PathRequestWriteOutcome::Written { .. }));
        assert_eq!(
            state.pop_timed_out_path_request(InstantMillis(38_999)),
            None
        );
        assert_eq!(
            state
                .pop_timed_out_path_request(InstantMillis(39_000))
                .map(|expired| expired.command_id),
            Some(CommandId(7)),
        );
    }
}
