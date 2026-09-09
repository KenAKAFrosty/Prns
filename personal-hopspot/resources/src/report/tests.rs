use std::path::Path;

use personal_hopspot_builder::{BuildContext, BuildIntent, BuildVersion, LtoMode};
use personal_hopspot_memory::T114;
use serde_json::{json, Value};

use super::build::{build_identity, firmware_flash_usage, ReportError};
use super::compare::{self, ComparisonError, CompatibilityDimension};
use super::contract;
use super::model::{BuildStatus, Evidence, ResourceReport, SCHEMA_VERSION};
use crate::matrix::RecipeIdentity;

#[test]
fn report_schema_round_trips_known_fields() -> Result<(), Box<dyn std::error::Error>> {
    let value = report_value();
    let report: ResourceReport = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(report)?, value);
    Ok(())
}

#[test]
fn report_schema_rejects_unknown_fields() {
    let mut value = report_value();
    value["unexpected"] = Value::Bool(true);
    assert!(serde_json::from_value::<ResourceReport>(value).is_err());
}

#[test]
fn report_schema_rejects_malformed_fingerprints() {
    let mut value = report_value();
    value["build"]["fingerprint"] = Value::String("invalid".to_string());
    assert!(serde_json::from_value::<ResourceReport>(value).is_err());
}

#[test]
fn report_validation_rejects_a_tampered_toolchain_identity(
) -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["toolchain"]["rustc_version"] = json!("tampered");
    let report: ResourceReport = serde_json::from_value(value)?;
    assert!(matches!(
        compare::validate_report(Path::new("report.json"), &report),
        Err(ComparisonError::InvalidToolchainIdentity { .. })
    ));
    Ok(())
}

#[test]
fn overflow_reports_preserve_only_available_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["status"] = json!({
        "kind": "memory-overflow",
        "regions": [{"linker_region": "FLASH", "overflow_bytes": 86_240}]
    });
    value["firmware_flash"] = json!({"kind": "unavailable"});
    value["static_ram"] = json!({"kind": "unavailable"});
    value["artifacts"] = json!({"kind": "unavailable"});
    value["analysis"]["allocated_sections"] = json!({"kind": "unavailable"});
    value["analysis"]["flash_attribution"]["kind"] = json!("partial");
    value["analysis"]["executable"] = json!({"kind": "unavailable"});
    value["analysis"]["async_memory"] = json!({"kind": "unavailable"});
    let report: ResourceReport = serde_json::from_value(value.clone())?;
    compare::validate_report(Path::new("overflow.json"), &report)?;
    assert_eq!(serde_json::to_value(report)?, value);
    Ok(())
}

#[test]
fn overflow_reports_require_region_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["status"] = json!({"kind": "memory-overflow", "regions": []});
    let report: ResourceReport = serde_json::from_value(value)?;
    assert!(matches!(
        compare::validate_report(Path::new("overflow.json"), &report),
        Err(ComparisonError::MissingOverflowEvidence { .. })
    ));
    Ok(())
}

#[test]
fn overflow_reports_require_positive_region_sizes() -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["status"] = json!({
        "kind": "memory-overflow",
        "regions": [{"linker_region": "FLASH", "overflow_bytes": 0}]
    });
    let report: ResourceReport = serde_json::from_value(value)?;
    assert!(matches!(
        compare::validate_report(Path::new("overflow.json"), &report),
        Err(ComparisonError::InvalidOverflowEvidence { .. })
    ));
    Ok(())
}

#[test]
fn successful_reports_require_complete_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["firmware_flash"] = json!({"kind": "unavailable"});
    let report: ResourceReport = serde_json::from_value(value)?;
    assert!(matches!(
        compare::validate_report(Path::new("success.json"), &report),
        Err(ComparisonError::MissingFlashEvidence { .. })
    ));
    Ok(())
}

#[test]
fn successful_reports_require_complete_attribution() -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["analysis"]["flash_attribution"]["kind"] = json!("partial");
    let report: ResourceReport = serde_json::from_value(value)?;
    assert!(matches!(
        compare::validate_report(Path::new("success.json"), &report),
        Err(ComparisonError::MissingAttributionEvidence { .. })
    ));
    Ok(())
}

#[test]
fn successful_reports_require_complete_executable_evidence(
) -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["analysis"]["executable"] =
        json!({"kind": "partial", "value": value["analysis"]["executable"]["value"].clone()});
    let report: ResourceReport = serde_json::from_value(value)?;
    assert!(matches!(
        compare::validate_report(Path::new("success.json"), &report),
        Err(ComparisonError::MissingExecutableEvidence { .. })
    ));
    Ok(())
}

