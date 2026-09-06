use std::path::Path;

use personal_hopspot_builder::{BuildContext, BuildIntent, BuildVersion, LtoMode};
use personal_hopspot_memory::T114;
use serde_json::{json, Value};

use super::build::build_identity;
use super::contract;
use super::model::{ResourceReport, SCHEMA_VERSION};
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

fn recipe() -> RecipeIdentity<'static> {
    RecipeIdentity {
        kind: "nrf-serial-dfu",
        package: "personal-hopspot-t114",
        binary: "personal-hopspot-t114",
        features: vec!["t114"],
    }
}

fn report_value() -> Value {
    let fingerprint = "a".repeat(64);
    json!({
        "schema_version": SCHEMA_VERSION,
        "target": {
            "id": "t114",
            "display_name": "LILYGO T114",
            "memory_profile": "t114"
        },
        "architecture": {
            "rust_target": "thumbv7em-none-eabihf",
            "adapter": "thumbv7em-rust-lld",
            "linker_flavor": "rust-lld"
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
            "fingerprint": fingerprint,
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
            "runtime_reservations": []
        },
        "status": "success",
        "artifacts": [{"path": "firmware.bin", "bytes": 42}],
        "analysis": {"status": "pending", "linker_map_bytes": 128}
    })
}
