use personal_rns::interfaces::rns_management::wire_names::{interface, transport};
use personal_rns::interfaces::rns_management::{
    RnsFleetPeerReport, RnsInterfaceStatsReport, RnsInterfaceStatusReport, RnsOptionalField,
};
use serde_json::{Map, Value};

pub fn render(report: &RnsInterfaceStatsReport) -> Result<String, serde_json::Error> {
    serde_json::to_string(&report_value(report))
}

fn report_value(report: &RnsInterfaceStatsReport) -> Value {
    let mut fields = Map::new();
    fields.insert(
        String::from(interface::INTERFACES),
        Value::Array(report.interfaces.iter().map(interface_value).collect()),
    );
    fields.insert(
        String::from(interface::RECEIVE_BYTES),
        report.receive_bytes.into(),
    );
    fields.insert(
        String::from(interface::TRANSMIT_BYTES),
        report.transmit_bytes.into(),
    );
    fields.insert(
        String::from(interface::RECEIVE_SPEED),
        number(report.receive_speed_bps),
    );
    fields.insert(
        String::from(interface::TRANSMIT_SPEED),
        number(report.transmit_speed_bps),
    );
    insert_optional(
        &mut fields,
        interface::RESIDENT_SET_SIZE,
        &report.resident_set_size_bytes,
        |value| (*value).into(),
    );
    insert_optional(
        &mut fields,
        transport::IDENTITY,
        &report.transport_identity,
        |value| Value::String(hex(value.as_bytes())),
    );
    insert_optional(
        &mut fields,
        transport::NETWORK_IDENTITY,
        &report.network_identity,
        |value| Value::String(hex(value.as_bytes())),
    );
    insert_optional(
        &mut fields,
        transport::UPTIME,
        &report.transport_uptime_seconds,
        |value| number(*value),
    );
    insert_optional(
        &mut fields,
        transport::PROBE_RESPONDER,
        &report.probe_responder,
        |value| Value::String(hex(value.as_bytes())),
    );
    Value::Object(fields)
}

