use std::fs;

use tempfile::tempdir;

use super::{artifact, validate_capability, RecordError};
use crate::contract::{ComponentId, ScenarioId};

#[test]
fn recorder_accepts_only_declared_miri_capabilities() -> Result<(), Box<dyn std::error::Error>> {
    let sx126x = ComponentId::parse("sx126x")?;
    validate_capability(&sx126x, &ScenarioId::parse("sx126x-state-machine")?)?;
    assert!(matches!(
        validate_capability(&sx126x, &ScenarioId::parse("different-scenario")?),
        Err(RecordError::Capability { .. })
    ));
    Ok(())
}

#[test]
fn recorded_log_is_relative_to_its_evidence_root() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let result = directory.path().join("results/embedded-miri");
    fs::create_dir_all(&result)?;
    let log = result.join("sx126x.log");
    fs::write(&log, b"Miri passed")?;
    let artifact = artifact(&result.canonicalize()?, &log)?;
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
        artifact(directory.path(), &log),
        Err(RecordError::LogFile(_))
    ));
    Ok(())
}
