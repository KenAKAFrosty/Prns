use super::process::RoleMetrics;
use super::*;
use personal_rns::wire::{HEADER_MIN_LEN, IFAC_MIN_LEN};
use std::collections::BTreeMap;

fn rustc_host_triple() -> String {
    command_line("rustc", &["-vV"])
        .and_then(|v| {
            v.lines()
                .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        })
        .unwrap_or_else(|| "unknown-host".into())
}
fn field(line: &str, key: &str) -> Option<f64> {
    line.split_whitespace()
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
}
fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn rate_metric(scenario: benchmarks::ScenarioId) -> &'static str {
    match scenario {
        benchmarks::ScenarioId::RequestResponse => "requests_per_sec",
        scenario if scenario.is_transport() => "forwarded_frames_per_sec",
        _ => "delivered_per_sec",
    }
}

pub(super) struct RunStamp {
    pub(super) host: String,
    pub(super) commit: String,
    pub(super) toolchain: String,
    pub(super) source_dirty: bool,
    pub(super) device_id: Option<DeviceId>,
    pub(super) submitter_id: Option<SubmitterId>,
}

pub(super) fn run_stamp() -> RunStamp {
    let host = rustc_host_triple();
    assert!(
        host != "unknown-host",
        "host triple unresolved — `rustc` is not on PATH (common under `sudo`, which resets it). \
         Re-run as `sudo env \"PATH=$PATH\" ...` so rows don't file under `unknown-host`.",
    );
    RunStamp {
        device_id: load_host(&host).and_then(|descriptor| descriptor.device_id),
        submitter_id: Some(load_or_create_submitter_id()),
        commit: command_line("git", &["rev-parse", "HEAD"]).unwrap_or_default(),
        toolchain: std::env::var("BENCHMARK_TOOLCHAIN")
            .ok()
            .or_else(|| command_line("rustc", &["--version"]))
            .unwrap_or_default(),
        source_dirty: command_line(
            "git",
            &["status", "--porcelain", "--untracked-files=normal"],
        )
        .is_some_and(|status| !status.is_empty()),
        host,
    }
}

pub(super) fn provenance_for(subject: &Subject) -> BTreeMap<String, String> {
    let mut provenance = BTreeMap::new();
    if let Ok(flags) = std::env::var("BENCHMARK_BUILD_FLAGS") {
        provenance.insert("rust_build_flags".into(), flags);
    }
    if let Ok(fingerprint) = std::env::var("BENCHMARK_SOURCE_FINGERPRINT") {
        provenance.insert("source_fingerprint".into(), fingerprint);
    }
    #[cfg(target_os = "macos")]
    provenance.insert(
        "energy_source".into(),
        "powermetrics cpu_power: CPU Power".into(),
    );
    #[cfg(target_os = "linux")]
    provenance.insert("energy_source".into(), "RAPL package-0 energy_uj".into());
    let uses_compiled_reference = match subject {
        Subject::Direct {
            initiator,
            responder,
            relay,
        } => {
            initiator == "rns-1.4.0-compiled"
                || responder == "rns-1.4.0-compiled"
                || relay.as_deref() == Some("rns-1.4.0-compiled")
        }
    };
    if !uses_compiled_reference {
        return provenance;
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("reference/.object-cache/proof.json");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("compiled-reference proof {}: {error}", path.display())
        }))
        .unwrap_or_else(|error| {
            panic!("parse compiled-reference proof {}: {error}", path.display())
        });
    provenance.extend(
        json.as_object()
            .expect("compiled-reference proof is an object")
            .iter()
            .map(|(key, value)| {
                let value = value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string());
                (format!("reference_{key}"), value)
            }),
    );
    provenance
}
pub(super) struct CollectedRun<'a> {
    pub(super) result: &'a str,
    pub(super) responder_result: &'a str,
    pub(super) wire_line: Option<&'a str>,
    pub(super) energy: Option<(f64, f64)>,
    pub(super) idle_watts: Option<f64>,
    pub(super) initiator: RoleMetrics,
    pub(super) responder: RoleMetrics,
    pub(super) relay: Option<RoleMetrics>,
}

#[derive(Clone, Copy)]
struct Conformance<'a> {
    rule: ConformanceRule,
    sent: f64,
    delivered: f64,
    timeouts: f64,
    raced: f64,
    culled: f64,
    responder_delivered: f64,
    result: &'a str,
    responder_result: &'a str,
    require_harness_headroom: bool,
}

