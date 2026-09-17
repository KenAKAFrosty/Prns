use std::fs;

use tempfile::tempdir;

use super::{
    artifact, artifact_root, record_failure, validate_capability, FailureCapability,
    FailureRecordRequest, RecordError,
};
use crate::contract::{
    ArchitectureId, ComponentId, FailureKind, PlatformId, ProofArtifactKind, ProofFragment,
    ProofKind, RunnerId, ScenarioId, Subject, ToolKind, Verdict,
};

#[test]
fn recorder_accepts_only_declared_miri_capabilities() -> Result<(), Box<dyn std::error::Error>> {
    let sx126x = ComponentId::parse("sx126x")?;
    let subject = Subject::Component(sx126x);
    validate_capability(
        &subject,
        &ScenarioId::parse("sx126x-state-machine")?,
        ProofKind::Miri,
    )?;
    assert!(matches!(
        validate_capability(
            &subject,
            &ScenarioId::parse("different-scenario")?,
            ProofKind::Miri,
        ),
        Err(RecordError::Capability { .. })
    ));
    Ok(())
}

#[test]
fn recorder_accepts_only_declared_target_isa_capabilities() -> Result<(), Box<dyn std::error::Error>>
{
    let subject = Subject::Architecture(ArchitectureId::parse("thumbv7em")?);
    validate_capability(
        &subject,
        &ScenarioId::parse("shared-state-machines")?,
        ProofKind::TargetIsa,
    )?;
    assert!(matches!(
        validate_capability(
            &subject,
            &ScenarioId::parse("shared-state-machines")?,
            ProofKind::Miri,
        ),
        Err(RecordError::Capability { .. })
    ));
    Ok(())
}

#[test]
fn recorder_accepts_declared_platform_pilots() -> Result<(), Box<dyn std::error::Error>> {
    validate_capability(
        &Subject::Platform(PlatformId::parse("nrf52840")?),
        &ScenarioId::parse("platform-startup")?,
        ProofKind::PlatformEmulation,
    )?;
    Ok(())
}

#[test]
fn recorded_log_is_relative_to_its_evidence_root() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let result = directory.path().join("results/embedded-miri");
    fs::create_dir_all(&result)?;
    let log = result.join("sx126x.log");
    fs::write(&log, b"Miri passed")?;
    let artifact = artifact(&result.canonicalize()?, &log, ProofArtifactKind::Log)?;
    assert_eq!(artifact.path.as_str(), "sx126x.log");
    assert_eq!(artifact.bytes, 11);
    Ok(())
}

#[test]
fn empty_logs_are_not_valid_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let log = directory.path().join("empty.log");
    fs::write(&log, b"")?;
    assert!(matches!(
        artifact(directory.path(), &log, ProofArtifactKind::Log),
        Err(RecordError::LogFile(_))
    ));
    Ok(())
}

#[test]
fn transcript_artifacts_retain_their_kind() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let transcript = directory.path().join("transcript.bin");
    fs::write(&transcript, b"target events")?;
    let artifact = artifact(
        &directory.path().canonicalize()?,
        &transcript,
        ProofArtifactKind::Transcript,
    )?;
    assert_eq!(artifact.kind, ProofArtifactKind::Transcript);
    assert_eq!(artifact.path.as_str(), "transcript.bin");
    Ok(())
}

#[test]
fn executable_artifacts_retain_their_kind() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let executable = directory.path().join("kernel.elf");
    fs::write(&executable, b"target machine code")?;
    let artifact = artifact(
        &directory.path().canonicalize()?,
        &executable,
        ProofArtifactKind::Executable,
    )?;
    assert_eq!(artifact.kind, ProofArtifactKind::Executable);
    assert_eq!(artifact.path.as_str(), "kernel.elf");
    Ok(())
}

#[test]
fn relative_artifact_roots_are_canonicalized() -> Result<(), Box<dyn std::error::Error>> {
    let root = artifact_root(std::path::Path::new("."))?;

    assert!(root.is_absolute());
    assert_eq!(root, std::env::current_dir()?.canonicalize()?);
    Ok(())
}

#[test]
fn failed_miri_evidence_round_trips_with_typed_cause() -> Result<(), Box<dyn std::error::Error>> {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = tempdir()?;
    let log = directory.path().join("sx126x.log");
    let output = directory.path().join("sx126x.assurance.json");
    fs::write(&log, b"Miri rejected an exercised operation")?;

    record_failure(
        &repository,
        FailureRecordRequest {
            capability: FailureCapability::Miri {
                component: ComponentId::parse("sx126x")?,
                rustc_version: "rustc 1.96.0-nightly".into(),
                miri_version: "miri 0.1.0".into(),
            },
            scenario: ScenarioId::parse("sx126x-state-machine")?,
            runner: RunnerId::parse("miri-stacked")?,
            kind: FailureKind::StructuralViolation,
            diagnostic: "Miri rejected an exercised operation".into(),
            sources: vec![repository.join("validation/hardening/embedded_miri.py")],
            logs: vec![log],
            output: output.clone(),
        },
    )?;

    let fragment: ProofFragment = serde_json::from_slice(&fs::read(output)?)?;
    assert!(matches!(
        fragment.verdict,
        Verdict::Failed {
            failure: crate::contract::Failure {
                kind: FailureKind::StructuralViolation,
                ref diagnostic,
            },
        } if diagnostic == "Miri rejected an exercised operation"
    ));
    assert_eq!(
        fragment
            .tools
            .iter()
            .map(|identity| identity.kind)
            .collect::<Vec<_>>(),
        vec![ToolKind::Rustc, ToolKind::Miri]
    );
    assert_eq!(fragment.artifacts.len(), 1);
    fragment.validate()?;
    Ok(())
}
