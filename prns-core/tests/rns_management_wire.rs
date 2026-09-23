#![cfg(feature = "rns-management-wire")]

use prns_core::engine::RouteSnapshot;
use prns_core::interfaces::rns_management::{
    decode_remote_path_request, write_route_snapshots, RnsPathTableWriteError, RnsPathTableWriter,
    RnsRemotePathRequest, RnsRemotePathTableRequest,
};
use prns_core::interfaces::InterfaceId;
use prns_core::routing::{NextHop, RouteRetention};
use prns_core::units::InstantMillis;
use prns_core::wire::DestinationHash;

#[test]
fn path_request_decodes_without_allocation_support() {
    let request = decode_remote_path_request(b"\x93\xa5table\xc0\xc0").unwrap();
    let expected = RnsRemotePathRequest::Table(RnsRemotePathTableRequest::new(None, None));

    assert_eq!(request, expected);
}

#[test]
fn route_snapshot_writes_without_allocation_support() {
    let route = RouteSnapshot {
        destination: DestinationHash::new([0x42; 16]),
        hops: 1,
        via: NextHop::Direct,
        learned_at: InstantMillis(1),
        last_route_activity_at: InstantMillis(2),
        expires_at: InstantMillis(3),
        interface: InterfaceId::new([0x84; 8]),
        retention: RouteRetention::Network,
    };
    let mut output = [0u8; 256];

    let written = write_route_snapshots(core::slice::from_ref(&route), &mut output).unwrap();
    let mut streamed = [0u8; 256];
    let mut writer = RnsPathTableWriter::new(1, &mut streamed).unwrap();
    writer.push(&route).unwrap();
    let streamed_len = writer.finish().unwrap();

    assert!(written > 1);
    assert_eq!(output[0], 0x91);
    assert_eq!(&streamed[..streamed_len], &output[..written]);
    assert_eq!(
        RnsPathTableWriter::new(1, &mut streamed).unwrap().finish(),
        Err(RnsPathTableWriteError::EntryCountMismatch)
    );
}
