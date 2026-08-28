use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use prns_core::identity::vault::{FileVault, FileVaultError};
use prns_core::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use prns_core::interfaces::bluetooth_auto::{
    decode_persisted_ble_identity, encode_persisted_ble_identity, BleIdentity,
    PersistedBleIdentityError, PERSISTED_BLE_IDENTITY_LEN,
};
use prns_core::interfaces::browser_rendezvous::{
    BrowserRendezvousId, BrowserSelectionSeed, ID_LEN as LOCAL_IDENTITY_LEN,
};
use prns_core::remote_control::{
    RemoteControlNodeIdentityBootstrap, RemoteControlNodeIdentityBootstrapError,
};

pub type RemoteControlFileIdentityBootstrapError =
    RemoteControlNodeIdentityBootstrapError<FileVaultError, OsEntropyError>;

pub struct RemoteControlIdentityDirectory {
    vault: FileVault,
}

impl RemoteControlIdentityDirectory {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            vault: FileVault::new(directory),
        }
    }

    pub fn load_or_generate(
        mut self,
    ) -> Result<RemoteControlNodeIdentityBootstrap, RemoteControlFileIdentityBootstrapError> {
        RemoteControlNodeIdentityBootstrap::load_or_generate(&mut self.vault, fill_os_entropy)
    }
}

#[must_use]
#[expect(
    clippy::expect_used,
    reason = "a host without a functioning CSPRNG cannot mint identities; failing loud beats weak keys"
)]
pub fn generate_identity_secret() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    try_generate_identity_secret().expect("OS CSPRNG must provide identity key material")
}

pub fn try_generate_identity_secret(
) -> Result<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>, OsEntropyError> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    fill_os_entropy(&mut *key)?;
    Ok(key)
}

pub fn fill_os_entropy(bytes: &mut [u8]) -> Result<(), OsEntropyError> {
    getrandom::getrandom(bytes).map_err(OsEntropyError)
}

pub fn load_or_create_ble_identity(path: &Path) -> Result<BleIdentity, LocalIdentityFileError> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return create_ble_identity(path)
        }
        Err(error) => return Err(LocalIdentityFileError::Io(error)),
    };
    let len = file.metadata().map_err(LocalIdentityFileError::Io)?.len();
    if len != PERSISTED_BLE_IDENTITY_LEN as u64 {
        return Err(LocalIdentityFileError::Malformed {
            len,
            expected: PERSISTED_BLE_IDENTITY_LEN,
        });
    }
    let mut record = Zeroizing::new([0u8; PERSISTED_BLE_IDENTITY_LEN]);
    file.read_exact(&mut record[..])
        .map_err(LocalIdentityFileError::Io)?;
    decode_persisted_ble_identity(&record)
        .map_err(LocalIdentityFileError::InvalidBleIdentity)?
        .ok_or(LocalIdentityFileError::EmptyBleIdentity)
}

pub fn load_or_create_browser_rendezvous_id(
    path: &Path,
) -> Result<BrowserRendezvousId, LocalIdentityFileError> {
    load_or_create_local_identity(path).map(BrowserRendezvousId::new)
}

pub fn load_or_create_browser_selection_seed(
    path: &Path,
) -> Result<BrowserSelectionSeed, LocalIdentityFileError> {
    load_or_create_local_identity(path).map(BrowserSelectionSeed::new)
}

fn load_or_create_local_identity(
    path: &Path,
) -> Result<[u8; LOCAL_IDENTITY_LEN], LocalIdentityFileError> {
    match fs::File::open(path) {
        Ok(mut file) => read_local_identity(&mut file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_local_identity(path),
        Err(error) => Err(LocalIdentityFileError::Io(error)),
    }
}

fn read_local_identity(
    file: &mut fs::File,
) -> Result<[u8; LOCAL_IDENTITY_LEN], LocalIdentityFileError> {
    let len = file.metadata().map_err(LocalIdentityFileError::Io)?.len();
    if len != LOCAL_IDENTITY_LEN as u64 {
        return Err(LocalIdentityFileError::Malformed {
            len,
            expected: LOCAL_IDENTITY_LEN,
        });
    }
    let mut bytes = [0u8; LOCAL_IDENTITY_LEN];
    file.read_exact(&mut bytes)
        .map_err(LocalIdentityFileError::Io)?;
    Ok(bytes)
}

fn create_ble_identity(path: &Path) -> Result<BleIdentity, LocalIdentityFileError> {
    let mut bytes = [0u8; LOCAL_IDENTITY_LEN];
    fill_os_entropy(&mut bytes).map_err(LocalIdentityFileError::Entropy)?;
    let identity = BleIdentity::new(bytes);
    let record = Zeroizing::new(encode_persisted_ble_identity(identity));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(LocalIdentityFileError::Io)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            let stored = file
                .write_all(&record[..])
                .and_then(|()| file.sync_all())
                .map_err(LocalIdentityFileError::Io);
            if let Err(error) = stored {
                drop(file);
                let _ = fs::remove_file(path);
                return Err(error);
            }
            Ok(identity)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            load_or_create_ble_identity(path)
        }
        Err(error) => Err(LocalIdentityFileError::Io(error)),
    }
}