fn scenario_conforms(input: &Conformance<'_>) -> bool {
    let &Conformance {
        rule,
        sent,
        delivered,
        timeouts,
        raced,
        culled,
        responder_delivered,
        result,
        responder_result,
        require_harness_headroom,
    } = input;
    let exact_delivery = sent > 0.0
        && sent == delivered
        && responder_delivered == sent
        && timeouts == 0.0
        && raced == 0.0;
    match rule {
        ConformanceRule::ExactSingle => exact_delivery && culled == 0.0,
        ConformanceRule::ExactLink => {
            let attempted = field(result, "attempted").unwrap_or(sent + culled);
            let sent_bytes = field(result, "payload_bytes").unwrap_or(f64::NAN);
            let received_bytes = field(responder_result, "payload_bytes").unwrap_or(f64::NAN);
            exact_delivery && attempted == sent + culled && sent_bytes == received_bytes
        }
        ConformanceRule::ExactRequest => {
            let expected = field(result, "expected_response_bytes").unwrap_or(f64::NAN);
            let received = field(result, "response_bytes").unwrap_or(f64::NAN);
            let served = field(responder_result, "response_bytes").unwrap_or(f64::NAN);
            let request_window = field(result, "request_window").unwrap_or(f64::NAN);
            let request_links = field(result, "request_links").unwrap_or(f64::NAN);
            exact_delivery
                && expected == received
                && received == served
                && request_window == 4.0
                && request_links == 4.0
        }
        ConformanceRule::ExactResource => {
            let sent_bytes = field(result, "payload_bytes").unwrap_or(f64::NAN);
            let received_bytes = field(responder_result, "payload_bytes").unwrap_or(f64::NAN);
            exact_delivery && sent_bytes == received_bytes
        }
        ConformanceRule::ExactTransport | ConformanceRule::ExactTransportResource => {
            let exact = |left, right| {
                field(result, left).unwrap_or(f64::NAN) == field(result, right).unwrap_or(f64::NAN)
            };
            let shape_is_exact = match rule {
                ConformanceRule::ExactTransport => field(result, "proofs") == Some(sent),
                ConformanceRule::ExactTransportResource => {
                    let negotiated_mtu =
                        field(result, "negotiated_link_mtu_bytes").unwrap_or(f64::NAN);
                    let payload =
                        field(result, "resource_payload_bytes_per_frame").unwrap_or(f64::NAN);
                    field(result, "proofs") == Some(0.0)
                        && negotiated_mtu > 0.0
                        && negotiated_mtu <= field(result, "relay_mtu_bytes").unwrap_or(f64::NAN)
                        && payload + (HEADER_MIN_LEN + IFAC_MIN_LEN) as f64 == negotiated_mtu
                }
                ConformanceRule::ExactSingle
                | ConformanceRule::ExactLink
                | ConformanceRule::ExactRequest
                | ConformanceRule::ExactResource => unreachable!("transport rules only"),
            };
            exact_delivery
                && shape_is_exact
                && field(result, "relay_bitrate_bps").is_some_and(|value| value > 0.0)
                && field(result, "relay_mtu_bytes").is_some_and(|value| value > 0.0)
                && field(result, "sent_a_to_b").is_some_and(|value| value > 0.0)
                && field(result, "sent_b_to_a").is_some_and(|value| value > 0.0)
                && exact("sent_a_to_b", "carried_a_to_b")
                && exact("sent_b_to_a", "carried_b_to_a")
                && exact("sent_payload_bytes", "carried_payload_bytes")
                && exact("sent_payload_bytes_a_to_b", "carried_payload_bytes_a_to_b")
                && exact("sent_payload_bytes_b_to_a", "carried_payload_bytes_b_to_a")
                && [
                    "duplicates",
                    "corrupt",
                    "reordered",
                    "unexpected",
                    "missing",
                    "timed_out_frames",
                    "drain_timeouts",
                    "outstanding",
                    "buffer_pool_misses",
                    "credit_leaks",
                ]
                .into_iter()
                .all(|metric| field(result, metric) == Some(0.0))
                && (!require_harness_headroom
                    || field(result, "harness_calibration_ms") == Some(2_000.0))
                && (!require_harness_headroom
                    || field(result, "harness_source_payload_bytes_per_sec")
                        .is_some_and(|value| value > 0.0))
                && (!require_harness_headroom
                    || field(result, "harness_sink_payload_bytes_per_sec")
                        .is_some_and(|value| value > 0.0))
                && (!require_harness_headroom || field(result, "harness_headroom") == Some(1.0))
        }
    }
}