fn interface_value(status: &RnsInterfaceStatusReport) -> Value {
    let mut fields = Map::new();
    fields.insert(
        String::from(interface::NAME),
        Value::String(status.name.clone()),
    );
    insert_optional_string(&mut fields, interface::SHORT_NAME, &status.short_name);
    insert_optional_string(&mut fields, interface::TYPE, &status.interface_type);
    insert_optional(
        &mut fields,
        interface::HASH,
        &status.interface_hash,
        |value| Value::String(hex(value)),
    );
    insert_optional_string(&mut fields, interface::PARENT_NAME, &status.parent_name);
    insert_optional(
        &mut fields,
        interface::PARENT_HASH,
        &status.parent_hash,
        |value| Value::String(hex(value)),
    );
    fields.insert(String::from(interface::STATUS), status.online.into());
    fields.insert(
        String::from(interface::MODE),
        status.mode.wire_value().into(),
    );
    insert_optional_i64(&mut fields, interface::GRAVITY, &status.gravity);
    insert_optional_u64(&mut fields, interface::CLIENTS, &status.clients);
    fields.insert(
        String::from(interface::RECEIVE_BYTES),
        status.receive_bytes.into(),
    );
    fields.insert(
        String::from(interface::TRANSMIT_BYTES),
        status.transmit_bytes.into(),
    );
    fields.insert(
        String::from(interface::RECEIVE_SPEED),
        number(status.receive_speed_bps),
    );
    fields.insert(
        String::from(interface::TRANSMIT_SPEED),
        number(status.transmit_speed_bps),
    );
    insert_optional_f64(&mut fields, interface::BITRATE, &status.bitrate_bps);
    insert_optional_u64(&mut fields, interface::PEERS, &status.peers);
    insert_optional(
        &mut fields,
        interface::IFAC_SIGNATURE,
        &status.ifac_signature,
        |value| Value::String(hex(value)),
    );
    insert_optional_u64(&mut fields, interface::IFAC_SIZE, &status.ifac_size_bytes);
    insert_optional_string(
        &mut fields,
        interface::IFAC_NETWORK_NAME,
        &status.ifac_network_name,
    );
    insert_optional_string(
        &mut fields,
        interface::AUTOCONNECT_SOURCE,
        &status.autoconnect_source,
    );
    insert_optional_u64(
        &mut fields,
        interface::ANNOUNCE_QUEUE,
        &status.announce_queue,
    );
    insert_optional_u64(
        &mut fields,
        interface::HELD_ANNOUNCES,
        &status.held_announces,
    );
    insert_optional_f64(
        &mut fields,
        interface::INCOMING_ANNOUNCE_FREQUENCY,
        &status.incoming_announce_frequency,
    );
    insert_optional_f64(
        &mut fields,
        interface::OUTGOING_ANNOUNCE_FREQUENCY,
        &status.outgoing_announce_frequency,
    );
    insert_optional_f64(
        &mut fields,
        interface::INCOMING_PATH_REQUEST_FREQUENCY,
        &status.incoming_path_request_frequency,
    );
    insert_optional_f64(
        &mut fields,
        interface::OUTGOING_PATH_REQUEST_FREQUENCY,
        &status.outgoing_path_request_frequency,
    );
    insert_optional_f64(
        &mut fields,
        interface::ANNOUNCE_RATE_TARGET,
        &status.announce_rate_target_seconds,
    );
    insert_optional_f64(
        &mut fields,
        interface::ANNOUNCE_RATE_PENALTY,
        &status.announce_rate_penalty_seconds,
    );
    insert_optional_f64(
        &mut fields,
        interface::ANNOUNCE_RATE_GRACE,
        &status.announce_rate_grace,
    );
    insert_optional_bool(&mut fields, interface::BURST_ACTIVE, &status.burst_active);
    insert_optional_f64(
        &mut fields,
        interface::BURST_ACTIVATED,
        &status.burst_activated_at,
    );
    insert_optional_bool(
        &mut fields,
        interface::PATH_REQUEST_BURST_ACTIVE,
        &status.path_request_burst_active,
    );
    insert_optional_f64(
        &mut fields,
        interface::PATH_REQUEST_BURST_ACTIVATED,
        &status.path_request_burst_activated_at,
    );
    insert_optional_bool(
        &mut fields,
        interface::I2P_CONNECTABLE,
        &status.i2p_connectable,
    );
    insert_optional_string(&mut fields, interface::I2P_B32, &status.i2p_b32);
    insert_optional_string(
        &mut fields,
        interface::I2P_TUNNEL_STATE,
        &status.i2p_tunnel_state,
    );
    insert_optional_f64(
        &mut fields,
        interface::AIRTIME_SHORT,
        &status.airtime_short_percent,
    );
    insert_optional_f64(
        &mut fields,
        interface::AIRTIME_LONG,
        &status.airtime_long_percent,
    );
    insert_optional_f64(
        &mut fields,
        interface::CHANNEL_LOAD_SHORT,
        &status.channel_load_short_percent,
    );
    insert_optional_f64(
        &mut fields,
        interface::CHANNEL_LOAD_LONG,
        &status.channel_load_long_percent,
    );
    insert_optional_f64(&mut fields, interface::NOISE_FLOOR, &status.noise_floor_dbm);
    insert_optional_f64(
        &mut fields,
        interface::INTERFERENCE,
        &status.interference_dbm,
    );
    insert_optional_f64(
        &mut fields,
        interface::INTERFERENCE_LAST_AT,
        &status.interference_last_at,
    );
    insert_optional_f64(
        &mut fields,
        interface::INTERFERENCE_LAST_DBM,
        &status.interference_last_dbm,
    );
    insert_optional_f64(&mut fields, interface::CPU_LOAD, &status.cpu_load_percent);
    insert_optional_f64(
        &mut fields,
        interface::CPU_TEMPERATURE,
        &status.cpu_temperature_celsius,
    );
    insert_optional_f64(
        &mut fields,
        interface::MEMORY_LOAD,
        &status.memory_load_percent,
    );
    insert_optional_f64(
        &mut fields,
        interface::BATTERY_PERCENT,
        &status.battery_percent,
    );
    insert_optional_string(&mut fields, interface::BATTERY_STATE, &status.battery_state);
    insert_optional_string(&mut fields, interface::SWITCH_ID, &status.switch_id);
    insert_optional_string(&mut fields, interface::ENDPOINT_ID, &status.endpoint_id);
    insert_optional_string(&mut fields, interface::VIA_SWITCH_ID, &status.via_switch_id);
    insert_optional(
        &mut fields,
        interface::BLOCKED_IP_LIST,
        &status.blocked_ip_list,
        |values| Value::Array(values.iter().cloned().map(Value::String).collect()),
    );
    insert_optional_i64(&mut fields, interface::RSSI, &status.rssi);
    if !status.fleet_peers.is_empty() {
        fields.insert(
            String::from(interface::FLEET_PEERS),
            Value::Array(status.fleet_peers.iter().map(fleet_peer_value).collect()),
        );
    }
    Value::Object(fields)
}

