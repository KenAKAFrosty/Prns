use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Only the ESP32-S3 (xtensa) overrides the linker's memory layout. The C6 (riscv32) takes
    // esp-hal's bundled esp32c6 memory.x; a generically-named package-root memory.x would shadow it
    // via the linker's CWD search, so the S3's is memory-esp32s3.x, copied to OUT_DIR for xtensa only.
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("xtensa") {
        return;
    }
    // app/memory.x overrides esp-hal's bundled esp32s3 memory.x: the linker's `INCLUDE memory.x`
    // (from esp-hal's linkall.x) resolves it from the package root, ahead of esp-hal's copy. It
    // raises ORIGIN(dram2_seg) so the core-0 construction stack grows into the reclaimed heap
    // region — needed for the full WiFi+LoRa+BLE coex firmware, harmless to the WiFi-only and
    // BLE-only builds (no BT reserve / no WiFi controller leaves them DRAM to spare). Copied to
    // OUT_DIR + put on the link search path as the explicit mechanism; rerun-if-changed relinks
    // when memory.x is edited.
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory-esp32s3.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory-esp32s3.x");
}
