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

use benchmarks::{
    energy_unavailable_hint, load_host, load_manifest, load_or_create_submitter_id, scenario_dir,
    write_rows, Axis, ConformanceRule, DeviceId, PowerMeter, ResultRow, ScenarioManifest, Subject,
    SubmitterId, RESULT_SCHEMA_VERSION,
};

use arguments::{parse_args, Args, RunnerCommand};
use implementation::{implementation, Implementation};
use process::{await_line, spawn_role};
use results::{file_results, CollectedRun};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasurementPhase {
    Startup,
    Linked,
    Measuring,
    Draining,
    Complete,
}

struct PhaseTracker(MeasurementPhase);

impl PhaseTracker {
    fn new() -> Self {
        Self(MeasurementPhase::Startup)
    }

    fn advance(&mut self, next: MeasurementPhase) -> Result<(), String> {
        let valid = matches!(
            (self.0, next),
            (MeasurementPhase::Startup, MeasurementPhase::Linked)
                | (MeasurementPhase::Linked, MeasurementPhase::Measuring)
                | (MeasurementPhase::Measuring, MeasurementPhase::Draining)
                | (MeasurementPhase::Draining, MeasurementPhase::Complete)
        );
        if !valid {
            return Err(format!(
                "invalid measurement phase transition {:?} -> {next:?}",
                self.0
            ));
        }
        self.0 = next;
        Ok(())
    }
}

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
    let manifest = scenario_dir(args.scenario.as_str()).join("manifest.json");
    assert!(manifest.exists(), "no manifest at {}", manifest.display());
    let manifest_data = load_manifest(args.scenario).expect("validated scenario manifest");
    run_interop(args, &manifest_data, &manifest);
}

fn run_interop(args: &Args, manifest_data: &ScenarioManifest, manifest: &std::path::Path) {
    let mut phase = PhaseTracker::new();
    let version = manifest_data.version;

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
        if std::env::var_os("BENCHMARK_REQUIRE_ENERGY").as_deref()
            == Some(std::ffi::OsStr::new("1"))
        {
            eprintln!("FAIL energy-required: no usable platform energy meter was detected");
            std::process::exit(2);
        }
    }
    let idle_watts = meter
        .as_ref()
        .map(|m| m.idle_watts(Duration::from_millis(1500)));
    let mut responder = spawn_role(
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

    let mut initiator = spawn_role(
        interop_command(&initiator_impl),
        manifest,
        "initiator",
        &addr,
        args,
    );
    await_line(&initiator, "MEASURE_READY", Duration::from_secs(30));
    phase
        .advance(MeasurementPhase::Linked)
        .expect("participants reached the measurement barrier");
    initiator.mark_measurement_start();
    responder.mark_measurement_start();
    let bracket = meter.as_ref().map(|meter| meter.start());
    phase
        .advance(MeasurementPhase::Measuring)
        .expect("measurement starts only after link establishment");
    initiator.start_measurement();

    let scenario_duration_ms = args
        .duration_ms
        .unwrap_or(manifest_data.profile.duration_ms);
    let drain_timeout_ms = manifest_data.profile.drain_timeout_ms;
    let window = Duration::from_millis(scenario_duration_ms + drain_timeout_ms + 30_000);
    await_line(&initiator, "MEASURE_DONE", window);
    initiator.mark_measurement_end();
    responder.mark_measurement_end();
    let energy = bracket.map(|bracket| bracket.finish());
    phase
        .advance(MeasurementPhase::Draining)
        .expect("initiator stopped issuing and drained every outstanding operation");
    let result = await_line(&initiator, "RESULT", Duration::from_secs(30));
    let resource_collection = manifest_data.conformance_rule == ConformanceRule::ExactResource;
    if resource_collection {
        responder.set_collection_target(
            result_metric(&result, "settled"),
            result_metric(&result, "payload_bytes"),
        );
    }
    let responder_result = await_line(
        &responder,
        "RESULT",
        if resource_collection {
            Duration::from_millis(drain_timeout_ms + 10_000)
        } else {
            Duration::from_secs(10)
        },
    );
    if resource_collection {
        initiator.release_collection();
    }
    phase
        .advance(MeasurementPhase::Complete)
        .expect("both roles reported complete results");
    let initiator_metrics = initiator.finalize();
    let responder_metrics = responder.finalize();

    let conformant = file_results(
        args,
        version,
        manifest_data.conformance_rule,
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
    if !conformant {
        std::process::exit(2);
    }
}

fn result_metric(line: &str, key: &str) -> u64 {
    let prefix = format!("{key}=");
    line.split_whitespace()
        .find_map(|field| field.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("RESULT is missing {key}: {line}"))
        .parse::<u64>()
        .unwrap_or_else(|error| panic!("RESULT has invalid {key}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{result_metric, MeasurementPhase, PhaseTracker};

    #[test]
    fn measurement_barrier_has_one_valid_phase_order() {
        let mut phases = PhaseTracker::new();
        for next in [
            MeasurementPhase::Linked,
            MeasurementPhase::Measuring,
            MeasurementPhase::Draining,
            MeasurementPhase::Complete,
        ] {
            phases.advance(next).expect("valid phase");
        }
        assert_eq!(phases.0, MeasurementPhase::Complete);
        assert!(phases.advance(MeasurementPhase::Measuring).is_err());
    }

    #[test]
    fn collection_targets_are_taken_from_typed_result_fields() {
        let result = "RESULT sent=4 settled=4 payload_bytes=268435456 failures=0";
        assert_eq!(result_metric(result, "settled"), 4);
        assert_eq!(result_metric(result, "payload_bytes"), 268_435_456);
    }
}
