use super::*;
use personal_hopspot_memory::{HELTEC_V4, T_BEAM_SUPREME};
use tempfile::TempDir;

const DIVERGENT_PROFILE_IDS: [MemoryProfileId; 2] = [HELTEC_V4.id, T_BEAM_SUPREME.id];
const DIVERGENT_PARTITION_TABLE: EspPartitionTable = EspPartitionTable {
    profiles: &DIVERGENT_PROFILE_IDS,
    partitions: ESP_16_MIB_PARTITION_TABLE.partitions,
};

fn prepare_root() -> TempDir {
    let root = TempDir::new().expect("temporary repository root is available");
    for artifact in PARTITION_ARTIFACTS {
        let path = root.path().join(artifact.relative_path);
        fs::create_dir_all(path.parent().expect("artifact has a parent directory"))
            .expect("artifact parent directory is writable");
    }
    root
}

#[test]
fn missing_contracts_are_reported_without_writes() {
    let root = prepare_root();

    let error = run(root.path(), ContractsMode::Check)
        .err()
        .expect("missing artifacts fail the check");
    if let ContractError::Stale { artifacts } = error {
        assert_eq!(artifacts.0.len(), PARTITION_ARTIFACTS.len());
        assert!(artifacts
            .0
            .iter()
            .all(|artifact| matches!(artifact.state, StaleArtifactState::Missing)));
    } else {
        assert!(matches!(error, ContractError::Stale { .. }));
    }
}

#[test]
fn writes_are_idempotent_and_make_checks_pass() {
    let root = prepare_root();

    let first_write = run(root.path(), ContractsMode::Write).expect("contracts can be written");
    if let ContractOutcome::Written { updated, unchanged } = first_write {
        assert_eq!(updated.len(), PARTITION_ARTIFACTS.len());
        assert_eq!(unchanged, 0);
    } else {
        assert!(matches!(first_write, ContractOutcome::Written { .. }));
    }

    let second_write =
        run(root.path(), ContractsMode::Write).expect("contracts can be written again");
    if let ContractOutcome::Written { updated, unchanged } = second_write {
        assert!(updated.is_empty());
        assert_eq!(unchanged, PARTITION_ARTIFACTS.len());
    } else {
        assert!(matches!(second_write, ContractOutcome::Written { .. }));
    }

    assert!(matches!(
        run(root.path(), ContractsMode::Check),
        Ok(ContractOutcome::Verified { artifact_count })
            if artifact_count == PARTITION_ARTIFACTS.len()
    ));
}

#[test]
fn changed_contracts_are_distinct_from_missing_contracts() {
    let root = prepare_root();
    run(root.path(), ContractsMode::Write).expect("contracts can be written");
    let changed_path = root.path().join(PARTITION_ARTIFACTS[0].relative_path);
    fs::write(&changed_path, "stale\n").expect("generated contract is writable");

    let error = run(root.path(), ContractsMode::Check)
        .err()
        .expect("changed artifact fails the check");
    if let ContractError::Stale { artifacts } = error {
        assert_eq!(artifacts.0.len(), 1);
        assert!(matches!(artifacts.0[0].state, StaleArtifactState::Outdated));
    } else {
        assert!(matches!(error, ContractError::Stale { .. }));
    }
}

#[test]
fn one_artifact_cannot_hide_divergent_profile_layouts() {
    let artifact = PartitionArtifact {
        table: &DIVERGENT_PARTITION_TABLE,
        relative_path: "divergent.csv",
    };

    assert!(matches!(
        render_partition_artifact(&artifact),
        Err(ContractError::DivergentProfiles {
            canonical,
            conflicting,
            ..
        }) if canonical == HELTEC_V4.id && conflicting == T_BEAM_SUPREME.id
    ));
}
