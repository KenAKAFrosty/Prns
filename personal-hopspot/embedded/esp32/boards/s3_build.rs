use std::env;
use std::fs;
use std::path::PathBuf;

pub fn link_memory_layout() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let memory = manifest.join("../../memory-esp32s3.x");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy(&memory, out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed={}", memory.display());
}
