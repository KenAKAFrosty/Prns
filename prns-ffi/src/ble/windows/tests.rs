use prns_core::interfaces::bluetooth_auto::core::{
    fragments_of, BleAddress, BleUuid, BLE_SERVICE_UUID, FRAGMENT_HEADER_LEN, NATIVE_CONTROL_UUID,
    NATIVE_DATA_UUID,
};
use windows::core::GUID;

use super::data_plane::{FRAGMENT_SCRATCH, GATT_FRAGMENT_PAYLOAD};
use super::{address_to_u64, guid_of};

#[test]
fn service_uuid_maps_to_canonical_guid() {
    let expected = GUID::from_values(
        0x37145b00,
        0x442d,
        0x4a94,
        [0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda, 0x28, 0xe3],
    );
    assert_eq!(guid_of(BLE_SERVICE_UUID), expected);
}

#[test]
fn control_and_data_uuids_match_the_spec() {
    let control = GUID::from_values(
        0x37145b00,
        0x442d,
        0x4a94,
        [0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda, 0x28, 0xe7],
    );
    let data = GUID::from_values(
        0x37145b00,
        0x442d,
        0x4a94,
        [0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda, 0x28, 0xe8],
    );
    assert_eq!(guid_of(NATIVE_CONTROL_UUID), control);
    assert_eq!(guid_of(NATIVE_DATA_UUID), data);
}

#[test]
fn bit16_uuid_expands_through_the_bluetooth_base() {
    let expected = GUID::from_values(
        0x0000_180f, // battery service, as an example short UUID
        0x0000,
        0x1000,
        [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
    );
    assert_eq!(guid_of(BleUuid::Bit16(0x180f)), expected);
}

#[test]
fn winrt_address_round_trips_through_sighting_octets() {
    for winrt_addr in [
        0x0000_5998_43cb_137c_u64,
        0x0000_ffff_ffff_ffff,
        0x0000_0000_0000_0001,
        0x0000_1234_5678_9abc,
    ] {
        let bytes = winrt_addr.to_be_bytes();
        let octets = [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]];
        assert_eq!(address_to_u64(BleAddress::new(octets)), winrt_addr);
    }
}

#[test]
fn scratch_buffer_holds_a_max_payload_fragment() {
    const { assert!(FRAGMENT_SCRATCH >= FRAGMENT_HEADER_LEN) };
    let payload = [0xAB_u8; GATT_FRAGMENT_PAYLOAD * 3];
    let mut buf = [0u8; GATT_FRAGMENT_PAYLOAD + FRAGMENT_SCRATCH];
    let mut fragments = 0;
    for fragment in fragments_of(&payload, GATT_FRAGMENT_PAYLOAD) {
        let len = fragment
            .encode(&mut buf)
            .expect("a full-payload fragment fits the scratch buffer");
        assert!(len <= buf.len());
        fragments += 1;
    }
    assert!(fragments >= 3);
}
