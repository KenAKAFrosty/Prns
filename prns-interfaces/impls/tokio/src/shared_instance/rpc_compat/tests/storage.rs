use super::*;

#[test]
fn it_seeds_a_missing_rns_identity_then_honors_the_seeded_one() {
    let dir = std::env::temp_dir().join(std::format!("prns-rpc-compat-id-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let seed = [0x33u8; 64];
    let seeded = rpc_key_from_rns_identity(&dir, &seed);
    assert_eq!(seeded, prns_core::crypto::sha256(&seed));

    let honored = rpc_key_from_rns_identity(&dir, &[0x99u8; 64]);
    assert_eq!(honored, seeded);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reticulum_storage_dir_uses_the_explicit_config_dir() {
    let _guard = ENV_LOCK.lock().unwrap();
    let _restore = EnvVarRestore::capture("RETICULUM_CONFIG_DIR");
    let config_dir =
        std::env::temp_dir().join(std::format!("prns-reticulum-config-{}", std::process::id()));
    std::env::set_var("RETICULUM_CONFIG_DIR", &config_dir);

    assert_eq!(reticulum_storage_dir(), config_dir.join("storage"));
}
