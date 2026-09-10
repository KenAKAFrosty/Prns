use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let (binary, memory) = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("arm") => (
            "assurance-thumbv7em",
            include_bytes!("linker/thumbv7em/memory.x").as_slice(),
        ),
        Ok("riscv32") => (
            "assurance-riscv32imac",
            include_bytes!("linker/riscv32imac/memory.x").as_slice(),
        ),
        _ => return Ok(()),
    };
    let output = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("OUT_DIR is missing"))?;
    fs::write(output.join("memory.x"), memory)?;
    println!("cargo:rustc-link-search={}", output.display());
    println!("cargo:rustc-link-arg-bin={binary}=-Tlink.x");
    println!("cargo:rerun-if-changed=linker");
    Ok(())
}