pub(super) fn file_results(
    args: &Args,
    version: u32,
    conformance_rule: ConformanceRule,
    subject: Subject,
    pairing_label: &str,
    run: CollectedRun<'_>,
) -> bool {
    let result = run.result;
    let responder_result = run.responder_result;
    let wire_line = run.wire_line;
    let energy = run.energy;
    let idle_watts = run.idle_watts;
    let initiator_cpu = run.initiator.cpu_seconds;
    let initiator_rss = run.initiator.peak_rss_bytes;
    let responder_cpu = run.responder.cpu_seconds;
    let responder_rss = run.responder.peak_rss_bytes;
    let relay = run.relay;

    let sent = field(result, "sent")
        .or_else(|| field(result, "cycles"))
        .unwrap_or(0.0);
    let delivered = field(result, "delivered")
        .or_else(|| field(result, "settled"))
        .or_else(|| field(result, "cycles"))
        .or_else(|| field(result, "carried"))
        .unwrap_or(0.0);
    let timeouts = field(result, "timeouts")
        .or_else(|| field(result, "failures"))
        .or_else(|| field(result, "drain_timeouts"))
        .unwrap_or(f64::NAN);
    let raced = field(result, "raced").unwrap_or(0.0);
    let culled = field(result, "culled").unwrap_or(0.0);
    let responder_delivered = field(responder_result, "delivered")
        .or_else(|| field(responder_result, "received"))
        .or_else(|| field(responder_result, "served"))
        .or_else(|| field(responder_result, "carried"))
        .unwrap_or(0.0);
    let died = field(result, "died").unwrap_or(0.0) > 0.0;
    let settled_clean = !died
        && scenario_conforms(&Conformance {
            rule: conformance_rule,
            sent,
            delivered,
            timeouts,
            raced,
            culled,
            responder_delivered,
            result,
            responder_result,
            require_harness_headroom: !args.smoke,
        });
    if died {
        eprintln!(
            "verdict: the initiator declared the responder DEAD mid-run — conformance filed, \
             throughput/latency/energy withheld (a dead run's last gasp is not a measurement)"
        );
    }
    let perf_valid = !result.contains("build=debug") && !responder_result.contains("build=debug");
    if !perf_valid {
        eprintln!(
            "verdict: a participant is a DEBUG build (build=debug) — crypto ~10x slower; \
             conformance filed, throughput/latency/memory/energy withheld (debug perf is not a \
             measurement; rebuild --release)"
        );
    }
    if !settled_clean {
        if matches!(
            conformance_rule,
            ConformanceRule::ExactTransport | ConformanceRule::ExactTransportResource
        ) && !args.smoke
            && field(result, "harness_headroom") != Some(1.0)
        {
            eprintln!(
                "FAIL harness_headroom: direct driver calibration did not exceed relay carried-payload throughput by 25%"
            );
        }
        eprintln!(
            "verdict: strict scenario accounting failed: sent={sent} delivered={delivered} \
             responder={responder_delivered} timeouts={timeouts} raced={raced} culled={culled}"
        );
    }

    let stamp = run_stamp();
    let mut provenance = provenance_for(&subject);
    provenance.insert("source_dirty".into(), stamp.source_dirty.to_string());
    if let Ok(suite_id) = std::env::var("BENCHMARK_SUITE_ID") {
        provenance.insert("suite_id".into(), suite_id);
    }
    let row = |axis: Axis, metric: &str, value: Option<f64>, unit: &str| ResultRow {
        schema_version: RESULT_SCHEMA_VERSION,
        run_id: args.run_id.clone(),
        sample_index: args.sample_index,
        scenario: args.scenario.to_string(),
        scenario_version: version,
        subject: subject.clone(),
        commit: stamp.commit.clone(),
        toolchain: stamp.toolchain.clone(),
        host: stamp.host.clone(),
        axis,
        metric: metric.into(),
        value,
        unit: unit.into(),
        device_id: stamp.device_id,
        submitter_id: stamp.submitter_id,
        provenance: provenance.clone(),
    };
    let elapsed_seconds = field(result, "elapsed_ms")
        .map(|ms| ms / 1_000.0)
        .filter(|seconds| *seconds > 0.0);
    let delivered_per_sec = field(result, "delivered_per_sec")
        .or_else(|| field(result, "requests_per_sec"))
        .or_else(|| field(result, "cycles_per_sec"))
        .or_else(|| field(result, "forwarded_frames_per_sec"))
        .or_else(|| elapsed_seconds.map(|seconds| delivered / seconds));
    let rate_metric = rate_metric(args.scenario);
    let rtt_p50_ms = field(result, "rtt_p50_ms").or_else(|| field(result, "transfer_p50_ms"));
    let rtt_p99_ms = field(result, "rtt_p99_ms").or_else(|| field(result, "transfer_p99_ms"));
    let application_payload_bytes = field(result, "payload_bytes")
        .or_else(|| field(result, "carried_payload_bytes"))
        .or_else(|| {
            match (
                field(result, "request_bytes"),
                field(result, "response_bytes"),
            ) {
                (Some(requests), Some(responses)) => Some(requests + responses),
                _ => None,
            }
        });
    let is_transport = matches!(
        conformance_rule,
        ConformanceRule::ExactTransport | ConformanceRule::ExactTransportResource
    );

    let mut rows = vec![
        row(
            Axis::Conformance,
            "settled_clean",
            Some(f64::from(settled_clean)),
            "bool",
        ),
        row(Axis::Conformance, "sent", Some(sent), "msgs"),
        row(Axis::Conformance, "delivered", Some(delivered), "msgs"),
        row(
            Axis::Conformance,
            "responder_delivered",
            Some(responder_delivered),
            "msgs",
        ),
        row(Axis::Conformance, "timed_out", Some(timeouts), "msgs"),
        row(
            Axis::Conformance,
            "receipt_unproved",
            field(result, "receipt_unproved"),
            "msgs",
        ),
        row(Axis::Conformance, "raced", Some(raced), "msgs"),
        row(Axis::Conformance, "locally_culled", Some(culled), "msgs"),
        row(
            Axis::Conformance,
            "attempted",
            field(result, "attempted").or(Some(sent + culled)),
            "msgs",
        ),
        row(
            Axis::Conformance,
            "expected_response_bytes",
            field(result, "expected_response_bytes"),
            "bytes",
        ),
        row(
            Axis::Conformance,
            "responder_payload_bytes",
            field(responder_result, "payload_bytes")
                .or_else(|| field(responder_result, "response_bytes")),
            "bytes",
        ),
        row(
            Axis::Conformance,
            "endpoint_count_complete",
            Some(f64::from(responder_delivered == sent)),
            "bool",
        ),
        row(
            Axis::Conformance,
            "zero_timeouts",
            Some(f64::from(timeouts == 0.0)),
            "bool",
        ),
        row(
            Axis::Throughput,
            rate_metric,
            delivered_per_sec.filter(|_| !died && perf_valid),
            "msgs/s",
        ),
        row(
            Axis::Throughput,
            "goodput_bytes_per_sec",
            field(result, "goodput_bytes_per_sec").filter(|_| !died && perf_valid),
            "B/s",
        ),
        row(
            Axis::Latency,
            "rtt_p50_ms",
            rtt_p50_ms.filter(|_| !died && perf_valid),
            "ms",
        ),
        row(
            Axis::Latency,
            "rtt_p99_ms",
            rtt_p99_ms.filter(|_| !died && perf_valid),
            "ms",
        ),
    ];
    if is_transport {
        rows.extend([
            row(
                Axis::Throughput,
                "carried_payload_bytes_per_sec",
                field(result, "carried_payload_bytes_per_sec")
                    .filter(|_| !died && perf_valid && settled_clean),
                "B/s",
            ),
            row(
                Axis::Throughput,
                "ingress_wire_bytes_per_sec",
                field(result, "ingress_wire_bytes_per_sec")
                    .filter(|_| !died && perf_valid && settled_clean),
                "B/s",
            ),
            row(
                Axis::Throughput,
                "egress_wire_bytes_per_sec",
                field(result, "egress_wire_bytes_per_sec")
                    .filter(|_| !died && perf_valid && settled_clean),
                "B/s",
            ),
            row(
                Axis::Throughput,
                "harness_carried_payload_bytes_per_sec",
                field(result, "harness_carried_payload_bytes_per_sec"),
                "B/s",
            ),
            row(
                Axis::Throughput,
                "harness_source_payload_bytes_per_sec",
                field(result, "harness_source_payload_bytes_per_sec"),
                "B/s",
            ),
            row(
                Axis::Throughput,
                "harness_sink_payload_bytes_per_sec",
                field(result, "harness_sink_payload_bytes_per_sec"),
                "B/s",
            ),
            row(
                Axis::Conformance,
                "harness_headroom",
                field(result, "harness_headroom"),
                "bool",
            ),
            row(
                Axis::Conformance,
                "harness_calibration_ms",
                field(result, "harness_calibration_ms"),
                "ms",
            ),
            row(
                Axis::Conformance,
                "relay_bitrate_bps",
                field(result, "relay_bitrate_bps"),
                "bps",
            ),
            row(
                Axis::Conformance,
                "relay_mtu_bytes",
                field(result, "relay_mtu_bytes"),
                "bytes",
            ),
            row(
                Axis::Memory,
                "wire_driver_peak_rss_bytes",
                Some(initiator_rss as f64).filter(|_| perf_valid),
                "bytes",
            ),
            row(
                Axis::Energy,
                "wire_driver_cpu_seconds",
                Some(initiator_cpu),
                "s",
            ),
        ]);
        for (metric, unit) in [
            ("sent_a_to_b", "frames"),
            ("carried_a_to_b", "frames"),
            ("sent_b_to_a", "frames"),
            ("carried_b_to_a", "frames"),
            ("proofs", "frames"),
            ("sent_payload_bytes", "bytes"),
            ("carried_payload_bytes", "bytes"),
            ("sent_payload_bytes_a_to_b", "bytes"),
            ("carried_payload_bytes_a_to_b", "bytes"),
            ("sent_payload_bytes_b_to_a", "bytes"),
            ("carried_payload_bytes_b_to_a", "bytes"),
            ("duplicates", "frames"),
            ("corrupt", "frames"),
            ("reordered", "frames"),
            ("unexpected", "frames"),
            ("missing", "frames"),
            ("timed_out_frames", "frames"),
            ("drain_timeouts", "frames"),
            ("outstanding", "frames"),
            ("buffer_pool_misses", "frames"),
            ("credit_leaks", "permits"),
            ("negotiated_link_mtu_bytes", "bytes"),
            ("resource_payload_bytes_per_frame", "bytes"),
        ] {
            rows.push(row(Axis::Conformance, metric, field(result, metric), unit));
        }
    } else {
        rows.extend([
            row(
                Axis::Memory,
                "initiator_peak_rss_bytes",
                Some(initiator_rss as f64).filter(|_| perf_valid),
                "bytes",
            ),
            row(
                Axis::Memory,
                "responder_peak_rss_bytes",
                Some(responder_rss as f64).filter(|_| perf_valid),
                "bytes",
            ),
            row(
                Axis::Energy,
                "initiator_cpu_seconds",
                Some(initiator_cpu),
                "s",
            ),
            row(
                Axis::Energy,
                "responder_cpu_seconds",
                Some(responder_cpu),
                "s",
            ),
        ]);
    }
    if let Some(relay) = &relay {
        rows.push(row(
            Axis::Memory,
            "relay_peak_rss_bytes",
            Some(relay.peak_rss_bytes as f64).filter(|_| perf_valid),
            "bytes",
        ));
        rows.push(row(
            Axis::Energy,
            "relay_cpu_seconds",
            Some(relay.cpu_seconds),
            "s",
        ));
    }
    if let Some(after_reconnect) = field(result, "delivered_after_reconnect") {
        rows.push(row(
            Axis::Conformance,
            "route_survived",
            Some(f64::from(after_reconnect > 0.0)),
            "bool",
        ));
    }
    if let (Some((raw_joules, wall_seconds)), Some(idle_watts)) = (energy, idle_watts) {
        let net_joules = raw_joules - idle_watts * wall_seconds;
        let measurable = net_joules > 0.0;
        let per_delivered_mj = (measurable && delivered > 0.0 && !died && perf_valid)
            .then(|| net_joules * 1_000.0 / delivered);
        let per_mib_mj = application_payload_bytes
            .filter(|bytes| *bytes > 0.0 && measurable && !died && perf_valid)
            .map(|bytes| net_joules * 1_000.0 / (bytes / 1_048_576.0));
        rows.push(row(
            Axis::Energy,
            if is_transport {
                "whole_cell_package_joules_raw"
            } else {
                "package_joules_raw"
            },
            Some(raw_joules),
            "J",
        ));
        rows.push(row(
            Axis::Energy,
            "idle_baseline_watts",
            Some(idle_watts),
            "W",
        ));
        rows.push(row(
            Axis::Energy,
            if is_transport {
                "whole_cell_net_joules"
            } else {
                "net_joules"
            },
            measurable.then_some(net_joules),
            "J",
        ));
        rows.push(row(
            Axis::Energy,
            if is_transport {
                "whole_cell_net_millijoules_per_forwarded"
            } else {
                "net_millijoules_per_delivered"
            },
            per_delivered_mj,
            if is_transport { "mJ/frame" } else { "mJ/msg" },
        ));
        let total_cpu = initiator_cpu + responder_cpu;
        let initiator_share = if total_cpu > 0.0 {
            initiator_cpu / total_cpu
        } else {
            0.5
        };
        if is_transport {
            rows.push(row(
                Axis::Energy,
                "whole_cell_net_millijoules_per_mebibyte",
                per_mib_mj,
                "mJ/MiB",
            ));
        } else {
            rows.push(row(
                Axis::Energy,
                "initiator_net_millijoules_per_delivered",
                per_delivered_mj.map(|mj| mj * initiator_share),
                "mJ/msg",
            ));
            rows.push(row(
                Axis::Energy,
                "responder_net_millijoules_per_delivered",
                per_delivered_mj.map(|mj| mj * (1.0 - initiator_share)),
                "mJ/msg",
            ));
            rows.push(row(
                Axis::Energy,
                "initiator_net_millijoules_per_mebibyte",
                per_mib_mj.map(|mj| mj * initiator_share),
                "mJ/MiB",
            ));
            rows.push(row(
                Axis::Energy,
                "responder_net_millijoules_per_mebibyte",
                per_mib_mj.map(|mj| mj * (1.0 - initiator_share)),
                "mJ/MiB",
            ));
        }
        if measurable {
            let combined = per_delivered_mj.unwrap_or(f64::NAN);
            if is_transport {
                println!(
                    "\nSUMMARY whole-cell energy raw={raw_joules:.1}J over {wall_seconds:.1}s \
                     (idle {idle_watts:.2}W) | net={net_joules:.1}J | \
                     {combined:.2} mJ/forwarded-frame (not relay-only)",
                );
            } else {
                println!(
                    "\nSUMMARY energy raw={raw_joules:.1}J over {wall_seconds:.1}s \
                     (idle {idle_watts:.2}W) | net={net_joules:.1}J | {combined:.2} mJ/msg \
                     (initiator {:.2} / responder {:.2}, by cpu {:.0}%/{:.0}%)",
                    combined * initiator_share,
                    combined * (1.0 - initiator_share),
                    initiator_share * 100.0,
                    (1.0 - initiator_share) * 100.0,
                );
            }
        } else {
            println!(
                "\nSUMMARY energy raw={raw_joules:.1}J over {wall_seconds:.1}s ran BELOW the \
                 idle baseline ({idle_watts:.2}W) — net energy unmeasurable this run \
                 (baseline drift), filed as pending",
            );
        }
    }
    if let Some(wire_line) = &wire_line {
        let wire_total = field(wire_line, "a_to_b_bytes").unwrap_or(0.0)
            + field(wire_line, "b_to_a_bytes").unwrap_or(0.0);
        let efficiency = application_payload_bytes
            .filter(|_| wire_total > 0.0 && !died)
            .map(|p| p / wire_total);
        rows.push(row(
            Axis::Throughput,
            "wire_bytes_total",
            Some(wire_total),
            "bytes",
        ));
        rows.push(row(
            Axis::Throughput,
            "payload_per_wire_byte",
            efficiency,
            "ratio",
        ));
        println!(
            "\nSUMMARY wire bytes={wire_total:.0} | payload/wire={}",
            efficiency
                .map(|e| format!("{e:.3}"))
                .unwrap_or_else(|| "unmeasured".into()),
        );
    }
    println!(
        "\nSUMMARY scenario={} pairing={pairing_label} host={}\n\
         SUMMARY conformance sent={sent:.0} delivered={delivered:.0} \
         responder_seen={responder_delivered:.0} timed_out={timeouts:.0} raced={raced:.0} locally_culled={culled:.0} settled_clean={}",
        args.scenario, stamp.host, settled_clean,
    );
    if is_transport {
        println!(
            "SUMMARY wire-driver cpu={initiator_cpu:.2} s peak_rss={:.1} MiB",
            initiator_rss as f64 / (1024.0 * 1024.0),
        );
    } else {
        println!(
            "SUMMARY initiator cpu={initiator_cpu:.2} s peak_rss={:.1} MiB | \
             responder cpu={responder_cpu:.2} s peak_rss={:.1} MiB",
            initiator_rss as f64 / (1024.0 * 1024.0),
            responder_rss as f64 / (1024.0 * 1024.0),
        );
    }
    if let Some(relay) = &relay {
        println!(
            "SUMMARY relay cpu={:.2} s peak_rss={:.1} MiB",
            relay.cpu_seconds,
            relay.peak_rss_bytes as f64 / (1024.0 * 1024.0),
        );
    }
    if let Some(after_reconnect) = field(result, "delivered_after_reconnect") {
        println!(
            "SUMMARY tunnel route_survived={} delivered_after_reconnect={after_reconnect:.0}",
            after_reconnect > 0.0,
        );
    }
    let subject_slug = subject.file_slug();
    if args.write {
        assert!(settled_clean, "refuse to publish a non-conformant sample");
        write_rows(&stamp.host, args.scenario.as_str(), &subject_slug, &rows);
        println!(
            "SUMMARY rows filed under results/{}/{}/{subject_slug}.jsonl",
            stamp.host, args.scenario,
        );
    } else {
        println!("SUMMARY no-write smoke: result rows were not published");
    }
    settled_clean
}