#[test]
fn successful_reports_require_complete_async_memory_evidence(
) -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["analysis"]["async_memory"]["kind"] = json!("partial");
    let report: ResourceReport = serde_json::from_value(value)?;
    assert!(matches!(
        compare::validate_report(Path::new("success.json"), &report),
        Err(ComparisonError::MissingAsyncMemoryEvidence { .. })
    ));
    Ok(())
}

#[test]
fn malformed_executable_evidence_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    for mutation in [
        |value: &mut Value| {
            value["analysis"]["executable"]["value"]["rust_target"] = json!("wrong-target");
        },
        |value: &mut Value| {
            value["analysis"]["executable"]["value"]["load_segments"][0]["run_end"] =
                json!(155_691);
        },
        |value: &mut Value| {
            value["analysis"]["executable"]["value"]["functions"]["boundary_count"] = json!(0);
        },
        |value: &mut Value| {
            value["analysis"]["executable"]["value"]["disassembly"]["decoded_bytes"] = json!(41);
        },
    ] {
        let mut value = report_value();
        mutation(&mut value);
        let report: ResourceReport = serde_json::from_value(value)?;
        assert!(matches!(
            compare::validate_report(Path::new("malformed.json"), &report),
            Err(ComparisonError::InvalidExecutableEvidence { .. })
        ));
    }
    Ok(())
}

#[test]
fn malformed_stack_evidence_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    for mutation in [
        |value: &mut Value| {
            value["analysis"]["executable"]["value"]["stack"]["kind"] = json!("complete");
        },
        |value: &mut Value| {
            value["analysis"]["executable"]["value"]["stack"]["value"]["largest_known_path"]
                ["bytes"] = json!(31);
        },
        |value: &mut Value| {
            value["analysis"]["executable"]["value"]["stack"]["value"]["limit"]["headroom_bytes"] =
                json!(69_599);
        },
        |value: &mut Value| {
            value["analysis"]["executable"]["value"]["stack"]["value"]["frame_source"] =
                json!("dwarf-debug-frame");
        },
    ] {
        let mut value = report_value();
        mutation(&mut value);
        let report: ResourceReport = serde_json::from_value(value)?;
        assert!(matches!(
            compare::validate_report(Path::new("malformed.json"), &report),
            Err(ComparisonError::InvalidExecutableEvidence { .. })
        ));
    }
    Ok(())
}

#[test]
fn malformed_async_memory_evidence_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    for mutation in [
        |value: &mut Value| {
            value["analysis"]["async_memory"]["value"]["task_pool_bytes"] = json!(31);
        },
        |value: &mut Value| {
            value["analysis"]["async_memory"]["value"]["task_pools"][0]["address"] =
                json!(536_920_128);
        },
        |value: &mut Value| {
            value["analysis"]["async_memory"]["value"]["scenario_futures"] = json!({
                "kind": "measured",
                "futures": [{"scenario": "duplicate", "bytes": 1}, {"scenario": "duplicate", "bytes": 2}]
            });
        },
    ] {
        let mut value = report_value();
        mutation(&mut value);
        let report: ResourceReport = serde_json::from_value(value)?;
        assert!(matches!(
            compare::validate_report(Path::new("malformed.json"), &report),
            Err(ComparisonError::InvalidAsyncMemoryEvidence { .. })
        ));
    }
    Ok(())
}

#[test]
fn overflow_reports_reject_duplicate_regions() -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["status"] = json!({
        "kind": "memory-overflow",
        "regions": [
            {"linker_region": "FLASH", "overflow_bytes": 1},
            {"linker_region": "FLASH", "overflow_bytes": 2}
        ]
    });
    let report: ResourceReport = serde_json::from_value(value)?;
    assert!(matches!(
        compare::validate_report(Path::new("overflow.json"), &report),
        Err(ComparisonError::InvalidOverflowEvidence { .. })
    ));
    Ok(())
}

