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
    write_rows, Axis, ConformanceRule, DeviceId, PowerMeter, ResultRow, ScenarioManifest,
    ScenarioTopology, Subject, SubmitterId, REFERENCE_IMPLEMENTATION, RESULT_SCHEMA_VERSION,
};
use personal_rns::interfaces::{hardware_mtu_for_bitrate, tcp};

use arguments::{parse_args, Args, RunnerCommand};
use implementation::{implementation, Implementation};
use process::{await_line, spawn_role};
use results::{file_results, CollectedRun};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasurementPhase {
    Startup,
    ParticipantsReady,
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
            (
                MeasurementPhase::Startup,
                MeasurementPhase::ParticipantsReady
            ) | (
                MeasurementPhase::ParticipantsReady,
                MeasurementPhase::Linked
            ) | (MeasurementPhase::Linked, MeasurementPhase::Measuring)
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
    run_scenario(&args);
}

fn run_scenario(args: &Args) {
    let manifest = scenario_dir(args.scenario.as_str()).join("manifest.json");
    assert!(manifest.exists(), "no manifest at {}", manifest.display());
    let manifest_data = load_manifest(args.scenario).expect("validated scenario manifest");
    match manifest_data.topology {
        ScenarioTopology::Direct => {
            assert!(
                args.relay.is_none(),
                "--relay is only valid for relay-topology scenarios"
            );
            run_interop(args, &manifest_data, &manifest);
        }
        ScenarioTopology::Relay => run_transport(args, &manifest_data, &manifest),
    }
}