#[cfg(test)]
mod tests {
    use super::{rate_metric, scenario_conforms, Conformance};
    use benchmarks::{ConformanceRule, ScenarioId};

    #[test]
    fn request_results_keep_the_manifest_owned_primary_metric() {
        assert_eq!(rate_metric(ScenarioId::RequestResponse), "requests_per_sec");
        assert_eq!(
            rate_metric(ScenarioId::SinglePacketThroughput),
            "delivered_per_sec"
        );
    }

    #[test]
    fn strict_link_accounting_requires_every_wire_send() {
        assert!(scenario_conforms(&Conformance {
            rule: ConformanceRule::ExactLink,
            sent: 2_143.0,
            delivered: 2_143.0,
            timeouts: 0.0,
            raced: 0.0,
            culled: 13.0,
            responder_delivered: 2_143.0,
            result: "RESULT attempted=2156 payload_bytes=500000",
            responder_result: "RESULT delivered=2143 payload_bytes=500000",
            require_harness_headroom: true,
        }));
        assert!(!scenario_conforms(&Conformance {
            rule: ConformanceRule::ExactLink,
            sent: 2_156.0,
            delivered: 2_143.0,
            timeouts: 13.0,
            raced: 0.0,
            culled: 0.0,
            responder_delivered: 2_156.0,
            result: "RESULT attempted=2156",
            responder_result: "RESULT delivered=2156",
            require_harness_headroom: true,
        }));
    }