fn fleet_peer_value(peer: &RnsFleetPeerReport) -> Value {
    let mut fields = Map::new();
    fields.insert(String::from(interface::NAME), Value::String(peer.name.clone()));
    fields.insert(String::from(interface::STATUS), peer.online.into());
    fields.insert(
        String::from(interface::RECEIVE_BYTES),
        peer.receive_bytes.into(),
    );
    fields.insert(
        String::from(interface::TRANSMIT_BYTES),
        peer.transmit_bytes.into(),
    );
    fields.insert(
        String::from(interface::RECEIVE_SPEED),
        number(peer.receive_speed_bps),
    );
    fields.insert(
        String::from(interface::TRANSMIT_SPEED),
        number(peer.transmit_speed_bps),
    );
    insert_optional_i64(&mut fields, interface::RSSI, &peer.rssi);
    Value::Object(fields)
}

fn insert_optional_string(
    fields: &mut Map<String, Value>,
    name: &'static str,
    field: &RnsOptionalField<String>,
) {
    insert_optional(fields, name, field, |value| Value::String(value.clone()));
}

fn insert_optional_u64(
    fields: &mut Map<String, Value>,
    name: &'static str,
    field: &RnsOptionalField<u64>,
) {
    insert_optional(fields, name, field, |value| (*value).into());
}

fn insert_optional_i64(
    fields: &mut Map<String, Value>,
    name: &'static str,
    field: &RnsOptionalField<i64>,
) {
    insert_optional(fields, name, field, |value| (*value).into());
}

fn insert_optional_f64(
    fields: &mut Map<String, Value>,
    name: &'static str,
    field: &RnsOptionalField<f64>,
) {
    insert_optional(fields, name, field, |value| number(*value));
}

fn insert_optional_bool(
    fields: &mut Map<String, Value>,
    name: &'static str,
    field: &RnsOptionalField<bool>,
) {
    insert_optional(fields, name, field, |value| (*value).into());
}

fn insert_optional<T>(
    fields: &mut Map<String, Value>,
    name: &'static str,
    field: &RnsOptionalField<T>,
    value: impl FnOnce(&T) -> Value,
) {
    match field {
        RnsOptionalField::Absent => {}
        RnsOptionalField::Null => {
            fields.insert(String::from(name), Value::Null);
        }
        RnsOptionalField::Value(field) => {
            fields.insert(String::from(name), value(field));
        }
    }
}

