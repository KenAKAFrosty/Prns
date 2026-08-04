use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use prns_config::{
    parse_and_plan, PlannedMedium, RNodeBleTarget, RNodeTcpTarget, RNodeTransportPlan,
};

mod support;

fn oracle_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("python/rnode_transport_oracle.py")
}

fn oracle(python: &std::ffi::OsStr, ports: &[&str]) -> serde_json::Value {
    let mut child = Command::new(python)
        .arg(oracle_script())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn RNS 1.4.2 RNode transport oracle");
    child
        .stdin
        .take()
        .expect("oracle stdin")
        .write_all(
            serde_json::to_string(ports)
                .expect("ports serialize")
                .as_bytes(),
        )
        .expect("write ports to oracle");
    let output = child.wait_with_output().expect("oracle runs");
    assert!(
        output.status.success(),
        "RNode transport oracle failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle emits JSON")
}

fn planned_transport(port: &str) -> Result<RNodeTransportPlan, prns_config::ConfigErrors> {
    let config = format!(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = {port}\n\
         frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
         codingrate = 5\n"
    );
    let plan = parse_and_plan(&config)?.value;
    let PlannedMedium::Rnode { transport, .. } = &plan.interfaces[0].medium else {
        panic!("RNode transport expected")
    };
    Ok(transport.clone())
}

fn planned_shape(port: &str) -> serde_json::Value {
    let Ok(transport) = planned_transport(port) else {
        return serde_json::json!({ "error": "PlanError" });
    };
    let shape = match transport {
        RNodeTransportPlan::Serial(device) => serde_json::json!({
            "serial_port": device.as_str(),
            "use_ble": false,
            "ble_name": null,
            "ble_addr": null,
            "use_tcp": false,
            "tcp_host": null,
        }),
        RNodeTransportPlan::Tcp(target) => {
            let host = match target {
                RNodeTcpTarget::Loopback => None,
                RNodeTcpTarget::Host(host) => Some(host.as_str().to_string()),
            };
            serde_json::json!({
                "serial_port": null,
                "use_ble": false,
                "ble_name": null,
                "ble_addr": null,
                "use_tcp": true,
                "tcp_host": host,
            })
        }
        RNodeTransportPlan::Ble(target) => {
            let (name, address) = match target {
                RNodeBleTarget::FirstBondedRnode => (None, None),
                RNodeBleTarget::Name(name) => (Some(name.as_str().to_string()), None),
                RNodeBleTarget::Address(address) => (None, Some(address.to_string())),
            };
            serde_json::json!({
                "serial_port": null,
                "use_ble": true,
                "ble_name": name,
                "ble_addr": address,
                "use_tcp": false,
                "tcp_host": null,
            })
        }
    };
    serde_json::json!({ "ok": shape })
}

#[test]
fn transport_selection_matches_rns_1_4_2() {
    let python = support::required_python("RPC_SMOKE_PYTHON");
    let ports = [
        "/dev/ttyUSB0",
        "tcp://",
        "tcp://radio.example",
        "ble://",
        "ble://RNode 1234",
        "ble://AA:BB:CC:DD:EE:FF",
        "BLE://aa:bb:cc:dd:ee:ff",
        "tcp:/radio.example",
        "ble:/RNode 1234",
        "tcp:///radio/socket",
        "ble://AA:BB:CC:DD:EE",
        "ble://AA:BB:CC:DD:EE:FF:00",
        "ble://RNode 🛰️",
    ];
    let reference = oracle(&python, &ports);
    assert_eq!(reference["version"], "1.4.2");
    let reference_results = reference["results"]
        .as_array()
        .expect("oracle results are an array");
    let ours: Vec<_> = ports.iter().map(|port| planned_shape(port)).collect();
    assert_eq!(ours, *reference_results);
}
