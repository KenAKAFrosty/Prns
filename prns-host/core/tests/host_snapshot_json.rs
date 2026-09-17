use std::collections::BTreeSet;

use prns_host::{
    serialize_host_snapshot_json, BackendInfo, BackendKind, Capability, DestinationHash,
    DestinationIdentitySnapshot, HostSnapshot, IdentityHash, InterfaceHealth, InterfaceId,
    InterfaceKind, InterfaceSnapshot, PersistenceFlushCause, PersistenceSnapshot, RouteSnapshot,
    RuntimeHealthSnapshot, SAFE_UINT_MAX,
};
use serde_json::Value;

const HOST_SCHEMA: &str = include_str!("../../schema/host-contract-v1.json");

fn complete_snapshot() -> HostSnapshot {
    HostSnapshot {
        revision: u64::MAX,
        backend: BackendInfo::new(
            BackendKind::Native,
            [Capability::TcpClient, Capability::Bluetooth],
            [
                InterfaceKind::TcpClient,
                InterfaceKind::AutomaticBluetoothLe,
            ],
        ),
        interfaces: vec![InterfaceSnapshot {
            interface_id: InterfaceId::new([0x11; 8]),
            name: Some("radio \"one\"".to_string()),
            kind: Some(InterfaceKind::AutomaticBluetoothLe),
            health: InterfaceHealth::Degraded,
            failure_detail: Some("line one\nline two".to_string()),
            rx_bytes: u64::MAX,
            tx_bytes: SAFE_UINT_MAX + 1,
            rx_bps: Some(SAFE_UINT_MAX),
            tx_bps: Some(42),
            route_count: u32::MAX,
            link_count: 3,
            transported_link_count: 2,
        }],
        routes: vec![RouteSnapshot {
            destination: DestinationHash::new([0x22; 16]),
            hops: u8::MAX,
            via_identity: Some(IdentityHash::new([0x33; 16])),
            interface_id: InterfaceId::new([0x11; 8]),
            learned_at_millis: SAFE_UINT_MAX,
            last_route_activity_at_millis: 7,
            expires_at_millis: 8,
        }],
        active_link_count: 4,
        destination_identities: vec![DestinationIdentitySnapshot {
            destination: DestinationHash::new([0x22; 16]),
            identity: IdentityHash::new([0x44; 16]),
        }],
        runtime: RuntimeHealthSnapshot {
            running: true,
            uptime_millis: SAFE_UINT_MAX,
            interface_count: 1,
            online_interface_count: 1,
            route_count: 1,
            link_count: 4,
            transported_link_count: 2,
            rx_bytes: u64::MAX,
            tx_bytes: SAFE_UINT_MAX + 1,
            rx_bps: SAFE_UINT_MAX,
            tx_bps: 42,
        },
        persistence: PersistenceSnapshot {
            persistent: true,
            restored: true,
            last_flush_cause: Some(PersistenceFlushCause::Startup),
            last_failure_detail: Some("none \"now\"".to_string()),
        },
    }
}

fn object_keys(value: &Value) -> Result<BTreeSet<String>, String> {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .ok_or_else(|| "expected a JSON object".to_string())
}

fn schema_record_fields(schema: &Value, name: &str) -> Result<BTreeSet<String>, String> {
    let records = schema["records"]
        .as_array()
        .ok_or_else(|| "schema records were not an array".to_string())?;
    let record = records
        .iter()
        .find(|record| record["name"] == name)
        .ok_or_else(|| format!("missing schema record {name}"))?;
    let fields = record["fields"]
        .as_array()
        .ok_or_else(|| format!("schema record {name} fields were not an array"))?;
    fields
        .iter()
        .map(|field| {
            field["name"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("schema record {name} had an unnamed field"))
        })
        .collect()
}

fn assert_record_fields(schema: &Value, name: &str, value: &Value) -> Result<(), String> {
    assert_eq!(object_keys(value)?, schema_record_fields(schema, name)?);
    Ok(())
}

