use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;

use prns_core::engine::RouteSnapshot;
use prns_core::identity::IdentityHash;
use prns_core::interfaces::rns_management::{RnsPathTable, RnsTransportStatus};

pub use prns_core::interfaces::rns_management::{
    decode_remote_path_request as decode_path_request,
    decode_remote_status_request as decode_status_request,
    RnsManagementEncodeError as RemoteResponseEncodeError,
    RnsRemotePathRequest as RemotePathRequest, RnsRemotePathTableRequest as RemotePathTableRequest,
    RnsRemoteRateTableRequest as RemoteRateTableRequest,
    RnsRemoteRequestDecodeError as RemoteRequestDecodeError,
    RnsRemoteStatusRequest as RemoteStatusRequest,
};

use super::node_introspection::{AnnounceRateSnapshot, InterfaceInventoryEntry};
use super::rns_management::{announce_rate_table, interface_stats};

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTransportStatus {
    pub transport_identity: IdentityHash,
    pub network_identity: Option<IdentityHash>,
    pub uptime: Duration,
    pub probe_responder: Option<prns_core::wire::DestinationHash>,
    pub software_version: Option<String>,
}

pub fn encode_status_response(
    request: RemoteStatusRequest,
    inventory: Vec<InterfaceInventoryEntry<String>>,
    link_count: u32,
    transport: Option<RemoteTransportStatus>,
) -> Result<Vec<u8>, RemoteResponseEncodeError> {
    let mut stats = interface_stats(inventory);
    if let Some(transport) = transport {
        let mut status = RnsTransportStatus::new(
            transport.transport_identity,
            transport.network_identity,
            transport.uptime,
        )
        .with_probe_responder(transport.probe_responder);
        if let Some(software_version) = transport.software_version {
            status = status.with_software_version(software_version);
        }
        stats = stats.with_transport(status);
    }
    let link_count =
        (request == RemoteStatusRequest::InterfaceStatsAndLinkCount).then_some(link_count);
    stats.encode_remote_response(link_count)
}

pub fn encode_path_table_response(
    selection: RemotePathTableRequest,
    entries: Vec<RouteSnapshot>,
) -> Result<Vec<u8>, RemoteResponseEncodeError> {
    let entries = entries
        .into_iter()
        .filter(|entry| selection.includes(entry.destination, entry.hops))
        .collect();
    RnsPathTable::new(entries).encode_message_pack()
}

pub fn encode_rate_table_response(
    selection: RemoteRateTableRequest,
    entries: Vec<AnnounceRateSnapshot>,
) -> Result<Vec<u8>, RemoteResponseEncodeError> {
    let entries = entries
        .into_iter()
        .filter(|entry| selection.includes(entry.destination))
        .collect();
    announce_rate_table(entries).encode_message_pack()
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::{InterfaceId, InterfaceKind};
    use prns_core::routing::NextHop;
    use prns_core::units::InstantMillis;
    use prns_core::wire::DestinationHash;

    #[test]
    fn table_response_filters_before_using_the_shared_stock_projection() {
        let selected = DestinationHash::new([0x42; 16]);
        let entries = vec![
            route(selected, 2),
            route(DestinationHash::new([0x43; 16]), 1),
            route(selected, 4),
        ];
        let Ok(RemotePathRequest::Table(selection)) = decode_path_request(&bytes_from_hex(
            "93a57461626c65c4104242424242424242424242424242424203",
        )) else {
            panic!("stock fixture is a path-table request");
        };

        let encoded = encode_path_table_response(selection, entries).unwrap();

        assert_eq!(
            encoded,
            RnsPathTable::new(vec![route(selected, 2)])
                .encode_message_pack()
                .unwrap()
        );
    }

    #[test]
    fn status_response_has_the_reference_outer_shape_and_transport_fields() {
        let encoded = encode_status_response(
            RemoteStatusRequest::InterfaceStatsAndLinkCount,
            Vec::new(),
            2,
            Some(RemoteTransportStatus {
                transport_identity: IdentityHash::new([0x11; 16]),
                network_identity: Some(IdentityHash::new([0x22; 16])),
                uptime: Duration::from_millis(1_500),
                probe_responder: None,
                software_version: None,
            }),
        )
        .unwrap();

        assert_eq!(
            encoded,
            bytes_from_hex(
                "928aaa696e746572666163657390a372786200a374786200a372787300a374787300a3727373c0ac7472616e73706f72745f6964c41011111111111111111111111111111111aa6e6574776f726b5f6964c41022222222222222222222222222222222b07472616e73706f72745f757074696d65cb3ff8000000000000af70726f62655f726573706f6e646572c002",
            )
        );
    }

    fn route(destination: DestinationHash, hops: u8) -> RouteSnapshot {
        RouteSnapshot {
            destination,
            hops,
            via: NextHop::Direct,
            learned_at: InstantMillis(1_000),
            last_route_activity_at: InstantMillis(1_500),
            expires_at: InstantMillis(2_000),
            interface: InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"remote"),
        }
    }

    fn bytes_from_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
}