fn create_local_identity(path: &Path) -> Result<[u8; LOCAL_IDENTITY_LEN], LocalIdentityFileError> {
    let mut bytes = [0u8; LOCAL_IDENTITY_LEN];
    fill_os_entropy(&mut bytes).map_err(LocalIdentityFileError::Entropy)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(LocalIdentityFileError::Io)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(&bytes).map_err(LocalIdentityFileError::Io)?;
            file.sync_all().map_err(LocalIdentityFileError::Io)?;
            Ok(bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut file = fs::File::open(path).map_err(LocalIdentityFileError::Io)?;
            read_local_identity(&mut file)
        }
        Err(error) => Err(LocalIdentityFileError::Io(error)),
    }
}

/// Load the identity secret at `path`, minting and persisting a fresh one when the file is absent (parent directories created, unix mode `0o600`). A malformed file is refused, never overwritten.
pub fn load_or_create_identity_secret(
    path: &Path,
) -> Result<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>, IdentitySecretFileError> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return create_identity_secret(path)
        }
        Err(error) => return Err(IdentitySecretFileError::Io(error)),
    };
    let len = file.metadata().map_err(IdentitySecretFileError::Io)?.len();
    if len != IDENTITY_SECRET_KEY_LEN as u64 {
        return Err(IdentitySecretFileError::Malformed { len });
    }
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    file.read_exact(&mut *key)
        .map_err(IdentitySecretFileError::Io)?;
    Ok(key)
}

fn create_identity_secret(
    path: &Path,
) -> Result<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>, IdentitySecretFileError> {
    let key = generate_identity_secret();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(IdentitySecretFileError::Io)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            let stored = file
                .write_all(&key[..])
                .and_then(|()| file.sync_all())
                .map_err(IdentitySecretFileError::Io);
            if let Err(error) = stored {
                drop(file);
                let _ = fs::remove_file(path);
                return Err(error);
            }
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            load_or_create_identity_secret(path)
        }
        Err(error) => Err(IdentitySecretFileError::Io(error)),
    }
}

#[derive(Debug)]
pub enum IdentitySecretFileError {
    Io(std::io::Error),
    Malformed { len: u64 },
}

#[derive(Debug)]
pub enum LocalIdentityFileError {
    Io(std::io::Error),
    Malformed { len: u64, expected: usize },
    EmptyBleIdentity,
    InvalidBleIdentity(PersistedBleIdentityError),
    Entropy(OsEntropyError),
}

#[derive(Debug)]
pub struct OsEntropyError(getrandom::Error);

impl core::fmt::Display for OsEntropyError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "OS CSPRNG failed: {}", self.0)
    }
}

impl std::error::Error for OsEntropyError {}

impl core::fmt::Display for IdentitySecretFileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IdentitySecretFileError::Io(error) => write!(f, "identity secret file: {error}"),
            IdentitySecretFileError::Malformed { len } => write!(
                f,
                "identity secret file holds {len} bytes, not the {IDENTITY_SECRET_KEY_LEN} of an X25519 ‖ Ed25519 secret"
            ),
        }
    }
}

impl std::error::Error for IdentitySecretFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IdentitySecretFileError::Io(error) => Some(error),
            IdentitySecretFileError::Malformed { .. } => None,
        }
    }
}