#[test]
fn complete_snapshot_json_matches_the_host_contract() -> Result<(), String> {
    let json =
        serialize_host_snapshot_json(&complete_snapshot()).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&json).map_err(|error| error.to_string())?;
    let schema: Value = serde_json::from_str(HOST_SCHEMA).map_err(|error| error.to_string())?;

    assert_record_fields(&schema, "HostSnapshot", &value)?;
    assert_record_fields(&schema, "BackendInfo", &value["backend"])?;
    assert_record_fields(&schema, "InterfaceSnapshot", &value["interfaces"][0])?;
    assert_record_fields(&schema, "RouteSnapshot", &value["routes"][0])?;
    assert_record_fields(
        &schema,
        "DestinationIdentitySnapshot",
        &value["destinationIdentities"][0],
    )?;
    assert_record_fields(&schema, "RuntimeHealthSnapshot", &value["runtime"])?;
    assert_record_fields(&schema, "PersistenceSnapshot", &value["persistence"])?;

    assert_eq!(value["revision"], u64::MAX.to_string());
    assert_eq!(value["interfaces"][0]["rxBytes"], u64::MAX.to_string());
    assert_eq!(
        value["interfaces"][0]["txBytes"],
        (SAFE_UINT_MAX + 1).to_string()
    );
    assert_eq!(value["runtime"]["rxBytes"], u64::MAX.to_string());
    assert_eq!(value["runtime"]["txBytes"], (SAFE_UINT_MAX + 1).to_string());

    assert_eq!(value["interfaces"][0]["rxBps"], SAFE_UINT_MAX);
    assert_eq!(value["routes"][0]["learnedAtMillis"], SAFE_UINT_MAX);
    assert_eq!(value["runtime"]["uptimeMillis"], SAFE_UINT_MAX);
    assert_eq!(value["activeLinkCount"], 4);
    assert_eq!(value["interfaces"][0]["routeCount"], u32::MAX);

    assert_eq!(value["backend"]["backend"], "Native");
    assert_eq!(
        value["backend"]["capabilities"],
        serde_json::json!(["TcpClient", "Bluetooth"])
    );
    assert_eq!(value["interfaces"][0]["kind"], "AutomaticBluetoothLe");
    assert_eq!(value["interfaces"][0]["health"], "Degraded");
    assert_eq!(value["persistence"]["lastFlushCause"], "Startup");

    assert_eq!(
        value["interfaces"][0]["interfaceId"],
        Value::Array(vec![Value::from(0x11); 8])
    );
    assert_eq!(
        value["routes"][0]["destination"],
        Value::Array(vec![Value::from(0x22); 16])
    );
    assert_eq!(
        value["routes"][0]["viaIdentity"],
        Value::Array(vec![Value::from(0x33); 16])
    );
    assert_eq!(
        value["destinationIdentities"][0]["identity"],
        Value::Array(vec![Value::from(0x44); 16])
    );
    assert_eq!(value["interfaces"][0]["name"], "radio \"one\"");
    assert_eq!(
        value["interfaces"][0]["failureDetail"],
        "line one\nline two"
    );
    Ok(())
}

#[test]
fn absent_optional_snapshot_fields_are_omitted() -> Result<(), String> {
    let mut snapshot = complete_snapshot();
    snapshot.interfaces[0].name = None;
    snapshot.interfaces[0].kind = None;
    snapshot.interfaces[0].failure_detail = None;
    snapshot.interfaces[0].rx_bps = None;
    snapshot.interfaces[0].tx_bps = None;
    snapshot.routes[0].via_identity = None;
    snapshot.persistence.last_flush_cause = None;
    snapshot.persistence.last_failure_detail = None;

    let json = serialize_host_snapshot_json(&snapshot).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&json).map_err(|error| error.to_string())?;

    for field in ["name", "kind", "failureDetail", "rxBps", "txBps"] {
        assert!(value["interfaces"][0].get(field).is_none());
    }
    assert!(value["routes"][0].get("viaIdentity").is_none());
    assert!(value["persistence"].get("lastFlushCause").is_none());
    assert!(value["persistence"].get("lastFailureDetail").is_none());
    Ok(())
}

#[test]
fn out_of_range_safe_uint_is_rejected_with_its_contract_path() -> Result<(), String> {
    let mut snapshot = complete_snapshot();
    snapshot.runtime.uptime_millis = SAFE_UINT_MAX + 1;

    let error = match serialize_host_snapshot_json(&snapshot) {
        Ok(_) => return Err("out-of-range safeUint unexpectedly serialized".to_string()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("runtime.uptimeMillis"));
    assert!(error.to_string().contains(&SAFE_UINT_MAX.to_string()));
    Ok(())
}
