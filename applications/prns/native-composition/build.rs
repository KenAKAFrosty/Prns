use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const CONTRACT_SOURCE: &str = "src/contract.rs";
    println!("cargo:rerun-if-changed={CONTRACT_SOURCE}");

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cargo did not provide CARGO_MANIFEST_DIR",
        )
    })?);
    let source = std::fs::read(manifest.join(CONTRACT_SOURCE))?;
    let fingerprint = source.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    println!(
        "cargo:rustc-env=PRNS_APP_CONTRACT_FINGERPRINT=prns-app-native/foundation-1/{:016x}",
        fingerprint
    );
    Ok(())
}
