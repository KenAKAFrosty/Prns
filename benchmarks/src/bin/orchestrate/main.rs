mod arguments;
mod implementation;
mod process;
mod relay;
mod results;

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use benchmarks::scenario_dir;
use benchmarks::{
    energy_unavailable_hint, load_host, load_or_create_submitter_id, write_rows, Axis, DeviceId,
    PowerMeter, ResultRow, SubmitterId,
};

use arguments::{parse_args, Args};
use implementation::{implementation, unsupported_pairing, Implementation};
use process::{await_line, spawn_role};
use relay::run_relay_interop;
use results::{file_results, CollectedRun};

fn udp_port_pair() -> (u16, u16) {
    let first = std::net::UdpSocket::bind("127.0.0.1:0").expect("probes a udp port");
    let second = std::net::UdpSocket::bind("127.0.0.1:0").expect("probes a udp port");
    (
        first.local_addr().expect("bound").port(),
        second.local_addr().expect("bound").port(),
    )
}

fn main() {
    let args = parse_args();
    let manifest = scenario_dir(&args.scenario).join("manifest.json");
    assert!(manifest.exists(), "no manifest at {}", manifest.display());
    let manifest_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).expect("reads the manifest"))
            .expect("parses the manifest");
    if manifest_json["profile"]["topology"].as_str() == Some("relay") {
        run_relay_interop(&args, &manifest_json, &manifest);
    } else {
        run_interop(&args, &manifest_json, &manifest);
    }
}

fn run_interop(args: &Args, manifest_json: &serde_json::Value, manifest: &std::path::Path) {
    let wire = manifest_json["profile"]["wire"].as_str().unwrap_or("tcp");
    let version = manifest_json["version"].as_u64().unwrap_or(1) as u32;

    let initiator_impl = implementation(&args.initiator);
    let responder_impl = implementation(&args.responder);
    let pairing_slug = format!("{}--{}", initiator_impl.slug, responder_impl.slug);
    let pairing_label = format!("{} \u{2192} {}", initiator_impl.label, responder_impl.label);

    let mechanism = manifest_json["profile"]["mechanism"]
        .as_str()
        .unwrap_or("single");
    if let Some(reason) = unsupported_pairing(&initiator_impl, &responder_impl, mechanism) {
        println!(
            "SKIP scenario={} pairing={pairing_label} reason={reason}",
            args.scenario
        );
        return;
    }
    let interop_command = |subject: &Implementation| {
        subject
            .interop_command()
            .unwrap_or_else(|| panic!("implementation {:?} fields no interop node", subject.name))
    };

    let (responder_addr, initiator_addr) = if wire == "udp" {
        let (responder_port, initiator_port) = udp_port_pair();
        (
            format!("127.0.0.1:{responder_port}>127.0.0.1:{initiator_port}"),
            Some(format!(
                "127.0.0.1:{initiator_port}>127.0.0.1:{responder_port}"
            )),
        )
    } else {
        ("127.0.0.1:0".to_string(), None)
    };

    let meter = PowerMeter::detect();
    if meter.is_none() {
        println!("{}", energy_unavailable_hint());
    }
    let idle_watts = meter
        .as_ref()
        .map(|m| m.idle_watts(Duration::from_millis(1500)));
    let bracket = meter.as_ref().map(|m| m.start());

    let responder = spawn_role(
        interop_command(&responder_impl),
        manifest,
        "responder",
        &responder_addr,
        args,
    );
    let ready = await_line(&responder, "READY", Duration::from_secs(10));
    let addr = initiator_addr.unwrap_or_else(|| {
        ready
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix("addr="))
            .expect("responder READY carries addr")
            .to_string()
    });

    let (pipe, addr) = if manifest_json["profile"]["wire_shape"].is_object() {
        assert!(wire != "udp", "wire_shape shapes tcp scenarios only");
        let pipe_bin = std::env::current_exe()
            .expect("own path")
            .parent()
            .expect("bin dir")
            .join("shaped_pipe");
        let pipe = spawn_role(Command::new(pipe_bin), manifest, "pipe", &addr, args);
        let pipe_ready = await_line(&pipe, "READY", Duration::from_secs(10));
        let pipe_addr = pipe_ready
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix("addr="))
            .expect("pipe READY carries addr")
            .to_string();
        (Some(pipe), pipe_addr)
    } else {
        (None, addr)
    };

    let initiator = spawn_role(
        interop_command(&initiator_impl),
        manifest,
        "initiator",
        &addr,
        args,
    );
    let scenario_duration_ms = args
        .duration_ms
        .or_else(|| manifest_json["profile"]["duration_ms"].as_u64())
        .unwrap_or(10_000);
    let window = Duration::from_millis(scenario_duration_ms + 30_000);
    let result = await_line(&initiator, "RESULT", window);
    let responder_result = await_line(&responder, "RESULT", Duration::from_secs(10));
    let wire_line = pipe
        .as_ref()
        .map(|p| await_line(p, "WIRE", Duration::from_secs(15)));

    let energy = bracket.map(|b| b.finish());

    let initiator_metrics = initiator.finalize();
    let responder_metrics = responder.finalize();

    file_results(
        args,
        version,
        &pairing_slug,
        &pairing_label,
        CollectedRun {
            result: &result,
            responder_result: &responder_result,
            wire_line: wire_line.as_deref(),
            energy,
            idle_watts,
            initiator: initiator_metrics,
            responder: responder_metrics,
            relay: None,
        },
    );
}
