use crate::engine::test_support::{
    bytes_from_hex, routable_descriptor, transporting_node, RNS_1_4_2_ANNOUNCE,
};
use crate::engine::{
    CommandId, EngineReaction, IngestIo, InstantMillis, Journaled, PathFound, PathRequestId,
    RequestPath, Settlement,
};
use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceId};
use crate::routing::{
    announce::{
        write_path_response_announce_wire_packet, write_relayed_path_response_wire_packet, Announce,
    },
    RouteResponsiveness,
};
use crate::units::HopCount;
use crate::wire::{TransportId, WirePacketHeader, BROADCAST_MTU};

enum Response {
    KnownPath,
    OrdinaryEcho,
    OtherInterface,
    OtherHop,
    OtherNextHop,
    InvalidSignature,
    Unresponsive,
    Expired,
}

#[test]
fn a_verified_cached_response_completes_discovery_without_renewing_route_evidence() {
    for response in [
        Response::KnownPath,
        Response::OrdinaryEcho,
        Response::OtherInterface,
        Response::OtherHop,
        Response::OtherNextHop,
        Response::InvalidSignature,
        Response::Unresponsive,
        Response::Expired,
    ] {
        let source = InterfaceId::new([0xa1; 8]);
        let other = InterfaceId::new([0xb2; 8]);
        let descriptors = [routable_descriptor(source), routable_descriptor(other)];
        let interfaces = AttachedInterfaces::new(&descriptors);
        let raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();
        let destination = announce.destination;
        let mut engine = transporting_node();
        let mut initial = raw.clone();
        let _ = engine.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(1000),
                source_interface: source,
                bytes: &mut initial,
            },
            interfaces,
        );
        if matches!(response, Response::Unresponsive) {
            engine
                .routing_table
                .mark_responsiveness(&destination, RouteResponsiveness::Unresponsive);
        }
        let now = match response {
            Response::Expired => {
                engine
                    .routing_table
                    .existing_route_for(&destination, interfaces)
                    .unwrap()
                    .expires_at
            }
            _ => InstantMillis(2000),
        };
        let before = engine.routing_table.path_row(&destination).unwrap();
        let request_id = CommandId(88);
        let mut scratch = [0; BROADCAST_MTU];
        let _ = engine.write_commanded_path_request_with_interfaces(
            request_id,
            &RequestPath {
                destination,
                id: PathRequestId::new([0x31; 16]),
            },
            now,
            interfaces,
            &mut scratch,
        );
        let mut wire = [0; BROADCAST_MTU];
        let length = match response {
            Response::OtherNextHop => write_relayed_path_response_wire_packet(
                &announce,
                0,
                TransportId::new([0x77; 16]),
                &mut wire,
            )
            .unwrap(),
            _ => write_path_response_announce_wire_packet(
                &announce,
                if matches!(response, Response::OtherHop) {
                    1
                } else {
                    0
                },
                &mut wire,
            )
            .unwrap(),
        };
        if matches!(response, Response::OrdinaryEcho) {
            wire[..raw.len()].copy_from_slice(&raw);
        }
        if matches!(response, Response::InvalidSignature) {
            wire[length - 1] ^= 1;
        }
        let mut settlements = std::vec::Vec::new();
        let mut announcements = 0;
        crate::engine::drive_packet_to_quiescence(
            &mut engine,
            InboundPacket {
                arrived_at: now,
                source_interface: if matches!(response, Response::OtherInterface) {
                    other
                } else {
                    source
                },
                bytes: &mut wire[..length],
            },
            IngestIo {
                interfaces,
                now,
                fill_random: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                        settlements.push((id, settlement))
                    }
                    EngineReaction::Journaled(Journaled::AnnounceHeard { .. }) => {
                        announcements += 1
                    }
                    _ => {}
                },
            },
        );
        let expected = match response {
            Response::KnownPath => std::vec![(
                request_id,
                Settlement::RequestPath(Ok(PathFound { hops: HopCount(1) }))
            )],
            _ => std::vec::Vec::new(),
        };
        assert_eq!(settlements, expected);
        assert_eq!(announcements, 0);
        assert_eq!(engine.routing_table.path_row(&destination).unwrap(), before);
    }
}