    #[test]
    fn single_packet_timeouts_fail_release_conformance() {
        assert!(!scenario_conforms(&Conformance {
            rule: ConformanceRule::ExactSingle,
            sent: 100.0,
            delivered: 99.0,
            timeouts: 1.0,
            raced: 0.0,
            culled: 0.0,
            responder_delivered: 99.0,
            result: "RESULT sent=100 delivered=99 timeouts=1",
            responder_result: "RESULT delivered=99",
            require_harness_headroom: true,
        }));
    }

    #[test]
    fn request_conformance_requires_exact_served_response_bytes() {
        let valid = Conformance {
            rule: ConformanceRule::ExactRequest,
            sent: 40.0,
            delivered: 40.0,
            timeouts: 0.0,
            raced: 0.0,
            culled: 0.0,
            responder_delivered: 40.0,
            result: "RESULT expected_response_bytes=120000 response_bytes=120000 request_window=4 request_links=4",
            responder_result: "RESULT served=40 response_bytes=120000",
            require_harness_headroom: true,
        };
        assert!(scenario_conforms(&valid));
        assert!(!scenario_conforms(&Conformance {
            delivered: 19.0,
            ..valid
        }));
        assert!(!scenario_conforms(&Conformance {
            responder_delivered: 19.0,
            ..valid
        }));
        assert!(!scenario_conforms(&Conformance {
            result: "RESULT expected_response_bytes=120000 response_bytes=119999 request_window=4 request_links=4",
            ..valid
        }));
    }

