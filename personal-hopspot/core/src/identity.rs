use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityStorageName(&'static str);

impl IdentityStorageName {
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

pub const NODE_IDENTITY_STORAGE: IdentityStorageName = IdentityStorageName("transport_identity");
pub const BLE_IDENTITY_STORAGE: IdentityStorageName = IdentityStorageName("ble_identity");

pub struct HopspotNodeIdentity {
    secret: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
}

impl HopspotNodeIdentity {
    pub fn new(secret: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> Self {
        Self { secret }
    }

    pub fn secret(&self) -> &[u8; IDENTITY_SECRET_KEY_LEN] {
        &self.secret
    }

    pub fn transport_secret(&self) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
        self.secret.clone()
    }

    pub fn into_destination_secret(self) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
        self.secret
    }
}

#[derive(Debug)]
pub enum IdentityPersistence<E> {
    Loaded,
    Created,
    Recovered(E),
    Ephemeral(E),
}

impl<E> IdentityPersistence<E> {
    pub const fn is_recovered(&self) -> bool {
        matches!(self, Self::Recovered(_))
    }

    pub const fn is_ephemeral(&self) -> bool {
        matches!(self, Self::Ephemeral(_))
    }
}

pub struct IdentityBootstrap<I, E> {
    identity: I,
    persistence: IdentityPersistence<E>,
}

impl<I, E> IdentityBootstrap<I, E> {
    fn new(identity: I, persistence: IdentityPersistence<E>) -> Self {
        Self {
            identity,
            persistence,
        }
    }

    pub fn persistence(&self) -> &IdentityPersistence<E> {
        &self.persistence
    }

    pub fn identity(&self) -> &I {
        &self.identity
    }

    pub fn into_identity(self) -> I {
        self.identity
    }

    pub fn loaded(identity: I) -> Self {
        Self::new(identity, IdentityPersistence::Loaded)
    }

    pub fn created(identity: I) -> Self {
        Self::new(identity, IdentityPersistence::Created)
    }

    pub fn recovered(identity: I, cause: E) -> Self {
        Self::new(identity, IdentityPersistence::Recovered(cause))
    }

    pub fn ephemeral(identity: I, cause: E) -> Self {
        Self::new(identity, IdentityPersistence::Ephemeral(cause))
    }
}

#[cfg(feature = "host")]
pub fn generate_host_node_identity() -> HopspotNodeIdentity {
    HopspotNodeIdentity::new(personal_rns::runtime::generate_identity_secret())
}

#[cfg(feature = "host")]
pub fn generate_host_ble_identity() -> personal_rns::interfaces::bluetooth_auto::BleIdentity {
    use personal_rns::interfaces::bluetooth_auto::BLE_IDENTITY_LEN;

    let secret = personal_rns::runtime::generate_identity_secret();
    let mut bytes = [0u8; BLE_IDENTITY_LEN];
    bytes.copy_from_slice(&secret[..BLE_IDENTITY_LEN]);
    personal_rns::interfaces::bluetooth_auto::BleIdentity::new(bytes)
}

#[cfg(feature = "host")]
pub fn load_host_node_identity(
    path: &std::path::Path,
) -> IdentityBootstrap<HopspotNodeIdentity, personal_rns::runtime::IdentitySecretFileError> {
    use personal_rns::runtime::{load_or_create_identity_secret, IdentitySecretFileError};

    let existed = match path.try_exists() {
        Ok(existed) => existed,
        Err(error) => {
            return IdentityBootstrap::new(
                generate_host_node_identity(),
                IdentityPersistence::Ephemeral(IdentitySecretFileError::Io(error)),
            );
        }
    };
    match load_or_create_identity_secret(path) {
        Ok(secret) => IdentityBootstrap::new(
            HopspotNodeIdentity::new(secret),
            if existed {
                IdentityPersistence::Loaded
            } else {
                IdentityPersistence::Created
            },
        ),
        Err(cause @ IdentitySecretFileError::Malformed { .. }) => {
            if let Err(error) = std::fs::remove_file(path) {
                return IdentityBootstrap::new(
                    generate_host_node_identity(),
                    IdentityPersistence::Ephemeral(IdentitySecretFileError::Io(error)),
                );
            }
            match load_or_create_identity_secret(path) {
                Ok(secret) => IdentityBootstrap::new(
                    HopspotNodeIdentity::new(secret),
                    IdentityPersistence::Recovered(cause),
                ),
                Err(error) => IdentityBootstrap::new(
                    generate_host_node_identity(),
                    IdentityPersistence::Ephemeral(error),
                ),
            }
        }
        Err(error) => IdentityBootstrap::new(
            generate_host_node_identity(),
            IdentityPersistence::Ephemeral(error),
        ),
    }
}

#[cfg(feature = "host")]
pub fn load_host_ble_identity(
    path: &std::path::Path,
) -> IdentityBootstrap<
    personal_rns::interfaces::bluetooth_auto::BleIdentity,
    personal_rns::runtime::LocalIdentityFileError,
> {
    use personal_rns::runtime::{load_or_create_ble_identity, LocalIdentityFileError};

    let existed = match path.try_exists() {
        Ok(existed) => existed,
        Err(error) => {
            return IdentityBootstrap::new(
                generate_host_ble_identity(),
                IdentityPersistence::Ephemeral(LocalIdentityFileError::Io(error)),
            );
        }
    };
    match load_or_create_ble_identity(path) {
        Ok(identity) => IdentityBootstrap::new(
            identity,
            if existed {
                IdentityPersistence::Loaded
            } else {
                IdentityPersistence::Created
            },
        ),
        Err(
            cause @ (LocalIdentityFileError::Malformed { .. }
            | LocalIdentityFileError::EmptyBleIdentity
            | LocalIdentityFileError::InvalidBleIdentity(_)),
        ) => {
            if let Err(error) = std::fs::remove_file(path) {
                return IdentityBootstrap::new(
                    generate_host_ble_identity(),
                    IdentityPersistence::Ephemeral(LocalIdentityFileError::Io(error)),
                );
            }
            match load_or_create_ble_identity(path) {
                Ok(identity) => {
                    IdentityBootstrap::new(identity, IdentityPersistence::Recovered(cause))
                }
                Err(error) => IdentityBootstrap::new(
                    generate_host_ble_identity(),
                    IdentityPersistence::Ephemeral(error),
                ),
            }
        }
        Err(error) => IdentityBootstrap::new(
            generate_host_ble_identity(),
            IdentityPersistence::Ephemeral(error),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_node_identity_supplies_transport_and_destination() {
        let secret = Zeroizing::new([0x5a; IDENTITY_SECRET_KEY_LEN]);
        let identity = HopspotNodeIdentity::new(secret);
        let transport = identity.transport_secret();
        let destination = identity.into_destination_secret();
        assert_eq!(&transport[..], &destination[..]);
    }

    #[cfg(feature = "host")]
    mod host {
        use super::*;
        use std::format;
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU32, Ordering};

        struct TempDir(PathBuf);

        impl TempDir {
            fn new() -> Self {
                static COUNTER: AtomicU32 = AtomicU32::new(0);
                let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
                Self(std::env::temp_dir().join(format!(
                    "hopspot-identities-{}-{unique}",
                    std::process::id()
                )))
            }

            fn identity_path(&self, name: IdentityStorageName) -> PathBuf {
                self.0.join(name.as_str())
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn a_node_identity_is_created_then_loaded() {
            let dir = TempDir::new();
            let path = dir.identity_path(NODE_IDENTITY_STORAGE);
            let created = load_host_node_identity(&path);
            assert!(matches!(
                created.persistence(),
                IdentityPersistence::Created
            ));
            let created = created.into_identity();
            let loaded = load_host_node_identity(&path);
            assert!(matches!(loaded.persistence(), IdentityPersistence::Loaded));
            assert_eq!(created.secret(), loaded.into_identity().secret());
        }

        #[test]
        fn a_malformed_node_identity_is_recovered() {
            let dir = TempDir::new();
            let path = dir.identity_path(NODE_IDENTITY_STORAGE);
            std::fs::create_dir_all(&dir.0).unwrap();
            std::fs::write(&path, b"broken").unwrap();
            let recovered = load_host_node_identity(&path);
            assert!(matches!(
                recovered.persistence(),
                IdentityPersistence::Recovered(_)
            ));
            assert_eq!(
                std::fs::metadata(path).unwrap().len(),
                IDENTITY_SECRET_KEY_LEN as u64
            );
        }

        #[test]
        fn a_malformed_ble_identity_is_recovered() {
            let dir = TempDir::new();
            let path = dir.identity_path(BLE_IDENTITY_STORAGE);
            std::fs::create_dir_all(&dir.0).unwrap();
            std::fs::write(&path, b"broken").unwrap();
            let recovered = load_host_ble_identity(&path);
            assert!(matches!(
                recovered.persistence(),
                IdentityPersistence::Recovered(_)
            ));
            assert_eq!(
                std::fs::metadata(path).unwrap().len(),
                personal_rns::interfaces::bluetooth_auto::PERSISTED_BLE_IDENTITY_LEN as u64
            );
        }

        #[test]
        fn a_same_length_corrupt_ble_identity_is_recovered() {
            let dir = TempDir::new();
            let path = dir.identity_path(BLE_IDENTITY_STORAGE);
            let created = load_host_ble_identity(&path).into_identity();
            let mut record = std::fs::read(&path).unwrap();
            record[24] ^= 0x01;
            std::fs::write(&path, record).unwrap();
            let recovered = load_host_ble_identity(&path);
            assert!(matches!(
                recovered.persistence(),
                IdentityPersistence::Recovered(_)
            ));
            assert_ne!(created, recovered.into_identity());
        }
    }
}