#[test]
fn compatible_reports_call_out_lto_and_resource_deltas() -> Result<(), Box<dyn std::error::Error>> {
    let before: ResourceReport = serde_json::from_value(report_value())?;
    let mut after_value = report_value();
    after_value["build"]["fingerprint"] = Value::String("b".repeat(64));
    after_value["build"]["lto"] = Value::String("thin".to_string());
    after_value["firmware_flash"]["value"]["image_bytes"] = json!(699_000);
    after_value["firmware_flash"]["value"]["headroom_bytes"] = json!(66_952);
    after_value["artifacts"]["value"][0]["bytes"] = json!(40);
    after_value["artifacts"]["value"][0]["fingerprint"] = Value::String("b".repeat(64));
    after_value["static_ram"]["value"][0]["static_section_bytes"] = json!(137_356);
    after_value["static_ram"]["value"][0]["capacity"]["headroom_bytes"] = json!(6_004);
    after_value["analysis"]["allocated_sections"]["value"][0]["run_end"] = json!(155_688);
    after_value["analysis"]["allocated_sections"]["value"][0]["run_bytes"] = json!(40);
    after_value["analysis"]["allocated_sections"]["value"][0]["load_bytes"] = json!(40);
    after_value["analysis"]["flash_attribution"]["value"]["crates"]["coverage"] = json!({
        "analyzed_bytes": 45,
        "attributed_bytes": 43,
        "unclassified_bytes": 2
    });
    after_value["analysis"]["flash_attribution"]["value"]["crates"]["largest"][0]["bytes"] =
        json!(42);
    after_value["analysis"]["flash_attribution"]["value"]["crates"]["largest"]
        .as_array_mut()
        .ok_or("crate attribution fixture is not an array")?
        .push(json!({"name": "new-crate", "bytes": 1}));
    after_value["analysis"]["flash_attribution"]["value"]["symbols"]["coverage"] = json!({
        "analyzed_bytes": 45,
        "attributed_bytes": 43,
        "unclassified_bytes": 2
    });
    after_value["analysis"]["flash_attribution"]["value"]["symbols"]["largest"][0]["bytes"] =
        json!(42);
    after_value["analysis"]["flash_attribution"]["value"]["symbols"]["largest"]
        .as_array_mut()
        .ok_or("symbol attribution fixture is not an array")?
        .push(json!({"name": "new_crate::run", "bytes": 1}));
    let mut after: ResourceReport = serde_json::from_value(after_value)?;
    let executable = match &mut after.analysis.executable {
        Evidence::Complete(executable) => executable,
        Evidence::Partial(_) | Evidence::Unavailable => {
            return Err("fixture has no complete executable evidence".into());
        }
    };
    executable.load_segments[0].run_end = 155_688;
    executable.load_segments[0].load_end = 155_688;
    executable.load_segments[0].file_bytes = 40;
    executable.load_segments[0].memory_bytes = 40;
    executable.executable_sections[0].end = 155_688;
    executable.executable_sections[0].bytes = 40;
    executable.executable_sections[0].fingerprint =
        super::fingerprint::Fingerprint::parse("b".repeat(64))?;
    executable.functions.classified_bytes = 40;
    executable.functions.largest[0].end = 155_688;
    executable.functions.largest[0].bytes = 40;
    executable.functions.largest[0].fingerprint =
        super::fingerprint::Fingerprint::parse("b".repeat(64))?;
    executable.functions.boundaries_fingerprint =
        super::fingerprint::Fingerprint::parse("b".repeat(64))?;
    executable.disassembly.executable_bytes = 40;
    executable.disassembly.decoded_bytes = 40;
    let stack = match &mut executable.stack {
        Evidence::Complete(stack) | Evidence::Partial(stack) => stack,
        Evidence::Unavailable => return Err("fixture has no stack evidence".into()),
    };
    stack.largest_frames[0].bytes = 24;
    stack.largest_known_path.bytes = 24;
    stack.largest_known_path.frames[0].frame_bytes = 24;
    if let super::model::StackLimitIdentity::Declared { headroom_bytes, .. } = &mut stack.limit {
        *headroom_bytes = 69_608;
    }
    let async_memory = match &mut after.analysis.async_memory {
        Evidence::Complete(async_memory) => async_memory,
        Evidence::Partial(_) | Evidence::Unavailable => {
            return Err("fixture has no complete async-memory evidence".into());
        }
    };
    async_memory.task_pool_bytes = 24;
    async_memory.task_pools[0].bytes = 24;
    compare::validate_report(Path::new("before.json"), &before)?;
    compare::validate_report(Path::new("after.json"), &after)?;
    let comparison = compare::compare_reports(&before, &after)?;
    let rendered = compare::render_comparison(
        &comparison,
        Path::new("before.json"),
        Path::new("after.json"),
    );
    assert!(rendered.contains("setting lto configured -> thin"));
    assert!(rendered.contains("flash image 700000 -> 699000 (-1000)"));
    assert!(rendered.contains("flash headroom 65952 -> 66952 (+1000)"));
    assert!(rendered.contains("artifact \"firmware.bin\" fingerprint changed"));
    assert!(rendered.contains("ram internal-sram headroom 5004 -> 6004 (+1000)"));
    assert!(rendered.contains("section code load 42 -> 40 (-2)"));
    assert!(rendered.contains("attribution evidence complete -> complete"));
    assert!(rendered.contains("attribution crates analyzed 42 -> 45 (+3)"));
    assert!(rendered.contains("attribution crates candidate 1 40 -> 42 (+2) \"example\""));
    assert!(rendered.contains("attribution crates candidate 2 not-ranked -> 1 \"new-crate\""));
    assert!(rendered.contains("attribution symbols candidate 1 40 -> 42 (+2) \"example::run\""));
    assert!(rendered.contains("machine executable 42 -> 40 (-2)"));
    assert!(rendered.contains("machine section changed \".text\""));
    assert!(rendered.contains("machine function-boundaries changed"));
    assert!(rendered.contains("machine functions 1 -> 1"));
    assert!(rendered.contains("machine ranked-function changed \"example::run\""));
    assert!(rendered.contains("stack evidence partial -> partial"));
    assert!(rendered.contains("stack frame-source unchanged"));
    assert!(rendered.contains("stack frame-source evidence 5 -> 5 (0)"));
    assert!(rendered.contains("stack known-path lower-bound 32 -> 24 (-8)"));
    assert!(rendered.contains("stack known-path headroom 69600 -> 69608 (+8)"));
    assert!(rendered.contains("async task-pool total 32 -> 24 (-8)"));
    assert!(rendered.contains("async task-pool \"example::run\" 32 -> 24 (-8)"));
    assert!(rendered.contains(
        "async scenario-futures unavailable semantic-harness-not-produced -> semantic-harness-not-produced"
    ));
    Ok(())
}

