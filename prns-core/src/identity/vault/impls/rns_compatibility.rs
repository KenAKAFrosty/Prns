use std::path::PathBuf;

use super::file::{read_identity_file, FileVaultError};
use crate::identity::vault::{IdentityLabel, IdentitySecretKey, IdentityVault, Removal};
use crate::identity::IDENTITY_SECRET_KEY_LEN;

/// The primary vault answers first; on a miss, registered stock Reticulum identity files are read through, never written back.
pub struct RnsCompatibilityVault<P: IdentityVault> {
    primary: P,
    reticulum_sources: Vec<ReticulumSource>,
}

struct ReticulumSource {
    label: IdentityLabel,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadSource {
    Primary,
    Reticulum,
}

#[derive(Debug)]
pub enum RnsCompatibilityVaultError<E> {
    Primary(E),
    Reticulum(FileVaultError),
}

impl<P: IdentityVault> RnsCompatibilityVault<P> {
    pub fn new(primary: P) -> Self {
        Self {
            primary,
            reticulum_sources: Vec::new(),
        }
    }

    pub fn adopting(mut self, label: IdentityLabel, reticulum_path: impl Into<PathBuf>) -> Self {
        self.reticulum_sources.push(ReticulumSource {
            label,
            path: reticulum_path.into(),
        });
        self
    }

    pub fn primary(&self) -> &P {
        &self.primary
    }

    pub fn load_reporting(
        &self,
        label: &IdentityLabel,
    ) -> Result<Option<(IdentitySecretKey, LoadSource)>, RnsCompatibilityVaultError<P::Error>> {
        if let Some(secret) = self
            .primary
            .load(label)
            .map_err(RnsCompatibilityVaultError::Primary)?
        {
            return Ok(Some((secret, LoadSource::Primary)));
        }
        for source in &self.reticulum_sources {
            if &source.label != label {
                continue;
            }
            if let Some(secret) =
                read_identity_file(&source.path).map_err(RnsCompatibilityVaultError::Reticulum)?
            {
                return Ok(Some((secret, LoadSource::Reticulum)));
            }
        }
        Ok(None)
    }
}

impl<P: IdentityVault> IdentityVault for RnsCompatibilityVault<P> {
    type Error = RnsCompatibilityVaultError<P::Error>;

    fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
        Ok(self.load_reporting(label)?.map(|(secret, _source)| secret))
    }

    fn store(
        &mut self,
        label: &IdentityLabel,
        secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<(), Self::Error> {
        self.primary
            .store(label, secret)
            .map_err(RnsCompatibilityVaultError::Primary)
    }

    fn remove(&mut self, label: &IdentityLabel) -> Result<Removal, Self::Error> {
        self.primary
            .remove(label)
            .map_err(RnsCompatibilityVaultError::Primary)
    }

    fn stored_blob_len(&self, label: &IdentityLabel) -> Result<Option<usize>, Self::Error> {
        self.primary
            .stored_blob_len(label)
            .map_err(RnsCompatibilityVaultError::Primary)
    }

    fn load_blob<'b>(
        &self,
        label: &IdentityLabel,
        buf: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Self::Error> {
        self.primary
            .load_blob(label, buf)
            .map_err(RnsCompatibilityVaultError::Primary)
    }

    fn store_blob(&mut self, label: &IdentityLabel, blob: &[u8]) -> Result<(), Self::Error> {
        self.primary
            .store_blob(label, blob)
            .map_err(RnsCompatibilityVaultError::Primary)
    }
}

impl<E: core::fmt::Display> core::fmt::Display for RnsCompatibilityVaultError<E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RnsCompatibilityVaultError::Primary(error) => write!(formatter, "{error}"),
            RnsCompatibilityVaultError::Reticulum(error) => {
                write!(formatter, "reading the Reticulum identity: {error}")
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for RnsCompatibilityVaultError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RnsCompatibilityVaultError::Primary(error) => Some(error),
            RnsCompatibilityVaultError::Reticulum(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vault::{load_or_generate, IdentityOrigin};
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Default)]
    struct MemoryVault {
        entries: HashMap<String, [u8; IDENTITY_SECRET_KEY_LEN]>,
        blobs: HashMap<String, Vec<u8>>,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum MemoryVaultError {}

    impl IdentityVault for MemoryVault {
        type Error = MemoryVaultError;

        fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
            Ok(self
                .entries
                .get(label.as_str())
                .map(|bytes| IdentitySecretKey::new(*bytes)))
        }

        fn store(
            &mut self,
            label: &IdentityLabel,
            secret: &[u8; IDENTITY_SECRET_KEY_LEN],
        ) -> Result<(), Self::Error> {
            self.entries.insert(label.as_str().to_owned(), *secret);
            Ok(())
        }