    #[test]
    fn resource_conformance_requires_exact_application_bytes() {
        assert!(scenario_conforms(&Conformance {
            rule: ConformanceRule::ExactResource,
            sent: 1.0,
            delivered: 1.0,
            timeouts: 0.0,
            raced: 0.0,
            culled: 0.0,
            responder_delivered: 1.0,
            result: "RESULT payload_bytes=67108864",
            responder_result: "RESULT received=1 payload_bytes=67108864",
            require_harness_headroom: true,
        }));
    }

    #[test]
    fn transport_conformance_requires_exact_bidirectional_accounting_and_headroom() {
        let valid = Conformance {
            rule: ConformanceRule::ExactTransport,
            sent: 20.0,
            delivered: 20.0,
            timeouts: 0.0,
            raced: 0.0,
            culled: 0.0,
            responder_delivered: 20.0,
            result: "RESULT sent=20 carried=20 proofs=20 relay_bitrate_bps=1000000000 relay_mtu_bytes=524288 sent_a_to_b=10 carried_a_to_b=10 sent_b_to_a=10 carried_b_to_a=10 sent_payload_bytes=4000 carried_payload_bytes=4000 sent_payload_bytes_a_to_b=2000 carried_payload_bytes_a_to_b=2000 sent_payload_bytes_b_to_a=2000 carried_payload_bytes_b_to_a=2000 missing=0 duplicates=0 corrupt=0 reordered=0 unexpected=0 timed_out_frames=0 drain_timeouts=0 outstanding=0 buffer_pool_misses=0 credit_leaks=0 harness_source_payload_bytes_per_sec=2000 harness_sink_payload_bytes_per_sec=1900 harness_calibration_ms=2000 harness_headroom=1",
            responder_result: "RESULT carried=20",
            require_harness_headroom: true,
        };
        assert!(scenario_conforms(&valid));
        for (healthy, broken) in [
            ("proofs=20", "proofs=19"),
            ("relay_bitrate_bps=1000000000", "relay_bitrate_bps=0"),
            ("relay_mtu_bytes=524288", "relay_mtu_bytes=0"),
            ("sent_a_to_b=10", "sent_a_to_b=0"),
            ("carried_a_to_b=10", "carried_a_to_b=9"),
            ("sent_b_to_a=10", "sent_b_to_a=0"),
            ("carried_b_to_a=10", "carried_b_to_a=9"),
            ("carried_payload_bytes=4000", "carried_payload_bytes=3999"),
            (
                "carried_payload_bytes_a_to_b=2000",
                "carried_payload_bytes_a_to_b=1999",
            ),
            (
                "carried_payload_bytes_b_to_a=2000",
                "carried_payload_bytes_b_to_a=1999",
            ),
            ("duplicates=0", "duplicates=1"),
            ("corrupt=0", "corrupt=1"),
            ("reordered=0", "reordered=1"),
            ("unexpected=0", "unexpected=1"),
            ("missing=0", "missing=1"),
            ("timed_out_frames=0", "timed_out_frames=1"),
            ("drain_timeouts=0", "drain_timeouts=1"),
            ("outstanding=0", "outstanding=1"),
            ("buffer_pool_misses=0", "buffer_pool_misses=1"),
            ("credit_leaks=0", "credit_leaks=1"),
            (
                "harness_source_payload_bytes_per_sec=2000",
                "harness_source_payload_bytes_per_sec=0",
            ),
            (
                "harness_sink_payload_bytes_per_sec=1900",
                "harness_sink_payload_bytes_per_sec=0",
            ),
            ("harness_calibration_ms=2000", "harness_calibration_ms=1999"),
            ("harness_headroom=1", "harness_headroom=0"),
        ] {
            let result = valid.result.replacen(healthy, broken, 1);
            assert!(
                !scenario_conforms(&Conformance {
                    result: &result,
                    ..valid
                }),
                "{broken} must fail"
            );
        }
        assert!(scenario_conforms(&Conformance {
            result: &valid
                .result
                .replace("harness_headroom=1", "harness_headroom=0")
                .replace("harness_calibration_ms=2000", "harness_calibration_ms=100"),
            require_harness_headroom: false,
            ..valid
        }));
    }