#[test]
fn successful_and_overflowing_builds_compare_without_invented_deltas(
) -> Result<(), Box<dyn std::error::Error>> {
    let before: ResourceReport = serde_json::from_value(report_value())?;
    let mut after_value = report_value();
    after_value["build"]["fingerprint"] = Value::String("b".repeat(64));
    after_value["build"]["lto"] = Value::String("thin".to_string());
    make_overflow(&mut after_value, "FLASH", 86_240);
    let after: ResourceReport = serde_json::from_value(after_value)?;
    compare::validate_report(Path::new("before.json"), &before)?;
    compare::validate_report(Path::new("after.json"), &after)?;

    let introduced = compare::render_comparison(
        &compare::compare_reports(&before, &after)?,
        Path::new("before.json"),
        Path::new("after.json"),
    );
    assert!(introduced.contains("setting lto configured -> thin"));
    assert!(introduced.contains("status success -> memory-overflow"));
    assert!(introduced.contains("overflow \"FLASH\" none -> 86240"));
    assert!(introduced.contains("flash evidence complete -> unavailable"));
    assert!(introduced.contains("attribution evidence complete -> partial"));
    assert!(introduced.contains("attribution crates candidate 1 40 -> 40 (0) \"example\""));
    assert!(!introduced.contains("flash image"));

    let resolved = compare::render_comparison(
        &compare::compare_reports(&after, &before)?,
        Path::new("after.json"),
        Path::new("before.json"),
    );
    assert!(resolved.contains("status memory-overflow -> success"));
    assert!(resolved.contains("overflow \"FLASH\" 86240 -> none"));
    Ok(())
}

#[test]
fn unavailable_attribution_does_not_invent_candidates() -> Result<(), Box<dyn std::error::Error>> {
    let before: ResourceReport = serde_json::from_value(report_value())?;
    let mut after_value = report_value();
    make_overflow(&mut after_value, "FLASH", 86_240);
    after_value["analysis"]["flash_attribution"] = json!({"kind": "unavailable"});
    let after: ResourceReport = serde_json::from_value(after_value)?;
    compare::validate_report(Path::new("before.json"), &before)?;
    compare::validate_report(Path::new("after.json"), &after)?;

    let rendered = compare::render_comparison(
        &compare::compare_reports(&before, &after)?,
        Path::new("before.json"),
        Path::new("after.json"),
    );
    assert!(rendered.contains("attribution evidence complete -> unavailable"));
    assert!(!rendered.contains("attribution crates candidate"));
    assert!(!rendered.contains("attribution symbols candidate"));
    Ok(())
}

