use super::*;

#[test]
fn a_missing_rns_identity_is_seeded_once_and_then_honored() {
    let dir = std::env::temp_dir().join(std::format!("prns-rpc-compat-id-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let seed = [0x33u8; IDENTITY_SECRET_KEY_LEN];
    let seeded = load_or_seed_rns_rpc_key(&dir, &seed).unwrap();
    assert_eq!(
        seeded.as_bytes(),
        prns_core::crypto::sha256(&seed).as_slice()
    );

    let honored = load_or_seed_rns_rpc_key(&dir, &[0x99u8; IDENTITY_SECRET_KEY_LEN]).unwrap();
    assert_eq!(honored.as_bytes(), seeded.as_bytes());
    let stored = std::fs::read(dir.join("transport_identity")).unwrap();
    assert_eq!(stored, seed);
    let credentials =
        SharedInstanceCredentials::from_identity_secret(&seed).with_rpc_authentication_key(honored);
    assert_eq!(credentials.rpc_key().as_bytes(), seeded.as_bytes());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_malformed_rns_identity_is_reported_instead_of_hashed() {
    let dir =
        std::env::temp_dir().join(std::format!("prns-rpc-malformed-id-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("transport_identity"), [0x44; 63]).unwrap();

    let error = load_or_seed_rns_rpc_key(&dir, &[0x99; IDENTITY_SECRET_KEY_LEN]).unwrap_err();
    assert!(matches!(
        error,
        RnsRpcKeyStorageError::InvalidTransportIdentityLength {
            expected: IDENTITY_SECRET_KEY_LEN,
            actual: 63,
            ..
        }
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_storage_read_error_never_falls_back_to_the_seed() {
    let dir = std::env::temp_dir().join(std::format!("prns-rpc-read-error-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("transport_identity")).unwrap();

    let error = load_or_seed_rns_rpc_key(&dir, &[0x99; IDENTITY_SECRET_KEY_LEN]).unwrap_err();
    assert!(matches!(
        error,
        RnsRpcKeyStorageError::ReadTransportIdentity { .. }
    ));

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
