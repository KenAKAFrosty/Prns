use alloc::vec;
use alloc::vec::Vec;

use proptest::prelude::*;
use rmpv::Value;

use crate::interfaces::rns_management::wire_names::{interface, transport};

use super::{
    RnsInterfaceMode, RnsInterfaceStatsDecodeError, RnsInterfaceStatsReport, RnsOptionalField,
    RnsRemoteInterfaceStatsReport, RnsStatsFieldPath,
};

fn encode(value: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, value).unwrap();
    bytes
}

fn field(name: &'static str, value: Value) -> (Value, Value) {
    (Value::from(name), value)
}

fn complete_interface() -> Value {
    Value::Map(vec![
        field(interface::NAME, Value::from("RNode 1")),
        field(interface::SHORT_NAME, Value::from("RNode")),
        field(interface::TYPE, Value::from("RNodeInterface")),
        field(interface::HASH, Value::Binary(vec![0x11; 8])),
        field(interface::PARENT_NAME, Value::Nil),
        field(interface::STATUS, Value::from(true)),
        field(interface::MODE, Value::from(7)),
        field(interface::CLIENTS, Value::from(2)),
        field(interface::RECEIVE_BYTES, Value::from(1_024)),
        field(interface::TRANSMIT_BYTES, Value::from(2_048)),
        field(interface::RECEIVE_SPEED, Value::from(12.5)),
        field(interface::TRANSMIT_SPEED, Value::from(25)),
        field(interface::BITRATE, Value::from(9_600.5)),
        field(interface::PEERS, Value::from(3)),
        field(interface::IFAC_SIGNATURE, Value::Binary(vec![0x22; 64])),
        field(interface::IFAC_SIZE, Value::from(8)),
        field(interface::IFAC_NETWORK_NAME, Value::from("field")),
        field(interface::ANNOUNCE_QUEUE, Value::from(4)),
        field(interface::HELD_ANNOUNCES, Value::Nil),
        field(interface::INCOMING_ANNOUNCE_FREQUENCY, Value::from(0.5)),
        field(interface::OUTGOING_ANNOUNCE_FREQUENCY, Value::from(1)),
        field(interface::BURST_ACTIVE, Value::from(false)),
        field(interface::I2P_CONNECTABLE, Value::from(true)),
        field(interface::I2P_B32, Value::from("example.b32.i2p")),
        field(interface::AIRTIME_SHORT, Value::from(1.25)),
        field(interface::NOISE_FLOOR, Value::from(-112)),
        field(interface::INTERFERENCE, Value::Nil),
        field(interface::BATTERY_PERCENT, Value::from(87.5)),
        field(interface::BATTERY_STATE, Value::from("charging")),
        field(interface::SWITCH_ID, Value::from("switch-a")),
        field(interface::ENDPOINT_ID, Value::from("endpoint-b")),
        field(interface::VIA_SWITCH_ID, Value::from("switch-c")),
        field("future_status_field", Value::Array(vec![Value::from(1)])),
    ])
}

fn complete_report() -> Value {
    Value::Map(vec![
        field(
            interface::INTERFACES,
            Value::Array(vec![complete_interface()]),
        ),
        field(interface::RECEIVE_BYTES, Value::from(1_024)),
        field(interface::TRANSMIT_BYTES, Value::from(2_048)),
        field(interface::RECEIVE_SPEED, Value::from(12.5)),
        field(interface::TRANSMIT_SPEED, Value::from(25)),
        field(interface::RESIDENT_SET_SIZE, Value::from(65_536)),
        field(transport::IDENTITY, Value::Binary(vec![0x33; 16])),
        field(transport::NETWORK_IDENTITY, Value::Nil),
        field(transport::UPTIME, Value::from(61.5)),
        field(transport::PROBE_RESPONDER, Value::Binary(vec![0x44; 16])),
        field("future_report_field", Value::Map(vec![])),
    ])
}

