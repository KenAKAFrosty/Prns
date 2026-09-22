use personal_hopspot_builder::platform::nrf52840::firmware;
use personal_hopspot_builder::LtoMode;
use personal_hopspot_memory::{MemoryProfile, MESH_TOWER_V2, MUZI_BASE_DUO};

const NRF52840_RUST_TARGET: &str = "thumbv7em-none-eabihf";
const NRF52840_PACKAGE: &str = "t-echo";

pub(super) struct BuildOnlyTarget {
    pub id: &'static str,
    pub display_name: &'static str,
    pub profile: &'static MemoryProfile,
    pub recipe: firmware::Recipe<'static>,
}

pub(super) const TARGETS: [BuildOnlyTarget; 2] = [
    BuildOnlyTarget {
        id: "mesh-tower-v2",
        display_name: "Heltec MeshTower V2",
        profile: &MESH_TOWER_V2,
        recipe: firmware::Recipe {
            package: NRF52840_PACKAGE,
            binary: "heltec-mesh-tower-v2",
            rust_target: NRF52840_RUST_TARGET,
            cargo_features: "board-mesh-tower-v2,softdevice-s140-v6",
            lto: LtoMode::Thin,
        },
    },
    BuildOnlyTarget {
        id: "muzi-base-duo",
        display_name: "muzi Base Duo",
        profile: &MUZI_BASE_DUO,
        recipe: firmware::Recipe {
            package: NRF52840_PACKAGE,
            binary: "muzi-base-duo",
            rust_target: NRF52840_RUST_TARGET,
            cargo_features: "board-muzi-base-duo,softdevice-s140-v6",
            lto: LtoMode::Thin,
        },
    },
];
