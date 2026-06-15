use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::identity::vault::{IdentityLabel, IdentitySecretKey, IdentityVault};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

pub struct FileVault {
    dir: PathBuf,
}

#[derive(Debug)]
pub enum FileVaultError {
    Io(std::io::Error),
    MalformedLength { found: u64 },
}

impl FileVault {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, label: &IdentityLabel) -> PathBuf {
        self.dir.join(label.as_str())
    }

    fn ensure_dir(&self) -> Result<(), FileVaultError> {
        if self.dir.exists() {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        let _ = fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700));
        Ok(())
    }
}

impl IdentityVault for FileVault {
    type Error = FileVaultError;

    fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
        read_identity_file(&self.path_for(label))
    }

    fn store(
        &mut self,
        label: &IdentityLabel,
        secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<(), Self::Error> {
        self.ensure_dir()?;
        let final_path = self.path_for(label);
        let staging_path = self.dir.join(format!(
            ".{}.{}.staging",
            label.as_str(),
            std::process::id()
        ));

        let staged = stage_secret(&staging_path, secret)
            .and_then(|()| fs::rename(&staging_path, &final_path).map_err(FileVaultError::from));
        if staged.is_err() {
            let _ = fs::remove_file(&staging_path);
        }
        staged
    }

    fn remove(&mut self, label: &IdentityLabel) -> Result<bool, Self::Error> {
        match fs::remove_file(self.path_for(label)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

pub fn read_identity_file(path: &Path) -> Result<Option<IdentitySecretKey>, FileVaultError> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let length = file.metadata()?.len();
    if length != IDENTITY_SECRET_KEY_LEN as u64 {
        return Err(FileVaultError::MalformedLength { found: length });
    }
    let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    file.read_exact(&mut secret[..])?;
    Ok(Some(secret))
}

fn stage_secret(
    staging_path: &Path,
    secret: &[u8; IDENTITY_SECRET_KEY_LEN],
) -> Result<(), FileVaultError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(staging_path)?;
    file.write_all(secret)?;
    file.sync_all()?;
    Ok(())
}

impl From<std::io::Error> for FileVaultError {
    fn from(error: std::io::Error) -> Self {
        FileVaultError::Io(error)
    }
}

impl core::fmt::Display for FileVaultError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FileVaultError::Io(error) => write!(formatter, "{error}"),
            FileVaultError::MalformedLength { found } => write!(
                formatter,
                "identity file holds {found} bytes, expected {IDENTITY_SECRET_KEY_LEN}"
            ),
        }
    }
}

impl std::error::Error for FileVaultError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FileVaultError::Io(error) => Some(error),
            FileVaultError::MalformedLength { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vault::{load_or_generate, IdentityOrigin};
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("prns-vault-{}-{}", std::process::id(), unique));
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn label(text: &str) -> IdentityLabel {
        IdentityLabel::new(text).unwrap()
    }

    fn secret(fill: u8) -> [u8; IDENTITY_SECRET_KEY_LEN] {
        let mut bytes = [0u8; IDENTITY_SECRET_KEY_LEN];
        bytes[..32].fill(fill);
        bytes[32..].fill(fill.wrapping_add(1));
        bytes
    }

    #[test]
    fn a_stored_secret_round_trips_byte_for_byte() {
        let temp = TempDir::new();
        let mut vault = FileVault::new(&temp.path);
        let label = label("primary");
        let written = secret(0xA1);
        vault.store(&label, &written).unwrap();
        let read = vault.load(&label).unwrap().unwrap();
        assert_eq!(*read, written);
    }

    #[test]
    fn a_missing_file_is_a_clean_miss_not_an_error() {
        let temp = TempDir::new();
        let vault = FileVault::new(&temp.path);
        assert!(vault.load(&label("absent")).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_stored_secret_is_owner_only_on_disk() {
        let temp = TempDir::new();
        let mut vault = FileVault::new(&temp.path);
        let label = label("primary");
        vault.store(&label, &secret(0x22)).unwrap();
        let mode = fs::metadata(temp.path.join("primary"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_foreign_written_reticulum_file_loads_unchanged() {
        let temp = TempDir::new();
        fs::create_dir_all(&temp.path).unwrap();
        let raw = secret(0x5E);
        fs::write(temp.path.join("identity"), raw).unwrap();
        let vault = FileVault::new(&temp.path);
        let read = vault.load(&label("identity")).unwrap().unwrap();
        assert_eq!(*read, raw);
    }

    #[test]
    fn a_wrong_length_file_is_reported_as_malformed() {
        let temp = TempDir::new();
        fs::create_dir_all(&temp.path).unwrap();
        fs::write(temp.path.join("primary"), [0u8; 10]).unwrap();
        let vault = FileVault::new(&temp.path);
        match vault.load(&label("primary")) {
            Err(FileVaultError::MalformedLength { found }) => assert_eq!(found, 10),
            other => panic!("expected MalformedLength, got {other:?}"),
        }
    }

    #[test]
    fn remove_reports_presence_then_absence() {
        let temp = TempDir::new();
        let mut vault = FileVault::new(&temp.path);
        let label = label("primary");
        vault.store(&label, &secret(0x10)).unwrap();
        assert!(vault.remove(&label).unwrap());
        assert!(!vault.remove(&label).unwrap());
    }

    #[test]
    fn load_or_generate_mints_once_then_loads_through_the_file() {
        let temp = TempDir::new();
        let mut vault = FileVault::new(&temp.path);
        let label = label("primary");
        let fill = |bytes: &mut [u8]| {
            for (offset, byte) in bytes.iter_mut().enumerate() {
                *byte = 0x40u8.wrapping_add(offset as u8);
            }
        };
        let (minted, origin) = load_or_generate(&mut vault, &label, fill).unwrap();
        assert_eq!(origin, IdentityOrigin::Generated);
        let (reloaded, origin) = load_or_generate(&mut vault, &label, fill).unwrap();
        assert_eq!(origin, IdentityOrigin::Loaded);
        assert_eq!(*minted, *reloaded);
    }
}