#[test]
fn preserved_lto_experiment_captures_overflow_and_control() -> Result<(), Box<dyn std::error::Error>>
{
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("experiments/lto");
    let fat_path = root.join("t-echo-s140-v6-fat.json");
    let thin_path = root.join("t-echo-s140-v6-thin.json");
    let control_path = root.join("mesh-tower-v2-thin.json");
    let fat = compare::load_report(&fat_path)?;
    let thin = compare::load_report(&thin_path)?;
    let control = compare::load_report(&control_path)?;

    assert_eq!(fat.target.id, "t-echo-s140-v6");
    assert_eq!(fat.build.lto, "fat");
    assert!(matches!(fat.status, BuildStatus::Success));
    let fat_flash = fat
        .firmware_flash
        .complete()
        .ok_or("fat T-Echo report has no complete flash evidence")?;
    assert_eq!(
        (fat_flash.image_bytes, fat_flash.headroom_bytes),
        (621_976, 4_712)
    );

    assert_eq!(thin.target, fat.target);
    assert_eq!(thin.build.lto, "thin");
    assert!(matches!(
        &thin.status,
        BuildStatus::MemoryOverflow { regions }
            if regions.len() == 1
                && regions[0].linker_region == "FLASH"
                && regions[0].overflow_bytes == 86_560
    ));
    assert!(matches!(
        thin.analysis.flash_attribution,
        Evidence::Partial(_)
    ));

    assert_eq!(control.target.id, "mesh-tower-v2");
    assert_eq!(control.build.lto, "thin");
    assert!(matches!(control.status, BuildStatus::Success));
    let control_flash = control
        .firmware_flash
        .complete()
        .ok_or("thin MeshTower report has no complete flash evidence")?;
    assert_eq!(
        (control_flash.image_bytes, control_flash.headroom_bytes),
        (651_508, 118_540)
    );
    assert_eq!(fat.toolchain, thin.toolchain);
    assert_eq!(fat.toolchain, control.toolchain);

    let rendered = compare::render_comparison(
        &compare::compare_reports(&fat, &thin)?,
        &fat_path,
        &thin_path,
    );
    assert!(rendered.contains("setting lto fat -> thin"));
    assert!(rendered.contains("overflow \"FLASH\" none -> 86560"));
    assert!(
        rendered.contains("attribution crates candidate 1 154592 -> 199770 (+45178) \"prns_core\"")
    );
    assert!(rendered.contains("attribution symbols candidate 1 26404 -> 26440 (+36)"));
    Ok(())
}

#[test]
fn overflow_comparisons_track_changed_and_unreported_regions(
) -> Result<(), Box<dyn std::error::Error>> {
    let mut before_value = report_value();
    make_overflow(&mut before_value, "FLASH", 86_240);
    let before: ResourceReport = serde_json::from_value(before_value)?;

    let mut after_value = report_value();
    make_overflow(&mut after_value, "FLASH", 80_000);
    let after: ResourceReport = serde_json::from_value(after_value)?;
    let changed = compare::render_comparison(
        &compare::compare_reports(&before, &after)?,
        Path::new("before.json"),
        Path::new("after.json"),
    );
    assert!(changed.contains("overflow \"FLASH\" 86240 -> 80000 (-6240)"));

    let mut other_value = report_value();
    make_overflow(&mut other_value, "RAM", 4_096);
    let other: ResourceReport = serde_json::from_value(other_value)?;
    let moved = compare::render_comparison(
        &compare::compare_reports(&before, &other)?,
        Path::new("before.json"),
        Path::new("after.json"),
    );
    assert!(moved.contains("overflow \"FLASH\" 86240 -> not-reported"));
    assert!(moved.contains("overflow \"RAM\" not-reported -> 4096"));
    Ok(())
}

#[test]
fn memory_contract_changes_are_not_compared_as_resource_deltas(
) -> Result<(), Box<dyn std::error::Error>> {
    let before: ResourceReport = serde_json::from_value(report_value())?;
    let mut after_value = report_value();
    after_value["memory_contract"]["fingerprint"] = Value::String("b".repeat(64));
    let after: ResourceReport = serde_json::from_value(after_value)?;
    assert!(matches!(
        compare::compare_reports(&before, &after),
        Err(ComparisonError::Incompatible {
            dimension: CompatibilityDimension::MemoryContract,
        })
    ));
    Ok(())
}

#[test]
fn malformed_ram_accounting_is_rejected_before_comparison() -> Result<(), Box<dyn std::error::Error>>
{
    let mut value = report_value();
    value["static_ram"]["value"][0]["capacity"]["headroom_bytes"] = json!(5_005);
    let report: ResourceReport = serde_json::from_value(value)?;
    assert!(matches!(
        compare::validate_report(Path::new("report.json"), &report),
        Err(ComparisonError::InvalidRamAccounting { .. })
    ));
    Ok(())
}

#[test]
fn build_only_reports_need_no_transport_artifacts() -> Result<(), Box<dyn std::error::Error>> {
    let mut value = report_value();
    value["artifacts"]["value"] = json!([]);
    let before: ResourceReport = serde_json::from_value(value.clone())?;
    let after: ResourceReport = serde_json::from_value(value)?;
    compare::validate_report(Path::new("before.json"), &before)?;
    compare::validate_report(Path::new("after.json"), &after)?;
    let comparison = compare::compare_reports(&before, &after)?;
    let rendered = compare::render_comparison(
        &comparison,
        Path::new("before.json"),
        Path::new("after.json"),
    );
    assert!(rendered.contains("flash image 700000 -> 700000 (0)"));
    Ok(())
}

