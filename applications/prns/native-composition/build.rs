use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const CONTRACT_SOURCE: &str = "src/contract.rs";
    const COMPATIBILITY_SOURCE: &str = "../../release/compatibility.json";
    println!("cargo:rerun-if-changed={CONTRACT_SOURCE}");
    println!("cargo:rerun-if-changed={COMPATIBILITY_SOURCE}");

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cargo did not provide CARGO_MANIFEST_DIR",
        )
    })?);
    let app_fingerprint = fingerprint(&std::fs::read(manifest.join(CONTRACT_SOURCE))?);
    let compatibility: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest.join(COMPATIBILITY_SOURCE))?)?;
    let host_fingerprint = compatibility
        .pointer("/prns/hostContract/fingerprint")
        .and_then(serde_json::Value::as_str)
        .filter(|value| valid_host_fingerprint(value))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "release/compatibility.json must contain a prns-host/v1 hexadecimal fingerprint",
            )
        })?;
    println!(
        "cargo:rustc-env=PRNS_APP_CONTRACT_FINGERPRINT=prns-app-native/local-node-1/{:016x}",
        app_fingerprint
    );
    println!("cargo:rustc-env=PRNS_HOST_CONTRACT_FINGERPRINT={host_fingerprint}");
    Ok(())
}

fn valid_host_fingerprint(value: &str) -> bool {
    value.strip_prefix("prns-host/v1/").is_some_and(|digest| {
        digest.len() == 16 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn fingerprint(source: &[u8]) -> u64 {
    source.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