fn raw_driver_command() -> Command {
    let mut path = std::env::current_exe().expect("current benchmark executable");
    path.set_file_name(format!(
        "raw_transport_driver{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        path.exists(),
        "raw transport driver missing at {}",
        path.display()
    );
    Command::new(path)
}

fn power_meter() -> (Option<PowerMeter>, Option<f64>) {
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
        .map(|meter| meter.idle_watts(Duration::from_millis(1500)));
    (meter, idle_watts)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RelayTcpPolicy {
    bitrate_bps: u64,
    mtu_bytes: usize,
}

fn relay_tcp_policy(line: &str) -> RelayTcpPolicy {
    let value = |key: &str| {
        line.split_whitespace()
            .find_map(|field| field.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("relay READY is missing {key}: {line}"))
    };
    RelayTcpPolicy {
        bitrate_bps: value("bitrate_bps")
            .parse()
            .unwrap_or_else(|error| panic!("relay READY has invalid bitrate: {error}")),
        mtu_bytes: value("mtu_bytes")
            .parse()
            .unwrap_or_else(|error| panic!("relay READY has invalid MTU: {error}")),
    }
}

fn expected_relay_tcp_policy(manifest: &ScenarioManifest, relay_slug: &str) -> RelayTcpPolicy {
    let bitrate_bps = manifest
        .profile
        .tcp_bitrate_bps
        .unwrap_or_else(|| match relay_slug {
            "personal-rns" => tcp::TCP_BITRATE_ESTIMATE.get(),
            REFERENCE_IMPLEMENTATION => 10_000_000,
            other => panic!("unknown relay implementation {other:?}"),
        });
    RelayTcpPolicy {
        bitrate_bps,
        mtu_bytes: hardware_mtu_for_bitrate(bitrate_bps)
            .expect("benchmark TCP bitrate selects an RNS MTU tier"),
    }
}

fn run_transport(args: &Args, manifest_data: &ScenarioManifest, manifest: &std::path::Path) {
    let relay_slug = args.relay.as_deref().unwrap_or("personal-rns");
    let relay_impl = implementation(relay_slug);
    let relay_command = relay_impl
        .interop_command()
        .unwrap_or_else(|| panic!("implementation {:?} has no participant", relay_impl.name()));
    let subject = Subject::Direct {
        initiator: "benchmark-wire-driver".into(),
        responder: "benchmark-wire-driver".into(),
        relay: Some(relay_impl.slug().into()),
    };
    let pairing_label = format!("{} relay", relay_impl.label());
    let (meter, idle_watts) = power_meter();

    let mut relay = spawn_role(relay_command, manifest, "relay", "127.0.0.1:0", args);
    let ready = await_line(&relay, "READY", Duration::from_secs(30));
    let relay_policy = relay_tcp_policy(&ready);
    let expected_policy = expected_relay_tcp_policy(manifest_data, relay_impl.slug());
    assert_eq!(
        relay_policy,
        expected_policy,
        "{} reported a TCP policy that does not match {}",
        relay_impl.label(),
        manifest_data.name
    );
    println!(
        "RELAY_POLICY bitrate_bps={} mtu_bytes={}",
        relay_policy.bitrate_bps, relay_policy.mtu_bytes
    );
    let addresses = ready
        .split_whitespace()
        .find_map(|field| field.strip_prefix("addr="))
        .expect("relay READY carries both addresses")
        .to_string();
    let mut driver_command = raw_driver_command();
    if args.smoke {
        driver_command.env("BENCHMARK_SMOKE", "1");
    }
    let mut driver = spawn_role(driver_command, manifest, "wire-driver", &addresses, args);
    await_line(&driver, "READY", Duration::from_secs(10));
    await_line(
        &driver,
        "HARNESS",
        Duration::from_secs(if args.smoke { 10 } else { 30 }),
    );
    await_line(&driver, "MEASURE_READY", Duration::from_secs(30));
    println!("STARTUP_ATTEMPT stage=relay-readiness attempt=1 result=pass");

    relay.mark_measurement_start();
    driver.mark_measurement_start();
    let bracket = meter.as_ref().map(|meter| meter.start());
    driver.start_measurement();
    let scenario_duration_ms = args
        .duration_ms
        .unwrap_or(manifest_data.profile.duration_ms);
    let window = Duration::from_millis(
        scenario_duration_ms + manifest_data.profile.drain_timeout_ms + 30_000,
    );
    await_line(&driver, "MEASURE_DONE", window);
    driver.mark_measurement_end();
    relay.mark_measurement_end();
    let energy = bracket.map(|bracket| bracket.finish());
    let driver_result = await_line(&driver, "RESULT", Duration::from_secs(10));
    let result = format!(
        "{driver_result} relay_bitrate_bps={} relay_mtu_bytes={}",
        relay_policy.bitrate_bps, relay_policy.mtu_bytes
    );
    let driver_metrics = driver.finalize();
    relay.stop();
    let relay_metrics = relay.finalize();

    let conformant = file_results(
        args,
        manifest_data.version,
        manifest_data.conformance_rule,
        subject,
        &pairing_label,
        CollectedRun {
            result: &result,
            responder_result: &result,
            wire_line: None,
            energy,
            idle_watts,
            initiator: driver_metrics,
            responder: process::RoleMetrics::default(),
            relay: Some(relay_metrics),
        },
    );
    if !conformant {
        std::process::exit(2);
    }
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

    let (meter, idle_watts) = power_meter();
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
    await_line(&initiator, "READY", Duration::from_secs(30));
    phase
        .advance(MeasurementPhase::ParticipantsReady)
        .expect("both participant processes initialized");
    println!("STARTUP_ATTEMPT stage=participant-readiness attempt=1 result=pass");
    responder.start_startup();
    await_line(&initiator, "MEASURE_READY", Duration::from_secs(30));
    await_line(&responder, "MEASURE_READY", Duration::from_secs(30));
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
    use super::{
        expected_relay_tcp_policy, relay_tcp_policy, result_metric, MeasurementPhase, PhaseTracker,
        RelayTcpPolicy,
    };
    use benchmarks::{load_manifest, ScenarioId, REFERENCE_IMPLEMENTATION};

    #[test]
    fn measurement_barrier_has_one_valid_phase_order() {
        let mut phases = PhaseTracker::new();
        for next in [
            MeasurementPhase::ParticipantsReady,
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

    #[test]
    fn relay_ready_policy_is_typed_and_manifest_checked() {
        assert_eq!(
            relay_tcp_policy(
                "READY role=relay addr=127.0.0.1:1>127.0.0.1:2 \
                 bitrate_bps=1000000000 mtu_bytes=524288"
            ),
            RelayTcpPolicy {
                bitrate_bps: 1_000_000_000,
                mtu_bytes: 524_288,
            }
        );
        let practical = load_manifest(ScenarioId::RawTransportThroughput).expect("manifest");
        assert_eq!(
            expected_relay_tcp_policy(&practical, "personal-rns"),
            RelayTcpPolicy {
                bitrate_bps: 500_000_000,
                mtu_bytes: 131_072,
            }
        );
        assert_eq!(
            expected_relay_tcp_policy(&practical, REFERENCE_IMPLEMENTATION),
            RelayTcpPolicy {
                bitrate_bps: 10_000_000,
                mtu_bytes: 8_192,
            }
        );
        let unleashed =
            load_manifest(ScenarioId::TransportResourceThroughputUnleashed).expect("manifest");
        assert_eq!(
            expected_relay_tcp_policy(&unleashed, REFERENCE_IMPLEMENTATION),
            RelayTcpPolicy {
                bitrate_bps: 1_000_000_000,
                mtu_bytes: 524_288,
            }
        );
    }
}