impl core::fmt::Display for LocalIdentityFileError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "local identity file: {error}"),
            Self::Malformed { len, expected } => write!(
                formatter,
                "local identity file holds {len} bytes, not {expected}"
            ),
            Self::EmptyBleIdentity => formatter.write_str("BLE identity file is empty"),
            Self::InvalidBleIdentity(error) => error.fmt(formatter),
            Self::Entropy(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for LocalIdentityFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Malformed { .. } | Self::EmptyBleIdentity => None,
            Self::InvalidBleIdentity(error) => Some(error),
            Self::Entropy(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::identity::vault::IdentityOrigin;

    #[test]
    fn a_fresh_path_mints_persists_and_reloads_the_same_secret() {
        let dir =
            std::env::temp_dir().join(format!("prns-identity-bootstrap-{}", std::process::id()));
        let path = dir.join("deeper").join("transport_identity");
        let _ = fs::remove_dir_all(&dir);

        let minted = load_or_create_identity_secret(&path).unwrap();
        let reloaded = load_or_create_identity_secret(&path).unwrap();
        assert_eq!(&minted[..], &reloaded[..]);
        assert_ne!(&minted[..], &[0u8; IDENTITY_SECRET_KEY_LEN][..]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_file_is_refused_not_overwritten() {
        let dir = std::env::temp_dir().join(format!(
            "prns-identity-bootstrap-malformed-{}",
            std::process::id()
        ));
        let path = dir.join("transport_identity");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, b"short").unwrap();

        assert!(matches!(
            load_or_create_identity_secret(&path),
            Err(IdentitySecretFileError::Malformed { len: 5 })
        ));
        assert_eq!(fs::read(&path).unwrap(), b"short");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_secrets_differ() {
        assert_ne!(
            &generate_identity_secret()[..],
            &generate_identity_secret()[..],
        );
    }

    #[test]
    fn local_service_identities_are_stable_and_independent() {
        let dir =
            std::env::temp_dir().join(format!("prns-local-identities-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let ble_path = dir.join("ble_identity");
        let browser_path = dir.join("browser_rendezvous_id");
        let selection_path = dir.join("browser_selection_seed");
        let transport_path = dir.join("transport_identity");
        let ble = load_or_create_ble_identity(&ble_path).unwrap();
        let browser = load_or_create_browser_rendezvous_id(&browser_path).unwrap();
        let selection = load_or_create_browser_selection_seed(&selection_path).unwrap();
        let transport = load_or_create_identity_secret(&transport_path).unwrap();

        assert_eq!(load_or_create_ble_identity(&ble_path).unwrap(), ble);
        assert_eq!(
            load_or_create_browser_rendezvous_id(&browser_path).unwrap(),
            browser
        );
        assert_eq!(
            load_or_create_browser_selection_seed(&selection_path).unwrap(),
            selection
        );
        assert_ne!(ble.as_bytes(), browser.as_bytes());
        assert_ne!(ble.as_bytes(), selection.as_bytes());
        assert_ne!(browser.as_bytes(), selection.as_bytes());
        assert_ne!(ble.as_bytes().as_slice(), &transport[..LOCAL_IDENTITY_LEN]);
        assert_ne!(
            browser.as_bytes().as_slice(),
            &transport[..LOCAL_IDENTITY_LEN]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_local_service_identities_are_refused() {
        let dir = std::env::temp_dir().join(format!(
            "prns-local-identities-malformed-{}",
            std::process::id()
        ));
        let path = dir.join("ble_identity");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, [0u8; LOCAL_IDENTITY_LEN - 1]).unwrap();

        assert!(matches!(
            load_or_create_ble_identity(&path),
            Err(LocalIdentityFileError::Malformed { len, .. })
                if len == (LOCAL_IDENTITY_LEN - 1) as u64
        ));
        assert_eq!(fs::read(&path).unwrap(), [0u8; LOCAL_IDENTITY_LEN - 1]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_control_identity_directory_persists_a_distinct_pair() {
        let dir = std::env::temp_dir().join(format!(
            "prns-remote-control-identities-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);

        let first = RemoteControlIdentityDirectory::new(&dir)
            .load_or_generate()
            .unwrap();
        let first_identities = first.secrets().identities();
        assert_eq!(
            (first.origins().controller(), first.origins().target()),
            (IdentityOrigin::Generated, IdentityOrigin::Generated)
        );
        assert_ne!(
            first_identities.controller().identity_hash(),
            first_identities.target().identity_hash()
        );

        let second = RemoteControlIdentityDirectory::new(&dir)
            .load_or_generate()
            .unwrap();
        assert_eq!(second.secrets().identities(), first_identities);
        assert_eq!(
            (second.origins().controller(), second.origins().target()),
            (IdentityOrigin::Loaded, IdentityOrigin::Loaded)
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_remote_control_identity_is_refused_unchanged() {
        let dir = std::env::temp_dir().join(format!(
            "prns-remote-control-identities-malformed-{}",
            std::process::id()
        ));
        let target = dir.join("target");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&target, b"short").unwrap();

        assert!(matches!(
            RemoteControlIdentityDirectory::new(&dir).load_or_generate(),
            Err(RemoteControlNodeIdentityBootstrapError::TargetLoad(
                FileVaultError::MalformedLength { found: 5 }
            ))
        ));
        assert_eq!(fs::read(&target).unwrap(), b"short");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unusable_remote_control_identity_directory_is_a_typed_failure() {
        let path = std::env::temp_dir().join(format!(
            "prns-remote-control-identities-unusable-{}",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        fs::write(&path, b"not a directory").unwrap();

        assert!(matches!(
            RemoteControlIdentityDirectory::new(&path).load_or_generate(),
            Err(RemoteControlNodeIdentityBootstrapError::ControllerLoad(
                FileVaultError::Io(_)
            ))
        ));

        let _ = fs::remove_file(&path);
    }
}
