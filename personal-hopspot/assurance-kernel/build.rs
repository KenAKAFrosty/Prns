use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("arm") {
        return Ok(());
    }
    let output = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("OUT_DIR is missing"))?;
    fs::write(output.join("memory.x"), include_bytes!("memory.x"))?;
    println!("cargo:rustc-link-search={}", output.display());
    println!("cargo:rustc-link-arg-bin=assurance-thumbv7em=-Tlink.x");
    println!("cargo:rerun-if-changed=memory.x");
    Ok(())
}
