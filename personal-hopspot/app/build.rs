use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // app/memory.x overrides esp-hal's bundled esp32s3 memory.x: the linker's `INCLUDE memory.x`
    // (from esp-hal's linkall.x) resolves it from the package root, ahead of esp-hal's copy. It
    // raises ORIGIN(dram2_seg) so the core-0 construction stack grows into the reclaimed heap
    // region — needed for the full WiFi+LoRa+BLE coex firmware, harmless to the WiFi-only and
    // BLE-only builds (no BT reserve / no WiFi controller leaves them DRAM to spare). Copied to
    // OUT_DIR + put on the link search path as the explicit mechanism; rerun-if-changed relinks
    // when memory.x is edited.
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
}
