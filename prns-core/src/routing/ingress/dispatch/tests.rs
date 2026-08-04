use super::*;
use crate::engine::test_support::*;
use crate::interfaces::InboundPacket;

#[test]
fn ingest_counts_each_packet_without_a_clock() {
    let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();

    let mut first_bytes = [1, 2, 3];
    let first = state.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(10),
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut first_bytes,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );
    let mut second_bytes = [4];
    let second = state.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(20),
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut second_bytes,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );

    assert_eq!(first, IngestPacketOutcome::Ignored(IgnoreReason::Malformed));
    assert_eq!(
        second,
        IngestPacketOutcome::Ignored(IgnoreReason::Malformed)
    );
    assert_eq!(state.ingested_packet_count(), 2);
}

#[test]
fn ingest_processes_but_does_not_accept_non_announce_bytes() {
    let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
    let junk = InboundPacket {
        arrived_at: InstantMillis(1),
        source_interface: InterfaceId::new([0u8; 8]),
        bytes: &mut [0x00, 0x00, 0x01, 0x02, 0x03],
    };
    let out = state.ingest_packet_with(
        junk,
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(out, IngestPacketOutcome::Ignored(IgnoreReason::Malformed));
    assert_eq!(state.route_count(), 0);
}

#[test]
fn an_ifac_flagged_packet_is_dropped_on_an_open_interface() {
    let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    raw[0] |= 0x80;
    let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
    let out = state.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&transporting_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(out, IngestPacketOutcome::Ignored(IgnoreReason::IfacRefused));
    assert_eq!(state.route_count(), 0);
}
