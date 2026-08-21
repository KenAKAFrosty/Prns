use std::env;
use std::fs;
use std::path::PathBuf;

const BOARD_T_ECHO_FEATURE: &str = "CARGO_FEATURE_BOARD_T_ECHO";
const BOARD_T096_FEATURE: &str = "CARGO_FEATURE_BOARD_T096";
const BOARD_T114_FEATURE: &str = "CARGO_FEATURE_BOARD_T114";
const BOARD_T1000E_FEATURE: &str = "CARGO_FEATURE_BOARD_T1000E";
const BOARD_MESH_TOWER_V2_FEATURE: &str = "CARGO_FEATURE_BOARD_MESH_TOWER_V2";
const S140_V6_FEATURE: &str = "CARGO_FEATURE_SOFTDEVICE_S140_V6";
const S140_V7_FEATURE: &str = "CARGO_FEATURE_SOFTDEVICE_S140_V7";

enum Board {
    TEcho,
    T096,
    T114,
    T1000e,
    MeshTowerV2,
}

enum Softdevice {
    S140V6,
    S140V7,
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let board = selected_board();
    let softdevice = selected_softdevice();
    let memory = match (board, softdevice) {
        (Board::TEcho, Some(Softdevice::S140V6)) => Some("memory-s140-v6.x"),
        (Board::TEcho, Some(Softdevice::S140V7)) => Some("memory-s140-v7.x"),
        (Board::TEcho, None) => panic!("T-Echo requires exactly one S140 compatibility feature"),
        (Board::T096, Some(Softdevice::S140V6)) => Some("memory-t096.x"),
        (Board::T096, None) => panic!("T096 requires softdevice-s140-v6"),
        (Board::T096, Some(Softdevice::S140V7)) => {
            panic!("T096 does not support S140 7.x")
        }
        (Board::T114, None) => Some("memory-t114.x"),
        (Board::T1000e, None) => Some("memory-t1000e.x"),
        (Board::MeshTowerV2, Some(Softdevice::S140V6)) => Some("memory-mesh-tower-v2.x"),
        (Board::MeshTowerV2, None) => {
            panic!("MeshTower V2 requires softdevice-s140-v6")
        }
        (Board::MeshTowerV2, Some(Softdevice::S140V7)) => {
            panic!("MeshTower V2 does not support S140 7.x")
        }
        (Board::T114 | Board::T1000e, Some(_)) => {
            panic!("only T-Echo, T096, and MeshTower V2 support S140 compatibility features")
        }
    };
    if let Some(memory) = memory {
        fs::copy(memory, out.join("memory.x")).unwrap();
    }
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rerun-if-changed=memory-s140-v6.x");
    println!("cargo:rerun-if-changed=memory-s140-v7.x");
    println!("cargo:rerun-if-changed=memory-t096.x");
    println!("cargo:rerun-if-changed=memory-t114.x");
    println!("cargo:rerun-if-changed=memory-t1000e.x");
    println!("cargo:rerun-if-changed=memory-mesh-tower-v2.x");
    println!("cargo:rerun-if-changed=build.rs");
}

fn selected_board() -> Board {
    match (
        env::var_os(BOARD_T_ECHO_FEATURE).is_some(),
        env::var_os(BOARD_T096_FEATURE).is_some(),
        env::var_os(BOARD_T114_FEATURE).is_some(),
        env::var_os(BOARD_T1000E_FEATURE).is_some(),
        env::var_os(BOARD_MESH_TOWER_V2_FEATURE).is_some(),
    ) {
        (true, false, false, false, false) => Board::TEcho,
        (false, true, false, false, false) => Board::T096,
        (false, false, true, false, false) => Board::T114,
        (false, false, false, true, false) => Board::T1000e,
        (false, false, false, false, true) => Board::MeshTowerV2,
        (false, false, false, false, false) => panic!("select exactly one nRF52840 board feature"),
        _ => panic!("nRF52840 board features are mutually exclusive"),
    }
}

fn selected_softdevice() -> Option<Softdevice> {
    match (
        env::var_os(S140_V6_FEATURE).is_some(),
        env::var_os(S140_V7_FEATURE).is_some(),
    ) {
        (false, false) => None,
        (true, false) => Some(Softdevice::S140V6),
        (false, true) => Some(Softdevice::S140V7),
        (true, true) => panic!("S140 compatibility features are mutually exclusive"),
    }
}
