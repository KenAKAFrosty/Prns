use std::path::Path;

use personal_hopspot_builder::platform::{esp, nrf52840};
use personal_hopspot_builder::{BuildContext, FirmwareEvidence};

use super::{mesh_tower_v2, MatrixError, Target, TargetRecipe};

pub(crate) struct BuildEvidence {
    firmware: FirmwareEvidence,
    artifacts: Vec<ArtifactEvidence>,
}

pub(crate) struct ArtifactEvidence {
    path: String,
    bytes: u64,
}

impl Target<'_> {
    pub(crate) fn build(&self, context: &BuildContext<'_>) -> Result<BuildEvidence, MatrixError> {
        let result = match self.recipe {
            TargetRecipe::Esp { board, recipe } => {
                esp::build(context, board, recipe).map(|output| BuildEvidence {
                    firmware: output.firmware().clone(),
                    artifacts: output
                        .parts()
                        .iter()
                        .map(|part| ArtifactEvidence {
                            path: part.descriptor().path.clone(),
                            bytes: part.descriptor().size,
                        })
                        .collect(),
                })
            }
            TargetRecipe::Uf2 {
                board,
                recipe,
                variant,
            } => {
                nrf52840::uf2::build(context, board, recipe, variant).map(|output| BuildEvidence {
                    firmware: output.firmware().clone(),
                    artifacts: vec![ArtifactEvidence {
                        path: output.descriptor().path.clone(),
                        bytes: output.descriptor().size,
                    }],
                })
            }
            TargetRecipe::SerialDfu { board, recipe } => {
                nrf52840::serial_dfu::build(context, board, recipe).map(|output| {
                    let manifest = output.manifest();
                    BuildEvidence {
                        firmware: output.firmware().clone(),
                        artifacts: [
                            &manifest.application,
                            &manifest.init_packet,
                            &manifest.recovery.artifact,
                        ]
                        .into_iter()
                        .map(|part| ArtifactEvidence {
                            path: part.path.clone(),
                            bytes: part.size,
                        })
                        .collect(),
                    }
                })
            }
            TargetRecipe::MeshTowerV2 => {
                nrf52840::firmware::build(context, self.id(), mesh_tower_v2::recipe()).map(
                    |firmware| BuildEvidence {
                        firmware,
                        artifacts: Vec::new(),
                    },
                )
            }
        };
        result.map_err(|source| MatrixError::Build {
            target: self.id.clone(),
            source,
        })
    }
}

impl BuildEvidence {
    pub(crate) fn elf(&self) -> &Path {
        self.firmware.elf()
    }

    pub(crate) fn artifacts(&self) -> &[ArtifactEvidence] {
        &self.artifacts
    }

    pub(crate) fn linker_map(&self) -> Option<&Path> {
        self.firmware.linker_map()
    }

    pub(crate) const fn firmware(&self) -> &FirmwareEvidence {
        &self.firmware
    }

    pub(crate) fn package_bytes(&self) -> u64 {
        self.artifacts.iter().map(|artifact| artifact.bytes).sum()
    }
}

impl ArtifactEvidence {
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) const fn bytes(&self) -> u64 {
        self.bytes
    }
}
