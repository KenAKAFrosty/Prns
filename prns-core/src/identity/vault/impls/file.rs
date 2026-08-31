use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::identity::vault::{IdentityLabel, IdentitySecretKey, IdentityVault, Removal};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

pub struct FileVault {
    dir: PathBuf,
    dir_ready: bool,
}

#[derive(Debug)]
pub enum FileVaultError {
    Io(std::io::Error),
    MalformedLength { found: u64 },
    BlobOutgrewBuffer { blob_len: usize, buffer_len: usize },
}

impl FileVault {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            dir_ready: false,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, label: &IdentityLabel) -> PathBuf {
        self.dir.join(label.as_str())
    }

    fn ensure_dir(&mut self) -> Result<(), FileVaultError> {
        if self.dir_ready {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        let _ = fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700));
        self.dir_ready = true;
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

        let staged = stage_bytes(&staging_path, secret)
            .and_then(|()| fs::rename(&staging_path, &final_path).map_err(FileVaultError::from));
        if staged.is_err() {
            let _ = fs::remove_file(&staging_path);
        }
        staged
    }

    fn remove(&mut self, label: &IdentityLabel) -> Result<Removal, Self::Error> {
        match fs::remove_file(self.path_for(label)) {
            Ok(()) => Ok(Removal::Removed),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Removal::NothingStored),
            Err(error) => Err(error.into()),
        }
    }

    fn stored_blob_len(&self, label: &IdentityLabel) -> Result<Option<usize>, Self::Error> {
        match fs::metadata(self.path_for(label)) {
            Ok(metadata) => Ok(Some(metadata.len() as usize)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn load_blob<'b>(
        &self,
        label: &IdentityLabel,
        buf: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Self::Error> {
        let mut file = match fs::File::open(self.path_for(label)) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let blob_len = file.metadata()?.len() as usize;
        if buf.len() < blob_len {
            return Err(FileVaultError::BlobOutgrewBuffer {
                blob_len,
                buffer_len: buf.len(),
            });
        }
        file.read_exact(&mut buf[..blob_len])?;
        Ok(Some(&buf[..blob_len]))
    }

    fn store_blob(&mut self, label: &IdentityLabel, blob: &[u8]) -> Result<(), Self::Error> {
        self.ensure_dir()?;
        let final_path = self.path_for(label);
        let staging_path = self.dir.join(format!(
            ".{}.{}.staging",
            label.as_str(),
            std::process::id()
        ));

        let staged = stage_bytes(&staging_path, blob)
            .and_then(|()| fs::rename(&staging_path, &final_path).map_err(FileVaultError::from));
        if staged.is_err() {
            let _ = fs::remove_file(&staging_path);
        }
        staged
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

fn stage_bytes(staging_path: &Path, bytes: &[u8]) -> Result<(), FileVaultError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(staging_path)?;
    file.write_all(bytes)?;
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
            FileVaultError::BlobOutgrewBuffer {
                blob_len,
                buffer_len,
            } => write!(
                formatter,
                "stored blob holds {blob_len} bytes, the buffer holds {buffer_len}"
            ),
        }
    }
}

impl std::error::Error for FileVaultError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FileVaultError::Io(error) => Some(error),
            FileVaultError::MalformedLength { .. } | FileVaultError::BlobOutgrewBuffer { .. } => {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(deprecated)]

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
    fn a_stored_blob_round_trips_and_reports_its_length() {
        let temp = TempDir::new();
        let mut vault = FileVault::new(&temp.path);
        let label = label("ratchets.a1b2");
        let blob = [0x5Eu8; 113];
        vault.store_blob(&label, &blob).unwrap();
        assert_eq!(vault.stored_blob_len(&label).unwrap(), Some(blob.len()));
        let mut buf = [0u8; 256];
        assert_eq!(vault.load_blob(&label, &mut buf).unwrap(), Some(&blob[..]));
        assert_eq!(vault.remove(&label).unwrap(), Removal::Removed);
        assert_eq!(vault.stored_blob_len(&label).unwrap(), None);
        assert_eq!(vault.load_blob(&label, &mut buf).unwrap(), None);
    }

    #[test]
    fn a_blob_buffer_too_short_is_an_error_never_a_truncation() {
        let temp = TempDir::new();
        let mut vault = FileVault::new(&temp.path);
        let label = label("ratchets.a1b2");
        vault.store_blob(&label, &[0x11; 64]).unwrap();
        let mut short = [0u8; 32];
        match vault.load_blob(&label, &mut short) {
            Err(FileVaultError::BlobOutgrewBuffer {
                blob_len,
                buffer_len,
            }) => {
                assert_eq!(blob_len, 64);
                assert_eq!(buffer_len, 32);
            }
            other => panic!("expected BlobOutgrewBuffer, got {other:?}"),
        }
    }

    #[test]
    fn remove_reports_presence_then_absence() {
        let temp = TempDir::new();
        let mut vault = FileVault::new(&temp.path);
        let label = label("primary");
        vault.store(&label, &secret(0x10)).unwrap();
        assert_eq!(vault.remove(&label).unwrap(), Removal::Removed);
        assert_eq!(vault.remove(&label).unwrap(), Removal::NothingStored);
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
