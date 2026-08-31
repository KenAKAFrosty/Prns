use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use personal_hopspot_core::{
    Nrf52840FirmwareMemory, HELTEC_DISPLAY_NRF52840_FIRMWARE_MEMORY, MESH_TOWER_V2_FIRMWARE_MEMORY,
    T1000E_FIRMWARE_MEMORY, T_ECHO_S140_V6_FIRMWARE_MEMORY, T_ECHO_S140_V7_FIRMWARE_MEMORY,
};

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
        (Board::TEcho, Some(Softdevice::S140V6)) => T_ECHO_S140_V6_FIRMWARE_MEMORY,
        (Board::TEcho, Some(Softdevice::S140V7)) => T_ECHO_S140_V7_FIRMWARE_MEMORY,
        (Board::TEcho, None) => panic!("T-Echo requires exactly one S140 compatibility feature"),
        (Board::T096, Some(Softdevice::S140V6)) => HELTEC_DISPLAY_NRF52840_FIRMWARE_MEMORY,
        (Board::T096, None) => panic!("T096 requires softdevice-s140-v6"),
        (Board::T096, Some(Softdevice::S140V7)) => {
            panic!("T096 does not support S140 7.x")
        }
        (Board::T114, Some(Softdevice::S140V6)) => HELTEC_DISPLAY_NRF52840_FIRMWARE_MEMORY,
        (Board::T114, None) => panic!("T114 requires softdevice-s140-v6"),
        (Board::T114, Some(Softdevice::S140V7)) => {
            panic!("T114 does not support S140 7.x")
        }
        (Board::T1000e, None) => T1000E_FIRMWARE_MEMORY,
        (Board::MeshTowerV2, Some(Softdevice::S140V6)) => MESH_TOWER_V2_FIRMWARE_MEMORY,
        (Board::MeshTowerV2, None) => {
            panic!("MeshTower V2 requires softdevice-s140-v6")
        }
        (Board::MeshTowerV2, Some(Softdevice::S140V7)) => {
            panic!("MeshTower V2 does not support S140 7.x")
        }
        (Board::T1000e, Some(_)) => {
            panic!("T1000-E does not support S140 compatibility features")
        }
    };
    write_nrf52840_memory(&out, memory);
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PRNS_BLE_DISCOVERY_GROUP");
}

fn write_nrf52840_memory(out: &Path, layout: Nrf52840FirmwareMemory) {
    let application_flash_origin = layout.application_flash.start;
    let application_flash_bytes = layout.application_flash.byte_len();
    let application_ram_origin = layout.application_ram.start;
    let application_ram_bytes = layout.application_ram.byte_len();
    let minimum_runtime_stack_bytes = layout.minimum_runtime_stack_bytes;
    let memory = format!(
        "APPLICATION_FLASH_ORIGIN = {application_flash_origin:#010X};\nAPPLICATION_FLASH_BYTES = {application_flash_bytes:#X};\nAPPLICATION_RAM_ORIGIN = {application_ram_origin:#010X};\nAPPLICATION_RAM_BYTES = {application_ram_bytes:#X};\n\nMEMORY\n{{\n  FLASH : ORIGIN = APPLICATION_FLASH_ORIGIN, LENGTH = APPLICATION_FLASH_BYTES\n  RAM   : ORIGIN = APPLICATION_RAM_ORIGIN, LENGTH = APPLICATION_RAM_BYTES\n}}\n\nASSERT(\n  ORIGIN(RAM) + LENGTH(RAM) - _stack_end >= {minimum_runtime_stack_bytes},\n  \"nRF52840 static memory leaves too little runtime stack\"\n)\n"
    );
    fs::write(out.join("memory.x"), memory).unwrap();
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
