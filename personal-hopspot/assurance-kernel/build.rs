use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::PathBuf;

use personal_hopspot_memory::{memory_profile_named, NRF52840_MEMORY_X_BINDING};

const NRF52840_PLATFORM_FEATURE: &str = "CARGO_FEATURE_PLATFORM_NRF52840";
const MEMORY_PROFILE_ENV: &str = "PRNS_ASSURANCE_MEMORY_PROFILE";

fn main() -> Result<(), Box<dyn Error>> {
    let (binary, memory) = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("arm") if env::var_os(NRF52840_PLATFORM_FEATURE).is_some() => {
            nrf52840_platform_memory()?
        }
        Ok("arm") => (
            "assurance-thumbv7em",
            include_bytes!("linker/thumbv7em/memory.x").to_vec(),
        ),
        Ok("riscv32") => (
            "assurance-riscv32imac",
            include_bytes!("linker/riscv32imac/memory.x").to_vec(),
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
    println!("cargo:rerun-if-env-changed={MEMORY_PROFILE_ENV}");
    Ok(())
}

fn nrf52840_platform_memory() -> Result<(&'static str, Vec<u8>), Box<dyn Error>> {
    let profile_id = env::var(MEMORY_PROFILE_ENV)
        .map_err(|_| io::Error::other(format!("{MEMORY_PROFILE_ENV} is missing")))?;
    let profile = memory_profile_named(&profile_id)
        .ok_or_else(|| io::Error::other(format!("unknown embedded memory profile {profile_id}")))?;
    let layout = NRF52840_MEMORY_X_BINDING
        .resolve(profile)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok((
        "assurance-platform-nrf52840",
        layout.to_string().into_bytes(),
    ))
}