#[test]
fn build_fingerprint_distinguishes_lto_configuration() -> Result<(), Box<dyn std::error::Error>> {
    let configured = BuildContext::new(
        Path::new("/repository"),
        Path::new("/output"),
        BuildVersion::Developer("0.1.0"),
    )?
    .with_intent(BuildIntent::ResourceReport {
        lto: LtoMode::Configured,
    });
    let thin = BuildContext::new(
        Path::new("/repository"),
        Path::new("/output"),
        BuildVersion::Developer("0.1.0"),
    )?
    .with_intent(BuildIntent::ResourceReport { lto: LtoMode::Thin });
    let configured = build_identity(&configured, recipe())?;
    let thin = build_identity(&thin, recipe())?;
    assert_ne!(configured.fingerprint, thin.fingerprint);
    assert_eq!(
        (configured.lto.as_str(), thin.lto.as_str()),
        ("configured", "thin")
    );
    Ok(())
}

#[test]
fn memory_contract_identity_captures_the_complete_profile() -> Result<(), Box<dyn std::error::Error>>
{
    let identity = contract::identity(&T114)?;
    assert_eq!(
        (
            identity.fingerprint.to_string().len(),
            identity.address_spaces.len(),
            identity.regions.len(),
            identity.runtime_reservations.len(),
            identity.firmware_owned_region.as_str(),
        ),
        (
            64,
            T114.address_spaces.len(),
            T114.regions.len(),
            T114.runtime_reservations.len(),
            T114.firmware.firmware_owned_region.0,
        )
    );
    Ok(())
}

#[test]
fn memory_contract_fingerprint_changes_with_profile_semantics(
) -> Result<(), Box<dyn std::error::Error>> {
    let t114 = contract::identity(&T114)?;
    let mesh_tower = contract::identity(&personal_hopspot_memory::MESH_TOWER_V2)?;
    assert_ne!(t114.fingerprint, mesh_tower.fingerprint);
    Ok(())
}

#[test]
fn firmware_flash_usage_is_bounded_by_the_owned_region() -> Result<(), ReportError> {
    let region = T114
        .region(T114.firmware.firmware_owned_region)
        .ok_or_else(|| ReportError::MissingFirmwareRegion {
            profile: T114.id.as_str().to_string(),
            region: T114.firmware.firmware_owned_region.0.to_string(),
        })?;
    let capacity = region.range.byte_len();
    let usage = firmware_flash_usage("t114", &T114, capacity - 1)?;
    assert_eq!(usage.headroom_bytes, 1);
    assert!(matches!(
        firmware_flash_usage("t114", &T114, capacity + 1),
        Err(ReportError::FirmwareOverflow {
            actual,
            maximum,
            ..
        }) if actual == capacity + 1 && maximum == capacity
    ));
    Ok(())
}

fn recipe() -> RecipeIdentity<'static> {
    RecipeIdentity {
        kind: "nrf-serial-dfu",
        package: "personal-hopspot-t114",
        binary: "personal-hopspot-t114",
        features: vec!["t114"],
    }
}

fn make_overflow(value: &mut Value, linker_region: &str, overflow_bytes: u64) {
    value["status"] = json!({
        "kind": "memory-overflow",
        "regions": [{
            "linker_region": linker_region,
            "overflow_bytes": overflow_bytes
        }]
    });
    value["firmware_flash"] = json!({"kind": "unavailable"});
    value["static_ram"] = json!({"kind": "unavailable"});
    value["artifacts"] = json!({"kind": "unavailable"});
    value["analysis"]["allocated_sections"] = json!({"kind": "unavailable"});
    value["analysis"]["flash_attribution"]["kind"] = json!("partial");
    value["analysis"]["executable"] = json!({"kind": "unavailable"});
    value["analysis"]["async_memory"] = json!({"kind": "unavailable"});
}

