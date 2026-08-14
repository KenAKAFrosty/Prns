use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let memory = match (
        env::var_os("CARGO_FEATURE_SOFTDEVICE_S140_V6").is_some(),
        env::var_os("CARGO_FEATURE_SOFTDEVICE_S140_V7").is_some(),
    ) {
        (true, false) => "memory-s140-v6.x",
        (false, true) => "memory-s140-v7.x",
        _ => panic!("select exactly one S140 compatibility feature"),
    };
    fs::copy(memory, out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory-s140-v6.x");
    println!("cargo:rerun-if-changed=memory-s140-v7.x");
    println!("cargo:rerun-if-changed=build.rs");
}