    #[test]
    fn transported_resource_conformance_requires_negotiated_mtu_and_no_data_proofs() {
        let valid = Conformance {
            rule: ConformanceRule::ExactTransportResource,
            sent: 20.0,
            delivered: 20.0,
            timeouts: 0.0,
            raced: 0.0,
            culled: 0.0,
            responder_delivered: 20.0,
            result: "RESULT sent=20 carried=20 proofs=0 relay_bitrate_bps=1000000000 relay_mtu_bytes=524288 negotiated_link_mtu_bytes=524288 resource_payload_bytes_per_frame=524268 sent_a_to_b=10 carried_a_to_b=10 sent_b_to_a=10 carried_b_to_a=10 sent_payload_bytes=10485360 carried_payload_bytes=10485360 sent_payload_bytes_a_to_b=5242680 carried_payload_bytes_a_to_b=5242680 sent_payload_bytes_b_to_a=5242680 carried_payload_bytes_b_to_a=5242680 missing=0 duplicates=0 corrupt=0 reordered=0 unexpected=0 timed_out_frames=0 drain_timeouts=0 outstanding=0 buffer_pool_misses=0 credit_leaks=0 harness_source_payload_bytes_per_sec=2000 harness_sink_payload_bytes_per_sec=1900 harness_calibration_ms=2000 harness_headroom=1",
            responder_result: "RESULT carried=20",
            require_harness_headroom: true,
        };
        assert!(scenario_conforms(&valid));
        for (healthy, broken) in [
            ("proofs=0", "proofs=20"),
            (
                "negotiated_link_mtu_bytes=524288",
                "negotiated_link_mtu_bytes=524287",
            ),
            (
                "resource_payload_bytes_per_frame=524268",
                "resource_payload_bytes_per_frame=524267",
            ),
        ] {
            let result = valid.result.replacen(healthy, broken, 1);
            assert!(
                !scenario_conforms(&Conformance {
                    result: &result,
                    ..valid
                }),
                "{broken} must fail"
            );
        }
    }
}
