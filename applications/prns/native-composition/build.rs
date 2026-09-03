use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const CONTRACT_SOURCE: &str = "src/contract.rs";
    const HOST_CONTRACT_SOURCE: &str = "../../../prns-host/schema/host-contract-v1.json";
    println!("cargo:rerun-if-changed={CONTRACT_SOURCE}");
    println!("cargo:rerun-if-changed={HOST_CONTRACT_SOURCE}");

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cargo did not provide CARGO_MANIFEST_DIR",
        )
    })?);
    let app_fingerprint = fingerprint(&std::fs::read(manifest.join(CONTRACT_SOURCE))?);
    let host_fingerprint = fingerprint(&std::fs::read(manifest.join(HOST_CONTRACT_SOURCE))?);
    println!(
        "cargo:rustc-env=PRNS_APP_CONTRACT_FINGERPRINT=prns-app-native/local-node-1/{:016x}",
        app_fingerprint
    );
    println!(
        "cargo:rustc-env=PRNS_HOST_CONTRACT_FINGERPRINT=prns-host/v1/{:016x}",
        host_fingerprint
    );
    Ok(())
}

fn fingerprint(source: &[u8]) -> u64 {
    source.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
