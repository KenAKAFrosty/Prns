use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let memory = if env::var_os("CARGO_FEATURE_BOARD_T114").is_some() {
        "memory-t114.x"
    } else {
        "memory-t-echo.x"
    };
    fs::copy(memory, out.join("board-memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=memory-t-echo.x");
    println!("cargo:rerun-if-changed=memory-t114.x");
    println!("cargo:rerun-if-changed=build.rs");
}