pub(super) fn report_value() -> Value {
    let fingerprint = "a".repeat(64);
    let boundary_encoding = format!(
        "[{{\"name\":\"example::run\",\"address\":155648,\"end\":155690,\"bytes\":42,\"fingerprint\":\"{fingerprint}\"}}]"
    );
    let boundaries_fingerprint = prns_flash_manifest::sha256_hex(boundary_encoding.as_bytes());
    let toolchain_fingerprint = "6e58e90c146639570099ad73f47e6e4a617f4e082daf70fb83c6719d3bc18129";
    let mut value = json!({
        "schema_version": SCHEMA_VERSION,
        "target": {
            "id": "t114",
            "display_name": "LILYGO T114",
            "memory_profile": "t114"
        },
        "architecture": {
            "rust_target": "thumbv7em-none-eabihf",
            "adapter": "thumbv7em-rust-lld",
            "linker_flavor": "rust-lld",
            "rustflags": [
                "-C",
                "link-arg=--icf=all",
                "-C",
                "llvm-args=-enable-machine-outliner",
                "-C",
                "llvm-args=-machine-outliner-reruns=2"
            ]
        },
        "build": {
            "fingerprint": fingerprint,
            "firmware_version": "0.1.0",
            "cargo_profile": "release",
            "recipe_kind": "nrf-serial-dfu",
            "package": "personal-hopspot-t114",
            "binary": "personal-hopspot-t114",
            "features": ["t114"],
            "lto": "configured"
        },
        "toolchain": {
            "fingerprint": toolchain_fingerprint,
            "rustc_version": "rustc 1.0.0",
            "cargo_version": "cargo 1.0.0",
            "linker_program": "rust-lld",
            "linker_version": "LLD 1.0.0"
        },
        "memory_contract": {
            "fingerprint": fingerprint,
            "address_spaces": [],
            "regions": [],
            "firmware_owned_region": "firmware",
            "transport_envelope": {
                "address_space": "internal-flash",
                "start": 0,
                "end": 1,
                "compatibility": "exact-firmware-region"
            },
            "runtime_reservations": [{
                "id": "minimum-runtime-stack",
                "address_space": "internal-ram",
                "bytes": 69632,
                "accounting": {
                    "kind": "dedicated",
                    "charge": "additional"
                }
            }]
        },
        "status": {"kind": "success"},
        "firmware_flash": {
            "kind": "complete",
            "value": {
                "region": "firmware",
                "start": 155648,
                "end": 921600,
                "image_bytes": 700000,
                "headroom_bytes": 65952
            }
        },
        "static_ram": {
            "kind": "complete",
            "value": [
                {
                    "backing_store": "internal-sram",
                    "address_spaces": ["internal-ram"],
                    "capacity": {
                        "kind": "known",
                        "bytes": 212992,
                        "headroom_bytes": 5004
                    },
                    "static_section_bytes": 138356,
                    "linker_padding_bytes": 0,
                    "additional_reservation_bytes": 69632,
                    "included_reservation_bytes": 0,
                    "external_reservation_bytes": 0
                }
            ]
        },
        "artifacts": {
            "kind": "complete",
            "value": [{
                "path": "firmware.bin",
                "bytes": 42,
                "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }]
        },
        "analysis": {
            "linker_map_bytes": 128,
            "allocated_sections": {
                "kind": "complete",
                "value": [
                    {
                        "name": ".text",
                        "kind": "code",
                        "run_address": 155648,
                        "run_end": 155690,
                        "run_bytes": 42,
                        "load_bytes": 42,
                        "alignment": 4
                    },
                    {
                        "name": ".bss",
                        "kind": "zero-fill",
                        "run_address": 536920064,
                        "run_end": 536920128,
                        "run_bytes": 64,
                        "load_bytes": 0,
                        "alignment": 8
                    }
                ]
            },
            "flash_attribution": {
                "kind": "complete",
                "value": {
                    "crates": {
                        "coverage": {
                            "analyzed_bytes": 42,
                            "attributed_bytes": 40,
                            "unclassified_bytes": 2
                        },
                        "largest": [
                            {"name": "example", "bytes": 40}
                        ]
                    },
                    "symbols": {
                        "coverage": {
                            "analyzed_bytes": 42,
                            "attributed_bytes": 40,
                            "unclassified_bytes": 2
                        },
                        "largest": [
                            {"name": "example::run", "bytes": 40}
                        ]
                    }
                }
            },
            "executable": {
                "kind": "complete",
                "value": {
                    "rust_target": "thumbv7em-none-eabihf",
                    "architecture": "thumbv7em",
                    "byte_order": "little",
                    "entry_point": 155649,
                    "load_segments": [
                        {
                            "file_offset": 0,
                            "run_address": 155648,
                            "run_end": 155690,
                            "load_address": 155648,
                            "load_end": 155690,
                            "file_bytes": 42,
                            "memory_bytes": 42,
                            "alignment": 4,
                            "permissions": ["read", "execute"]
                        }
                    ],
                    "executable_sections": [
                        {
                            "name": ".text",
                            "address": 155648,
                            "end": 155690,
                            "bytes": 42,
                            "alignment": 4,
                            "fingerprint": fingerprint
                        }
                    ],
                    "startup": {
                        "entry_section": ".text",
                        "entry_symbol": "__stext",
                        "anchors": [
                            {
                                "role": "entry-point",
                                "address": 155649,
                                "section": ".text"
                            },
                            {
                                "role": "initial-stack-pointer",
                                "address": 536920064,
                                "section": ".vector_table"
                            },
                            {
                                "role": "reset-vector",
                                "address": 155649,
                                "section": ".vector_table"
                            }
                        ]
                    },
                    "functions": {
                        "normalization": "linked-function-body-sha256-v1",
                        "boundary_count": 1,
                        "boundaries_fingerprint": boundaries_fingerprint,
                        "boundaries_artifact": {
                            "path": "work/t114/function-boundaries.json",
                            "bytes": 128,
                            "fingerprint": fingerprint
                        },
                        "classified_bytes": 42,
                        "unclassified_bytes": 0,
                        "largest": [
                            {
                                "name": "example::run",
                                "address": 155648,
                                "end": 155690,
                                "bytes": 42,
                                "fingerprint": fingerprint
                            }
                        ]
                    },
                    "disassembly": {
                        "adapter": "thumbv7em",
                        "flavor": "llvm-objdump",
                        "program": "llvm-objdump",
                        "version": "LLVM 1.0.0",
                        "executable_bytes": 42,
                        "decoded_bytes": 42,
                        "undecoded_bytes": 0,
                        "instruction_count": 10
                    }
                }
            }
        }
    });
    value["analysis"]["executable"]["value"]["stack"] = stack_value(&fingerprint);
    value["analysis"]["async_memory"] = async_memory_value();
    value
}

fn stack_value(fingerprint: &str) -> Value {
    json!({
        "kind": "partial",
        "value": {
            "frame_source": "llvm-stack-sizes",
            "source_bytes": 5,
            "frame_count": 1,
            "functions_without_frames": 0,
            "largest_frames": [{
                "name": "example::run",
                "address": 155648,
                "bytes": 32
            }],
            "roots": [{
                "role": "startup",
                "name": "example::run",
                "address": 155648
            }],
            "direct_call_count": 0,
            "largest_known_path": {
                "bytes": 32,
                "frames": [{
                    "name": "example::run",
                    "address": 155648,
                    "frame_bytes": 32
                }]
            },
            "limit": {
                "kind": "declared",
                "reservation": "minimum-runtime-stack",
                "bytes": 69632,
                "headroom_bytes": 69600
            },
            "gaps": [{
                "kind": "interrupt-nesting-unmodeled",
                "occurrences": 1
            }],
            "artifact": {
                "path": "work/t114/stack-evidence.json",
                "bytes": 128,
                "fingerprint": fingerprint
            }
        }
    })
}

fn async_memory_value() -> Value {
    json!({
        "kind": "complete",
        "value": {
            "task_pool_accounting": "included-in-static-ram",
            "task_pool_bytes": 32,
            "task_pools": [{
                "task": "example::run",
                "address": 536920064,
                "bytes": 32,
                "section": ".bss"
            }],
            "scenario_futures": {
                "kind": "unavailable",
                "reason": "semantic-harness-not-produced"
            }
        }
    })
}

pub(super) fn retarget_executable(report: &mut ResourceReport, target: &crate::matrix::Target<'_>) {
    if let Evidence::Complete(executable) = &mut report.analysis.executable {
        executable.rust_target = target.adapter().rust_target().to_string();
        executable.architecture =
            super::executable::architecture_identity(target.profile().architecture);
        executable.disassembly.adapter = executable.architecture;
        executable.functions.boundaries_artifact.path =
            format!("work/{}/function-boundaries.json", target.id());
        if let Evidence::Complete(stack) | Evidence::Partial(stack) = &mut executable.stack {
            stack.artifact.path = format!("work/{}/stack-evidence.json", target.id());
            match target.profile().architecture {
                personal_hopspot_memory::ProcessorArchitecture::ThumbV7em => {
                    stack.frame_source = super::model::StackFrameSourceIdentity::LlvmStackSizes;
                    let reservation = target
                        .profile()
                        .runtime_reservations
                        .iter()
                        .find(|reservation| reservation.id.0 == "minimum-runtime-stack")
                        .expect("Arm fixture profile has a stack reservation");
                    stack.limit = super::model::StackLimitIdentity::Declared {
                        reservation: reservation.id.0.to_string(),
                        bytes: reservation.bytes,
                        headroom_bytes: reservation.bytes - stack.largest_known_path.bytes,
                    };
                    stack.gaps.retain(|gap| {
                        gap.kind != super::model::StackAnalysisGapKindIdentity::StackLimitUndeclared
                    });
                }
                personal_hopspot_memory::ProcessorArchitecture::RiscV32Imac
                | personal_hopspot_memory::ProcessorArchitecture::XtensaEsp32S3 => {
                    stack.frame_source = super::model::StackFrameSourceIdentity::DwarfDebugFrame;
                    stack.limit = super::model::StackLimitIdentity::Undeclared;
                    if !stack.gaps.iter().any(|gap| {
                        gap.kind == super::model::StackAnalysisGapKindIdentity::StackLimitUndeclared
                    }) {
                        stack.gaps.push(super::model::StackAnalysisGapIdentity {
                            kind: super::model::StackAnalysisGapKindIdentity::StackLimitUndeclared,
                            occurrences: 1,
                        });
                    }
                }
            }
        }
    }
}
