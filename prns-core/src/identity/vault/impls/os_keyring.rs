use keyring::{Entry, Error as KeyringError};

use crate::identity::vault::{
    is_label_byte, IdentityLabel, IdentitySecretKey, IdentityVault, Removal,
};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

pub struct KeyringVault {
    service: KeyringService,
}

/// Shares the identity-label byte law; no length ceiling, since the platform refuses oversized names itself (`KeyringError::TooLong`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyringService(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyringServiceError {
    Empty,
    LeadingNonAlphanumeric,
    InvalidCharacter,
}

impl KeyringService {
    pub fn new(service: &str) -> Result<Self, KeyringServiceError> {
        let bytes = service.as_bytes();
        let Some(&first) = bytes.first() else {
            return Err(KeyringServiceError::Empty);
        };
        if !first.is_ascii_alphanumeric() {
            return Err(KeyringServiceError::LeadingNonAlphanumeric);
        }
        if bytes.iter().any(|byte| !is_label_byte(*byte)) {
            return Err(KeyringServiceError::InvalidCharacter);
        }
        Ok(Self(service.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for KeyringService {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl core::str::FromStr for KeyringService {
    type Err = KeyringServiceError;

    fn from_str(service: &str) -> Result<Self, Self::Err> {
        Self::new(service)
    }
}

impl TryFrom<&str> for KeyringService {
    type Error = KeyringServiceError;

    fn try_from(service: &str) -> Result<Self, Self::Error> {
        Self::new(service)
    }
}

#[derive(Debug)]
pub enum KeyringVaultError {
    Keyring(KeyringError),
    MalformedLength { found: usize },
    BlobOutgrewBuffer { blob_len: usize, buffer_len: usize },
}

impl KeyringVault {
    pub fn new(service: KeyringService) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &KeyringService {
        &self.service
    }

    fn entry(&self, label: &IdentityLabel) -> Result<Entry, KeyringVaultError> {
        Entry::new(self.service.as_str(), label.as_str()).map_err(KeyringVaultError::Keyring)
    }
}

impl IdentityVault for KeyringVault {
    type Error = KeyringVaultError;

    fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
        classify_fetch(self.entry(label)?.get_secret())
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

    fn remove(&mut self, label: &IdentityLabel) -> Result<Removal, Self::Error> {
        classify_removal(self.entry(label)?.delete_credential())
    }

    fn stored_blob_len(&self, label: &IdentityLabel) -> Result<Option<usize>, Self::Error> {
        Ok(classify_blob_fetch(self.entry(label)?.get_secret())?.map(|blob| blob.len()))
    }

    fn load_blob<'b>(
        &self,
        label: &IdentityLabel,
        buf: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Self::Error> {
        let Some(blob) = classify_blob_fetch(self.entry(label)?.get_secret())? else {
            return Ok(None);
        };
        if buf.len() < blob.len() {
            return Err(KeyringVaultError::BlobOutgrewBuffer {
                blob_len: blob.len(),
                buffer_len: buf.len(),
            });
        }
        buf[..blob.len()].copy_from_slice(&blob);
        Ok(Some(&buf[..blob.len()]))
    }

    fn store_blob(&mut self, label: &IdentityLabel, blob: &[u8]) -> Result<(), Self::Error> {
        self.entry(label)?
            .set_secret(blob)
            .map_err(KeyringVaultError::Keyring)
    }
}

fn classify_fetch(
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

fn classify_blob_fetch(
    fetched: Result<Vec<u8>, KeyringError>,
) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringVaultError> {
    match fetched {
        Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(other) => Err(KeyringVaultError::Keyring(other)),
    }
}

fn classify_removal(deleted: Result<(), KeyringError>) -> Result<Removal, KeyringVaultError> {
    match deleted {
        Ok(()) => Ok(Removal::Removed),
        Err(KeyringError::NoEntry) => Ok(Removal::NothingStored),
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
            KeyringVaultError::BlobOutgrewBuffer {
                blob_len,
                buffer_len,
            } => write!(
                formatter,
                "stored blob holds {blob_len} bytes, the buffer holds {buffer_len}"
            ),
        }
    }
}

impl std::error::Error for KeyringVaultError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KeyringVaultError::Keyring(error) => Some(error),
            KeyringVaultError::MalformedLength { .. }
            | KeyringVaultError::BlobOutgrewBuffer { .. } => None,
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
        let secret = classify_fetch(Ok(good_secret())).unwrap().unwrap();
        assert_eq!(secret[0], 0x42);
        assert_eq!(secret[32], 0x43);
    }

    #[test]
    fn an_absent_keyring_entry_is_a_clean_miss() {
        assert!(classify_fetch(Err(KeyringError::NoEntry))
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_secret_of_the_wrong_length_is_malformed_not_truncated() {
        match classify_fetch(Ok(vec![0u8; 10])) {
            Err(KeyringVaultError::MalformedLength { found }) => assert_eq!(found, 10),
            other => panic!("expected MalformedLength, got {other:?}"),
        }
    }

    #[test]
    fn a_platform_error_on_load_surfaces_rather_than_reading_as_a_miss() {
        match classify_fetch(Err(KeyringError::TooLong("secret".into(), 64))) {
            Err(KeyringVaultError::Keyring(KeyringError::TooLong(_, _))) => {}
            other => panic!("expected the keyring error to surface, got {other:?}"),
        }
    }

    #[test]
    fn deleting_a_present_entry_reports_removed() {
        assert_eq!(classify_removal(Ok(())).unwrap(), Removal::Removed);
    }

    #[test]
    fn deleting_an_absent_entry_reports_nothing_stored_not_an_error() {
        assert_eq!(
            classify_removal(Err(KeyringError::NoEntry)).unwrap(),
            Removal::NothingStored
        );
    }

    #[test]
    fn a_platform_error_on_delete_surfaces() {
        match classify_removal(Err(KeyringError::TooLong("user".into(), 64))) {
            Err(KeyringVaultError::Keyring(_)) => {}
            other => panic!("expected the keyring error to surface, got {other:?}"),
        }
    }

    #[test]
    fn a_reverse_domain_service_name_round_trips() {
        let service = KeyringService::new("rs.reticulum.prns").unwrap();
        assert_eq!(service.as_str(), "rs.reticulum.prns");
    }

    #[test]
    fn an_empty_service_name_is_rejected() {
        assert_eq!(
            KeyringService::new("").unwrap_err(),
            KeyringServiceError::Empty
        );
    }

    #[test]
    fn a_service_name_must_start_alphanumeric() {
        assert_eq!(
            KeyringService::new(".hidden").unwrap_err(),
            KeyringServiceError::LeadingNonAlphanumeric
        );
    }

    #[test]
    fn a_service_name_with_a_space_is_an_invalid_character() {
        assert_eq!(
            KeyringService::new("my app").unwrap_err(),
            KeyringServiceError::InvalidCharacter
        );
    }
}