        fn remove(&mut self, label: &IdentityLabel) -> Result<Removal, Self::Error> {
            let entry = self.entries.remove(label.as_str());
            let blob = self.blobs.remove(label.as_str());
            Ok(match (entry, blob) {
                (None, None) => Removal::NothingStored,
                _ => Removal::Removed,
            })
        }

        fn stored_blob_len(&self, label: &IdentityLabel) -> Result<Option<usize>, Self::Error> {
            Ok(self.blobs.get(label.as_str()).map(Vec::len))
        }

        fn load_blob<'b>(
            &self,
            label: &IdentityLabel,
            buf: &'b mut [u8],
        ) -> Result<Option<&'b [u8]>, Self::Error> {
            let Some(blob) = self.blobs.get(label.as_str()) else {
                return Ok(None);
            };
            buf[..blob.len()].copy_from_slice(blob);
            Ok(Some(&buf[..blob.len()]))
        }

        fn store_blob(&mut self, label: &IdentityLabel, blob: &[u8]) -> Result<(), Self::Error> {
            self.blobs.insert(label.as_str().to_owned(), blob.to_vec());
            Ok(())
        }
    }

    struct TempFile {
        path: PathBuf,
    }

    impl TempFile {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "prns-rns-compat-vault-{}-{}",
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&dir).unwrap();
            Self {
                path: dir.join(name),
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write_reticulum_identity(&self, secret: &[u8; IDENTITY_SECRET_KEY_LEN]) {
            fs::write(&self.path, secret).unwrap();
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            if let Some(dir) = self.path.parent() {
                let _ = fs::remove_dir_all(dir);
            }
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
    fn the_primary_store_answers_before_any_reticulum_source() {
        let reticulum = TempFile::new("identity");
        reticulum.write_reticulum_identity(&secret(0x9E));
        let mut primary = MemoryVault::default();
        primary.store(&label("primary"), &secret(0x01)).unwrap();
        let vault = RnsCompatibilityVault::new(primary)
            .adopting(label("primary"), reticulum.path().to_path_buf());

        let (secret, source) = vault.load_reporting(&label("primary")).unwrap().unwrap();
        assert_eq!(source, LoadSource::Primary);
        assert_eq!(secret[0], 0x01);
    }

    #[test]
    fn a_primary_miss_adopts_the_reticulum_identity_read_through() {
        let reticulum = TempFile::new("identity");
        let inherited = secret(0x5E);
        reticulum.write_reticulum_identity(&inherited);
        let vault = RnsCompatibilityVault::new(MemoryVault::default())
            .adopting(label("primary"), reticulum.path().to_path_buf());

        let (secret, source) = vault.load_reporting(&label("primary")).unwrap().unwrap();
        assert_eq!(source, LoadSource::Reticulum);
        assert_eq!(*secret, inherited);
    }

    #[test]
    fn adoption_never_writes_the_reticulum_identity_back_into_the_primary() {
        let reticulum = TempFile::new("identity");
        reticulum.write_reticulum_identity(&secret(0x5E));
        let vault = RnsCompatibilityVault::new(MemoryVault::default())
            .adopting(label("primary"), reticulum.path().to_path_buf());

        vault.load(&label("primary")).unwrap().unwrap();
        assert!(vault.primary().load(&label("primary")).unwrap().is_none());
    }

    #[test]
    fn adoption_only_answers_for_the_label_it_was_registered_under() {
        let reticulum = TempFile::new("identity");
        reticulum.write_reticulum_identity(&secret(0x5E));
        let vault = RnsCompatibilityVault::new(MemoryVault::default())
            .adopting(label("primary"), reticulum.path().to_path_buf());

        assert!(vault.load(&label("lxmf")).unwrap().is_none());
    }

    #[test]
    fn with_no_reticulum_present_a_miss_stays_a_miss() {
        let vault = RnsCompatibilityVault::new(MemoryVault::default()).adopting(
            label("primary"),
            PathBuf::from("/nonexistent/reticulum/identity"),
        );
        assert!(vault.load(&label("primary")).unwrap().is_none());
    }

    #[test]
    fn load_or_generate_persists_a_fresh_identity_into_the_primary_not_reticulum() {
        let reticulum = TempFile::new("identity");
        let mut vault = RnsCompatibilityVault::new(MemoryVault::default())
            .adopting(label("primary"), reticulum.path().to_path_buf());
        let fill = |bytes: &mut [u8]| bytes.fill(0x33);

        let (_minted, origin) = load_or_generate(&mut vault, &label("primary"), fill).unwrap();
        assert_eq!(origin, IdentityOrigin::Generated);
        assert!(vault.primary().load(&label("primary")).unwrap().is_some());
        assert!(!reticulum.path().exists());
    }
}
