mod arguments;
mod implementation;
mod process;
mod results;
mod suite;

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use benchmarks::scenario_dir;
use benchmarks::{
    energy_unavailable_hint, load_host, load_or_create_submitter_id, write_rows, Axis, DeviceId,
    PowerMeter, ResultRow, Subject, SubmitterId, RESULT_SCHEMA_VERSION,
};

use arguments::{parse_args, Args, RunnerCommand};
use implementation::{implementation, Implementation};
use process::{await_line, spawn_role};
use results::{file_results, CollectedRun};

fn main() {
    match parse_args() {
        RunnerCommand::Run(args) => run(args),
        RunnerCommand::Suite(args) => suite::run(args),
    }
}

fn run(args: Args) {
    if cfg!(debug_assertions) && !args.smoke {
        eprintln!("FAIL release-build-required: run target/release/benchmark_runner or pass --smoke for a non-publishing check");
        std::process::exit(2);
    }
    run_direct(&args);
}

fn run_direct(args: &Args) {
    let manifest = scenario_dir(&args.scenario).join("manifest.json");
    assert!(manifest.exists(), "no manifest at {}", manifest.display());
    let manifest_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).expect("reads the manifest"))
            .expect("parses the manifest");
    run_interop(args, &manifest_json, &manifest);
}

fn run_interop(args: &Args, manifest_json: &serde_json::Value, manifest: &std::path::Path) {
    let version = manifest_json["version"].as_u64().unwrap_or(1) as u32;

    let initiator_impl = implementation(&args.initiator);
    let responder_impl = implementation(&args.responder);
    let subject = Subject::Direct {
        initiator: initiator_impl.slug().to_string(),
        responder: responder_impl.slug().to_string(),
        relay: None,
    };
    let pairing_label = format!(
        "{} \u{2192} {}",
        initiator_impl.label(),
        responder_impl.label()
    );

    let interop_command = |subject: &Implementation| {
        subject
            .interop_command()
            .unwrap_or_else(|| panic!("implementation {:?} has no participant", subject.name()))
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
        "127.0.0.1:0",
        args,
    );
    let ready = await_line(&responder, "READY", Duration::from_secs(10));
    let addr = ready
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("addr="))
        .expect("responder READY carries addr")
        .to_string();

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
    let energy = bracket.map(|b| b.finish());

    let initiator_metrics = initiator.finalize();
    let responder_metrics = responder.finalize();

    file_results(
        args,
        version,
        subject,
        &pairing_label,
        CollectedRun {
            result: &result,
            responder_result: &responder_result,
            wire_line: None,
            energy,
            idle_watts,
            initiator: initiator_metrics,
            responder: responder_metrics,
            relay: None,
        },
    );
}
