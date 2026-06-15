use keyring::{Entry, Error as KeyringError};

use crate::identity::vault::{IdentityLabel, IdentitySecretKey, IdentityVault};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

pub struct KeyringVault {
    service: String,
}

#[derive(Debug)]
pub enum KeyringVaultError {
    Keyring(KeyringError),
    MalformedLength { found: usize },
}

impl KeyringVault {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    fn entry(&self, label: &IdentityLabel) -> Result<Entry, KeyringVaultError> {
        Entry::new(&self.service, label.as_str()).map_err(KeyringVaultError::Keyring)
    }
}

impl IdentityVault for KeyringVault {
    type Error = KeyringVaultError;

    fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
        interpret_load(self.entry(label)?.get_secret())
    }

    fn store(
        &mut self,
        label: &IdentityLabel,
        secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<(), Self::Error> {
        self.entry(label)?
            .set_secret(secret)
            .map_err(KeyringVaultError::Keyring)
    }

    fn remove(&mut self, label: &IdentityLabel) -> Result<bool, Self::Error> {
        interpret_remove(self.entry(label)?.delete_credential())
    }
}

fn interpret_load(
    fetched: Result<Vec<u8>, KeyringError>,
) -> Result<Option<IdentitySecretKey>, KeyringVaultError> {
    let raw = match fetched {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(KeyringError::NoEntry) => return Ok(None),
        Err(other) => return Err(KeyringVaultError::Keyring(other)),
    };
    if raw.len() != IDENTITY_SECRET_KEY_LEN {
        return Err(KeyringVaultError::MalformedLength { found: raw.len() });
    }
    let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret.copy_from_slice(&raw);
    Ok(Some(secret))
}

fn interpret_remove(deleted: Result<(), KeyringError>) -> Result<bool, KeyringVaultError> {
    match deleted {
        Ok(()) => Ok(true),
        Err(KeyringError::NoEntry) => Ok(false),
        Err(other) => Err(KeyringVaultError::Keyring(other)),
    }
}

impl core::fmt::Display for KeyringVaultError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KeyringVaultError::Keyring(error) => write!(formatter, "{error}"),
            KeyringVaultError::MalformedLength { found } => write!(
                formatter,
                "keyring secret holds {found} bytes, expected {IDENTITY_SECRET_KEY_LEN}"
            ),
        }
    }
}

impl std::error::Error for KeyringVaultError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KeyringVaultError::Keyring(error) => Some(error),
            KeyringVaultError::MalformedLength { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_secret() -> Vec<u8> {
        let mut bytes = vec![0u8; IDENTITY_SECRET_KEY_LEN];
        bytes[..32].fill(0x42);
        bytes[32..].fill(0x43);
        bytes
    }

    #[test]
    fn a_present_secret_of_the_right_length_loads() {
        let secret = interpret_load(Ok(good_secret())).unwrap().unwrap();
        assert_eq!(secret[0], 0x42);
        assert_eq!(secret[32], 0x43);
    }

    #[test]
    fn an_absent_keyring_entry_is_a_clean_miss() {
        assert!(interpret_load(Err(KeyringError::NoEntry))
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_secret_of_the_wrong_length_is_malformed_not_truncated() {
        match interpret_load(Ok(vec![0u8; 10])) {
            Err(KeyringVaultError::MalformedLength { found }) => assert_eq!(found, 10),
            other => panic!("expected MalformedLength, got {other:?}"),
        }
    }

    #[test]
    fn a_platform_error_on_load_surfaces_rather_than_reading_as_a_miss() {
        match interpret_load(Err(KeyringError::TooLong("secret".into(), 64))) {
            Err(KeyringVaultError::Keyring(KeyringError::TooLong(_, _))) => {}
            other => panic!("expected the keyring error to surface, got {other:?}"),
        }
    }

    #[test]
    fn deleting_a_present_entry_reports_true() {
        assert!(interpret_remove(Ok(())).unwrap());
    }

    #[test]
    fn deleting_an_absent_entry_reports_false_not_an_error() {
        assert!(!interpret_remove(Err(KeyringError::NoEntry)).unwrap());
    }

    #[test]
    fn a_platform_error_on_delete_surfaces() {
        match interpret_remove(Err(KeyringError::TooLong("user".into(), 64))) {
            Err(KeyringVaultError::Keyring(_)) => {}
            other => panic!("expected the keyring error to surface, got {other:?}"),
        }
    }
}
