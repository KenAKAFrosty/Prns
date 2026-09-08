use super::{mesh_tower_v2, TargetRecipe};

pub(crate) struct RecipeIdentity<'a> {
    pub kind: &'static str,
    pub package: &'a str,
    pub binary: &'a str,
    pub features: Vec<&'a str>,
}

impl<'a> TargetRecipe<'a> {
    pub(super) fn identity(self) -> RecipeIdentity<'a> {
        match self {
            Self::Esp { recipe, .. } => RecipeIdentity {
                kind: "esp-sparse-image",
                package: &recipe.package,
                binary: &recipe.binary,
                features: Vec::new(),
            },
            Self::Uf2 {
                recipe, variant, ..
            } => {
                let mut features = vec![recipe.board_feature.as_str()];
                features.extend(variant.application_link.cargo_feature());
                RecipeIdentity {
                    kind: "uf2",
                    package: &recipe.package,
                    binary: &recipe.binary,
                    features,
                }
            }
            Self::SerialDfu { recipe, .. } => RecipeIdentity {
                kind: "nrf-serial-dfu",
                package: &recipe.package,
                binary: &recipe.binary,
                features: vec![recipe.cargo_feature.as_str()],
            },
            Self::MeshTowerV2 => {
                let recipe = mesh_tower_v2::recipe();
                RecipeIdentity {
                    kind: "build-only",
                    package: recipe.package,
                    binary: recipe.binary,
                    features: recipe.cargo_features.split(',').collect(),
                }
            }
        }
    }
}