fn number(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use personal_rns::interfaces::rns_management::{RnsInterfaceMode, RnsOptionalField};

    use super::*;

    #[test]
    fn absent_fields_are_omitted_and_null_fields_remain_null() {
        let status = RnsInterfaceStatusReport {
            name: String::from("test"),
            short_name: RnsOptionalField::Absent,
            interface_type: RnsOptionalField::Null,
            interface_hash: RnsOptionalField::Absent,
            parent_name: RnsOptionalField::Absent,
            parent_hash: RnsOptionalField::Absent,
            online: true,
            mode: RnsInterfaceMode::Internal,
            gravity: RnsOptionalField::Value(-7),
            clients: RnsOptionalField::Null,
            receive_bytes: 1,
            transmit_bytes: 2,
            receive_speed_bps: 3.0,
            transmit_speed_bps: 4.0,
            bitrate_bps: RnsOptionalField::Absent,
            peers: RnsOptionalField::Absent,
            ifac_signature: RnsOptionalField::Absent,
            ifac_size_bytes: RnsOptionalField::Absent,
            ifac_network_name: RnsOptionalField::Absent,
            autoconnect_source: RnsOptionalField::Absent,
            announce_queue: RnsOptionalField::Absent,
            held_announces: RnsOptionalField::Absent,
            incoming_announce_frequency: RnsOptionalField::Absent,
            outgoing_announce_frequency: RnsOptionalField::Absent,
            incoming_path_request_frequency: RnsOptionalField::Absent,
            outgoing_path_request_frequency: RnsOptionalField::Absent,
            announce_rate_target_seconds: RnsOptionalField::Absent,
            announce_rate_penalty_seconds: RnsOptionalField::Absent,
            announce_rate_grace: RnsOptionalField::Absent,
            burst_active: RnsOptionalField::Absent,
            burst_activated_at: RnsOptionalField::Absent,
            path_request_burst_active: RnsOptionalField::Absent,
            path_request_burst_activated_at: RnsOptionalField::Absent,
            i2p_connectable: RnsOptionalField::Absent,
            i2p_b32: RnsOptionalField::Absent,
            i2p_tunnel_state: RnsOptionalField::Absent,
            airtime_short_percent: RnsOptionalField::Absent,
            airtime_long_percent: RnsOptionalField::Absent,
            channel_load_short_percent: RnsOptionalField::Absent,
            channel_load_long_percent: RnsOptionalField::Absent,
            noise_floor_dbm: RnsOptionalField::Absent,
            interference_dbm: RnsOptionalField::Absent,
            interference_last_at: RnsOptionalField::Absent,
            interference_last_dbm: RnsOptionalField::Absent,
            cpu_load_percent: RnsOptionalField::Absent,
            cpu_temperature_celsius: RnsOptionalField::Absent,
            memory_load_percent: RnsOptionalField::Absent,
            battery_percent: RnsOptionalField::Absent,
            battery_state: RnsOptionalField::Absent,
            switch_id: RnsOptionalField::Absent,
            endpoint_id: RnsOptionalField::Absent,
            via_switch_id: RnsOptionalField::Absent,
            blocked_ip_list: RnsOptionalField::Value(vec![
                String::from("192.0.2.10"),
                String::from("2001:db8::1"),
            ]),
            rssi: RnsOptionalField::Absent,
            fleet_peers: vec![],
        };
        let Value::Object(fields) = interface_value(&status) else {
            unreachable!();
        };
        assert!(!fields.contains_key(interface::SHORT_NAME));
        assert_eq!(fields.get(interface::TYPE), Some(&Value::Null));
        assert_eq!(fields.get(interface::CLIENTS), Some(&Value::Null));
        assert_eq!(fields.get(interface::MODE), Some(&Value::from(7)));
        assert_eq!(fields.get(interface::GRAVITY), Some(&Value::from(-7)));
        assert_eq!(
            fields.get(interface::BLOCKED_IP_LIST),
            Some(&serde_json::json!(["192.0.2.10", "2001:db8::1"])),
        );
    }
}
