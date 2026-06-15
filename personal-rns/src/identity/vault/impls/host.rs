use std::path::PathBuf;

use super::file::{read_identity_file, FileVaultError};
use crate::identity::vault::{IdentityLabel, IdentitySecretKey, IdentityVault};
use crate::identity::IDENTITY_SECRET_KEY_LEN;

pub struct HostVault<P: IdentityVault> {
    primary: P,
    reticulum_sources: Vec<ReticulumSource>,
}

struct ReticulumSource {
    label: IdentityLabel,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostLoadSource {
    Primary,
    Reticulum,
}

#[derive(Debug)]
pub enum HostVaultError<E> {
    Primary(E),
    Reticulum(FileVaultError),
}

impl<P: IdentityVault> HostVault<P> {
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
    ) -> Result<Option<(IdentitySecretKey, HostLoadSource)>, HostVaultError<P::Error>> {
        if let Some(secret) = self.primary.load(label).map_err(HostVaultError::Primary)? {
            return Ok(Some((secret, HostLoadSource::Primary)));
        }
        for source in &self.reticulum_sources {
            if &source.label != label {
                continue;
            }
            if let Some(secret) =
                read_identity_file(&source.path).map_err(HostVaultError::Reticulum)?
            {
                return Ok(Some((secret, HostLoadSource::Reticulum)));
            }
        }
        Ok(None)
    }
}

impl<P: IdentityVault> IdentityVault for HostVault<P> {
    type Error = HostVaultError<P::Error>;

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
            .map_err(HostVaultError::Primary)
    }

    fn remove(&mut self, label: &IdentityLabel) -> Result<bool, Self::Error> {
        self.primary.remove(label).map_err(HostVaultError::Primary)
    }
}

impl<E: core::fmt::Display> core::fmt::Display for HostVaultError<E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HostVaultError::Primary(error) => write!(formatter, "{error}"),
            HostVaultError::Reticulum(error) => {
                write!(formatter, "reading the Reticulum identity: {error}")
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for HostVaultError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HostVaultError::Primary(error) => Some(error),
            HostVaultError::Reticulum(error) => Some(error),
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

        fn remove(&mut self, label: &IdentityLabel) -> Result<bool, Self::Error> {
            Ok(self.entries.remove(label.as_str()).is_some())
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
                "prns-hostvault-{}-{}",
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
        let vault =
            HostVault::new(primary).adopting(label("primary"), reticulum.path().to_path_buf());

        let (secret, source) = vault.load_reporting(&label("primary")).unwrap().unwrap();
        assert_eq!(source, HostLoadSource::Primary);
        assert_eq!(secret[0], 0x01);
    }

    #[test]
    fn a_primary_miss_adopts_the_reticulum_identity_read_through() {
        let reticulum = TempFile::new("identity");
        let inherited = secret(0x5E);
        reticulum.write_reticulum_identity(&inherited);
        let vault = HostVault::new(MemoryVault::default())
            .adopting(label("primary"), reticulum.path().to_path_buf());

        let (secret, source) = vault.load_reporting(&label("primary")).unwrap().unwrap();
        assert_eq!(source, HostLoadSource::Reticulum);
        assert_eq!(*secret, inherited);
    }

    #[test]
    fn adoption_never_writes_the_reticulum_identity_back_into_the_primary() {
        let reticulum = TempFile::new("identity");
        reticulum.write_reticulum_identity(&secret(0x5E));
        let vault = HostVault::new(MemoryVault::default())
            .adopting(label("primary"), reticulum.path().to_path_buf());

        vault.load(&label("primary")).unwrap().unwrap();
        assert!(vault.primary().load(&label("primary")).unwrap().is_none());
    }

    #[test]
    fn adoption_only_answers_for_the_label_it_was_registered_under() {
        let reticulum = TempFile::new("identity");
        reticulum.write_reticulum_identity(&secret(0x5E));
        let vault = HostVault::new(MemoryVault::default())
            .adopting(label("primary"), reticulum.path().to_path_buf());

        assert!(vault.load(&label("lxmf")).unwrap().is_none());
    }

    #[test]
    fn with_no_reticulum_present_a_miss_stays_a_miss() {
        let vault = HostVault::new(MemoryVault::default()).adopting(
            label("primary"),
            PathBuf::from("/nonexistent/reticulum/identity"),
        );
        assert!(vault.load(&label("primary")).unwrap().is_none());
    }

    #[test]
    fn load_or_generate_persists_a_fresh_identity_into_the_primary_not_reticulum() {
        let reticulum = TempFile::new("identity");
        let mut vault = HostVault::new(MemoryVault::default())
            .adopting(label("primary"), reticulum.path().to_path_buf());
        let fill = |bytes: &mut [u8]| bytes.fill(0x33);

        let (_minted, origin) = load_or_generate(&mut vault, &label("primary"), fill).unwrap();
        assert_eq!(origin, IdentityOrigin::Generated);
        assert!(vault.primary().load(&label("primary")).unwrap().is_some());
        assert!(!reticulum.path().exists());
    }
}
