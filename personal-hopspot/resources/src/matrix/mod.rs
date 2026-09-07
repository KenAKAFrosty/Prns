mod build;
mod mesh_tower_v2;
mod recipe;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use personal_hopspot_builder::architecture::{adapter_for, Adapter};
use personal_hopspot_builder::BuildError;
use personal_hopspot_memory::{MemoryProfile, ValidationError};
use prns_flash_manifest::{
    BoardBuild, BoardCatalog, BoardCatalogEntry, EspBuild, MemoryProfileReferenceError,
    NrfSerialDfuBuild, Uf2Build, Uf2BuildVariant,
};
use thiserror::Error;

pub(crate) use build::BuildEvidence;
pub(crate) use recipe::RecipeIdentity;

pub(crate) struct Matrix<'a> {
    targets: Vec<Target<'a>>,
}

pub(crate) struct Target<'a> {
    id: String,
    display_name: String,
    profile: &'static MemoryProfile,
    adapter: &'static Adapter,
    recipe: TargetRecipe<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetPlatform {
    Esp,
    Nrf52840,
}

#[derive(Clone, Copy)]
enum TargetRecipe<'a> {
    Esp {
        board: &'a BoardCatalogEntry,
        recipe: &'a EspBuild,
    },
    Uf2 {
        board: &'a BoardCatalogEntry,
        recipe: &'a Uf2Build,
        variant: &'a Uf2BuildVariant,
    },
    SerialDfu {
        board: &'a BoardCatalogEntry,
        recipe: &'a NrfSerialDfuBuild,
    },
    MeshTowerV2,
}

#[derive(Debug, Error)]
pub(crate) enum MatrixError {
    #[error("resource target {target:?} has an invalid memory profile: {source}")]
    CatalogProfile {
        target: String,
        #[source]
        source: MemoryProfileReferenceError,
    },
    #[error("build-only resource target {target:?} has an invalid memory profile: {error}")]
    BuildOnlyProfile {
        target: &'static str,
        error: ValidationError,
    },
    #[error("resource target ID {0:?} is duplicated")]
    DuplicateTarget(String),
    #[error("unknown resource target {0:?}")]
    UnknownTarget(String),
    #[error("failed to build resource target {target:?}: {source}")]
    Build {
        target: String,
        #[source]
        source: BuildError,
    },
}

impl<'a> Matrix<'a> {
    pub(crate) fn from_catalog(catalog: &'a BoardCatalog) -> Result<Self, MatrixError> {
        let mut targets = Vec::new();
        for board in &catalog.boards {
            match &board.build {
                BoardBuild::Esp(recipe) => {
                    let memory =
                        recipe
                            .memory_layout()
                            .map_err(|source| MatrixError::CatalogProfile {
                                target: board.slug.clone(),
                                source,
                            })?;
                    targets.push(Target {
                        id: memory.id().0.to_string(),
                        display_name: board.display_name.clone(),
                        profile: memory.profile(),
                        adapter: adapter_for(memory.architecture()),
                        recipe: TargetRecipe::Esp { board, recipe },
                    });
                }
                BoardBuild::Uf2(recipe) => {
                    for variant in &recipe.variants {
                        let memory = variant.memory_layout().map_err(|source| {
                            MatrixError::CatalogProfile {
                                target: variant.memory_profile.as_str().to_string(),
                                source,
                            }
                        })?;
                        targets.push(Target {
                            id: memory.id().0.to_string(),
                            display_name: format!(
                                "{} S140 {}",
                                board.display_name, variant.softdevice_version
                            ),
                            profile: memory.profile(),
                            adapter: adapter_for(memory.architecture()),
                            recipe: TargetRecipe::Uf2 {
                                board,
                                recipe,
                                variant,
                            },
                        });
                    }
                }
                BoardBuild::NrfSerialDfu(recipe) => {
                    let memory =
                        recipe
                            .memory_layout()
                            .map_err(|source| MatrixError::CatalogProfile {
                                target: board.slug.clone(),
                                source,
                            })?;
                    targets.push(Target {
                        id: memory.id().0.to_string(),
                        display_name: board.display_name.clone(),
                        profile: memory.profile(),
                        adapter: adapter_for(memory.architecture()),
                        recipe: TargetRecipe::SerialDfu { board, recipe },
                    });
                }
            }
        }

        let profile = mesh_tower_v2::profile();
        profile
            .validate()
            .map_err(|error| MatrixError::BuildOnlyProfile {
                target: mesh_tower_v2::ID,
                error,
            })?;
        targets.push(Target {
            id: mesh_tower_v2::ID.to_string(),
            display_name: mesh_tower_v2::DISPLAY_NAME.to_string(),
            profile,
            adapter: adapter_for(profile.architecture),
            recipe: TargetRecipe::MeshTowerV2,
        });

        let mut ids = BTreeSet::new();
        for target in &targets {
            if !ids.insert(target.id.as_str()) {
                return Err(MatrixError::DuplicateTarget(target.id.clone()));
            }
        }
        Ok(Self { targets })
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Target<'a>> {
        self.targets.iter()
    }

    pub(crate) fn target(&self, id: &str) -> Result<&Target<'a>, MatrixError> {
        self.targets
            .iter()
            .find(|target| target.id == id)
            .ok_or_else(|| MatrixError::UnknownTarget(id.to_string()))
    }
}

impl Target<'_> {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    pub(crate) const fn profile(&self) -> &'static MemoryProfile {
        self.profile
    }

    pub(crate) const fn adapter(&self) -> &'static Adapter {
        self.adapter
    }

    pub(crate) const fn platform(&self) -> TargetPlatform {
        match self.recipe {
            TargetRecipe::Esp { .. } => TargetPlatform::Esp,
            TargetRecipe::Uf2 { .. }
            | TargetRecipe::SerialDfu { .. }
            | TargetRecipe::MeshTowerV2 => TargetPlatform::Nrf52840,
        }
    }

    pub(crate) fn recipe_identity(&self) -> RecipeIdentity<'_> {
        self.recipe.identity()
    }
}