#[test]
fn decodes_complete_rns_1_3_8_status_shape() {
    let report = RnsInterfaceStatsReport::decode_message_pack(&encode(&complete_report())).unwrap();
    let status = &report.interfaces[0];

    assert_eq!(status.name, "RNode 1");
    assert_eq!(status.short_name.value().map(String::as_str), Some("RNode"));
    assert_eq!(status.mode, RnsInterfaceMode::Internal);
    assert!(status.online);
    assert_eq!(status.clients, RnsOptionalField::Value(2));
    assert_eq!(status.receive_bytes, 1_024);
    assert_eq!(status.transmit_speed_bps, 25.0);
    assert_eq!(status.bitrate_bps, RnsOptionalField::Value(9_600.5));
    assert_eq!(status.parent_name, RnsOptionalField::Null);
    assert_eq!(status.parent_hash, RnsOptionalField::Absent);
    assert_eq!(status.held_announces, RnsOptionalField::Null);
    assert_eq!(status.interference_dbm, RnsOptionalField::Null);
    assert_eq!(
        status.switch_id.value().map(String::as_str),
        Some("switch-a")
    );
    assert_eq!(report.receive_bytes, 1_024);
    assert_eq!(
        report.resident_set_size_bytes,
        RnsOptionalField::Value(65_536)
    );
    assert_eq!(
        report.transport_identity.value().unwrap().as_bytes(),
        &[0x33; 16]
    );
    assert_eq!(report.network_identity, RnsOptionalField::Null);
    assert_eq!(
        report.transport_uptime_seconds,
        RnsOptionalField::Value(61.5)
    );
    assert_eq!(
        report.probe_responder.value().unwrap().as_bytes(),
        &[0x44; 16]
    );
}

#[test]
fn reports_missing_required_interface_field_with_its_path() {
    let mut report = complete_report();
    let Value::Map(fields) = &mut report else {
        unreachable!();
    };
    let Value::Array(interfaces) = &mut fields[0].1 else {
        unreachable!();
    };
    let Value::Map(interface_fields) = &mut interfaces[0] else {
        unreachable!();
    };
    interface_fields.retain(|(key, _)| key.as_str() != Some(interface::NAME));

    assert_eq!(
        RnsInterfaceStatsReport::decode_message_pack(&encode(&report)),
        Err(RnsInterfaceStatsDecodeError::MissingField(
            RnsStatsFieldPath::interface(0, interface::NAME)
        ))
    );
}

#[test]
fn reports_duplicate_top_level_field_with_its_path() {
    let mut report = complete_report();
    let Value::Map(fields) = &mut report else {
        unreachable!();
    };
    fields.push(field(interface::RECEIVE_BYTES, Value::from(99)));

    assert_eq!(
        RnsInterfaceStatsReport::decode_message_pack(&encode(&report)),
        Err(RnsInterfaceStatsDecodeError::DuplicateField(
            RnsStatsFieldPath::report(interface::RECEIVE_BYTES)
        ))
    );
}

#[test]
fn reports_invalid_identity_hash_length() {
    let mut report = complete_report();
    let Value::Map(fields) = &mut report else {
        unreachable!();
    };
    let identity = fields
        .iter_mut()
        .find(|(key, _)| key.as_str() == Some(transport::IDENTITY))
        .unwrap();
    identity.1 = Value::Binary(vec![0x33; 15]);

    assert_eq!(
        RnsInterfaceStatsReport::decode_message_pack(&encode(&report)),
        Err(RnsInterfaceStatsDecodeError::InvalidHashLength {
            path: RnsStatsFieldPath::report(transport::IDENTITY),
            expected: 16,
            actual: 15,
        })
    );
}

#[test]
fn decodes_remote_status_outer_shape_and_optional_link_count() {
    let response = Value::Array(vec![complete_report(), Value::from(3)]);
    let decoded = RnsRemoteInterfaceStatsReport::decode_message_pack(&encode(&response)).unwrap();
    assert_eq!(decoded.status.interfaces[0].name, "RNode 1");
    assert_eq!(decoded.link_count, Some(3));

    let response = Value::Array(vec![complete_report()]);
    let decoded = RnsRemoteInterfaceStatsReport::decode_message_pack(&encode(&response)).unwrap();
    assert_eq!(decoded.link_count, None);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2_048)) {
        let _ = RnsInterfaceStatsReport::decode_message_pack(&bytes);
    }
}
