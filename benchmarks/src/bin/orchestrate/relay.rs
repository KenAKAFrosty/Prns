use super::implementation::{implementation, unsupported_pairing, Implementation};
use super::process::{await_line, spawn_role};
use super::results::{file_results, CollectedRun};
use super::*;

pub(super) fn run_relay_interop(args: &Args, manifest_json: &serde_json::Value, manifest: &Path) {
    let version = manifest_json["version"].as_u64().unwrap_or(1) as u32;
    let initiator_impl = implementation(&args.initiator);
    let responder_impl = implementation(&args.responder);
    let relay_impl = implementation(&args.relay);
    let mechanism = manifest_json["profile"]["mechanism"]
        .as_str()
        .unwrap_or("single");
    if let Some(reason) = unsupported_pairing(&initiator_impl, &responder_impl, mechanism) {
        println!(
            "SKIP scenario={} relay={} initiator={} responder={} reason={reason}",
            args.scenario, relay_impl.label, initiator_impl.label, responder_impl.label,
        );
        return;
    }

    let endpoints_are_self = args.initiator == "self" && args.responder == "self";
    let pairing_slug = if endpoints_are_self {
        relay_impl.slug.to_string()
    } else {
        format!(
            "{}--{}--{}",
            relay_impl.slug, initiator_impl.slug, responder_impl.slug
        )
    };
    let pairing_label = if endpoints_are_self {
        format!("{} (relay)", relay_impl.label)
    } else {
        format!(
            "{} (relay) {}/{}",
            relay_impl.label, initiator_impl.label, responder_impl.label
        )
    };

    let node = |subject: &Implementation| {
        subject
            .interop_command()
            .unwrap_or_else(|| panic!("implementation {:?} fields no interop node", subject.name))
    };

    let meter = PowerMeter::detect();
    if meter.is_none() {
        println!("{}", energy_unavailable_hint());
    }
    let idle_watts = meter
        .as_ref()
        .map(|m| m.idle_watts(Duration::from_millis(1500)));
    let bracket = meter.as_ref().map(|m| m.start());

    let mut relay = spawn_role(node(&relay_impl), manifest, "relay", "127.0.0.1:0", args);
    let relay_ready = await_line(&relay, "READY", Duration::from_secs(10));
    let endpoints = relay_ready
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("addr="))
        .expect("relay READY carries addr=<side_a>><side_b>");
    let (addr_a, addr_b) = endpoints
        .split_once('>')
        .expect("relay READY addr is <side_a>><side_b>");

    let responder = spawn_role(node(&responder_impl), manifest, "responder", addr_a, args);
    await_line(&responder, "READY", Duration::from_secs(10));
    let initiator = spawn_role(node(&initiator_impl), manifest, "initiator", addr_b, args);

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
    let _ = relay.child.kill();
    let relay_metrics = relay.finalize();

    file_results(
        args,
        version,
        &pairing_slug,
        &pairing_label,
        CollectedRun {
            result: &result,
            responder_result: &responder_result,
            wire_line: None,
            energy,
            idle_watts,
            initiator: initiator_metrics,
            responder: responder_metrics,
            relay: Some(relay_metrics),
        },
    );
}
