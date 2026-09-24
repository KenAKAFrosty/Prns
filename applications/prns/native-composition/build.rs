use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const CONTRACT_SOURCES: &[&str] = &[
        "src/contract.rs",
        "src/contract/remote_management.rs",
        "src/contract/remote_wifi.rs",
    ];
    for source in CONTRACT_SOURCES {
        println!("cargo:rerun-if-changed={source}");
    }

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cargo did not provide CARGO_MANIFEST_DIR",
        )
    })?);
    let mut contract = Vec::new();
    for source in CONTRACT_SOURCES {
        contract.extend_from_slice(source.as_bytes());
        contract.push(0);
        contract.extend_from_slice(&std::fs::read(manifest.join(source))?);
        contract.push(0);
    }
    let app_fingerprint = fingerprint(&contract);
    println!(
        "cargo:rustc-env=PRNS_APP_CONTRACT_FINGERPRINT=prns-app-native/local-node-1/{:016x}",
        app_fingerprint
    );
    Ok(())
}

fn fingerprint(source: &[u8]) -> u64 {
    source.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
