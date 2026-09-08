use personal_hopspot_builder::platform::nrf52840::firmware;
use personal_hopspot_memory::{MemoryProfile, MESH_TOWER_V2};

pub(super) const ID: &str = "mesh-tower-v2";
pub(super) const DISPLAY_NAME: &str = "Heltec MeshTower V2";

pub(super) const fn profile() -> &'static MemoryProfile {
    &MESH_TOWER_V2
}

pub(super) const fn recipe() -> firmware::Recipe<'static> {
    firmware::Recipe {
        package: "t-echo",
        binary: "heltec-mesh-tower-v2",
        rust_target: "thumbv7em-none-eabihf",
        cargo_features: "board-mesh-tower-v2,softdevice-s140-v6",
    }
}
