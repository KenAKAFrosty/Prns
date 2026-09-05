use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_memory::{
    memory_profile, EspPartitionCsvError, EspPartitionTable, MemoryProfileId,
    ESP_16_MIB_PARTITION_TABLE, ESP_4_MIB_PARTITION_TABLE, ESP_8_MIB_PARTITION_TABLE,
};
use thiserror::Error;

const PARTITION_ARTIFACTS: [PartitionArtifact; 3] = [
    PartitionArtifact {
        table: &ESP_4_MIB_PARTITION_TABLE,
        relative_path: "personal-hopspot/embedded/esp32/partitions-hopspot-4mb.csv",
    },
    PartitionArtifact {
        table: &ESP_8_MIB_PARTITION_TABLE,
        relative_path: "personal-hopspot/embedded/esp32/partitions-hopspot-8mb.csv",
    },
    PartitionArtifact {
        table: &ESP_16_MIB_PARTITION_TABLE,
        relative_path: "personal-hopspot/embedded/esp32/partitions-hopspot-16mb.csv",
    },
];

struct PartitionArtifact {
    table: &'static EspPartitionTable,
    relative_path: &'static str,
}

pub(crate) enum ContractsMode {
    Check,
    Write,
}

pub(crate) enum ContractOutcome {
    Verified {
        artifact_count: usize,
    },
    Written {
        updated: Vec<PathBuf>,
        unchanged: usize,
    },
}

#[derive(Debug, Error)]
pub(crate) enum ContractError {
    #[error("failed to resolve repository root {path}: {source}")]
    ResolveRepositoryRoot {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("partition artifact {path} has no memory profiles")]
    EmptyProfileSet { path: &'static str },
    #[error("partition artifact {path} references unknown memory profile {profile:?}")]
    UnknownProfile {
        path: &'static str,
        profile: MemoryProfileId,
    },
    #[error("failed to render ESP partitions for {profile:?}: {source}")]
    Render {
        profile: MemoryProfileId,
        #[source]
        source: EspPartitionCsvError,
    },
    #[error(
        "partition artifact {path} differs between memory profiles {canonical:?} and {conflicting:?}"
    )]
    DivergentProfiles {
        path: &'static str,
        canonical: MemoryProfileId,
        conflicting: MemoryProfileId,
    },
    #[error("failed to read generated contract {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write generated contract {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("generated embedded contracts are stale:\n{artifacts}")]
    Stale { artifacts: StaleArtifacts },
}

#[derive(Debug)]
struct StaleArtifact {
    path: PathBuf,
    state: StaleArtifactState,
}

#[derive(Debug)]
enum StaleArtifactState {
    Missing,
    Outdated,
}

#[derive(Debug)]
pub(crate) struct StaleArtifacts(Vec<StaleArtifact>);

impl fmt::Display for StaleArtifacts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for artifact in &self.0 {
            writeln!(
                formatter,
                "  {} ({})",
                artifact.path.display(),
                artifact.state
            )?;
        }
        Ok(())
    }
}

impl fmt::Display for StaleArtifactState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("missing"),
            Self::Outdated => formatter.write_str("outdated"),
        }
    }
}

pub(crate) fn run(root: &Path, mode: ContractsMode) -> Result<ContractOutcome, ContractError> {
    let rendered = render_artifacts(root)?;
    match mode {
        ContractsMode::Check => check_artifacts(rendered),
        ContractsMode::Write => write_artifacts(rendered),
    }
}

fn render_artifacts(root: &Path) -> Result<Vec<RenderedArtifact>, ContractError> {
    PARTITION_ARTIFACTS
        .iter()
        .map(|artifact| {
            Ok(RenderedArtifact {
                path: root.join(artifact.relative_path),
                contents: render_partition_artifact(artifact)?,
            })
        })
        .collect()
}

fn render_partition_artifact(artifact: &PartitionArtifact) -> Result<String, ContractError> {
    let Some((canonical_id, remaining_ids)) = artifact.table.profiles.split_first() else {
        return Err(ContractError::EmptyProfileSet {
            path: artifact.relative_path,
        });
    };
    let canonical = render_profile(artifact, *canonical_id)?;
    for profile_id in remaining_ids {
        let candidate = render_profile(artifact, *profile_id)?;
        if candidate != canonical {
            return Err(ContractError::DivergentProfiles {
                path: artifact.relative_path,
                canonical: *canonical_id,
                conflicting: *profile_id,
            });
        }
    }
    Ok(canonical)
}

fn render_profile(
    artifact: &PartitionArtifact,
    profile_id: MemoryProfileId,
) -> Result<String, ContractError> {
    let profile = memory_profile(profile_id).ok_or(ContractError::UnknownProfile {
        path: artifact.relative_path,
        profile: profile_id,
    })?;
    let mut contents = String::new();
    artifact
        .table
        .write_csv(profile, &mut contents)
        .map_err(|source| ContractError::Render {
            profile: profile_id,
            source,
        })?;
    Ok(contents)
}

struct RenderedArtifact {
    path: PathBuf,
    contents: String,
}

fn check_artifacts(rendered: Vec<RenderedArtifact>) -> Result<ContractOutcome, ContractError> {
    let artifact_count = rendered.len();
    let mut stale = Vec::new();
    for artifact in rendered {
        match fs::read_to_string(&artifact.path) {
            Ok(existing) if existing == artifact.contents => {}
            Ok(_) => stale.push(StaleArtifact {
                path: artifact.path,
                state: StaleArtifactState::Outdated,
            }),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                stale.push(StaleArtifact {
                    path: artifact.path,
                    state: StaleArtifactState::Missing,
                });
            }
            Err(source) => {
                return Err(ContractError::Read {
                    path: artifact.path,
                    source,
                });
            }
        }
    }
    if stale.is_empty() {
        Ok(ContractOutcome::Verified { artifact_count })
    } else {
        Err(ContractError::Stale {
            artifacts: StaleArtifacts(stale),
        })
    }
}

fn write_artifacts(rendered: Vec<RenderedArtifact>) -> Result<ContractOutcome, ContractError> {
    let mut updated = Vec::new();
    let mut unchanged = 0;
    for artifact in rendered {
        match fs::read_to_string(&artifact.path) {
            Ok(existing) if existing == artifact.contents => {
                unchanged += 1;
            }
            Ok(_) => write_artifact(artifact, &mut updated)?,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                write_artifact(artifact, &mut updated)?;
            }
            Err(source) => {
                return Err(ContractError::Read {
                    path: artifact.path,
                    source,
                });
            }
        }
    }
    Ok(ContractOutcome::Written { updated, unchanged })
}

fn write_artifact(
    artifact: RenderedArtifact,
    updated: &mut Vec<PathBuf>,
) -> Result<(), ContractError> {
    fs::write(&artifact.path, artifact.contents).map_err(|source| ContractError::Write {
        path: artifact.path.clone(),
        source,
    })?;
    updated.push(artifact.path);
    Ok(())
}

#[cfg(test)]
mod tests;
