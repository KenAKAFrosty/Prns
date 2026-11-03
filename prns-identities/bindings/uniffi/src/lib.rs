//! Representation and lifetime adapters for identity owners.
//!
//! Key creation, recovery, validation and publication remain in prns_identities::keys.
//! Default exports are key-only; `catalog` adds a standalone owner factory.
//! Ordinary catalog operations are exported by the canonical identity Client.
#![forbid(unsafe_code)]

use prns_core::identity::IdentityHash;
use prns_core::wire::TRUNCATED_HASH_BYTE_LEN;
use prns_identities::{keys as identity_keys, Error as IdentityError};

uniffi::setup_scaffolding!();
/// Canonical optional card namespace; aggregate images retain its scaffolding.
#[cfg(feature = "contact-card")]
pub use prns_contact_card as contact_card;
#[cfg(feature = "catalog")]
mod catalog;
#[cfg(feature = "catalog")]
pub use catalog::{IdentityCatalogClose, IdentityCatalogOwner};

/// Typed boundary failures. An I/O failure can follow a partially completed
/// creation/publication; preserve the directory and use explicit recovery rather
/// than assuming no state was written. Errors never contain private key bytes.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum KeyError {
    #[error("expected public identity must contain exactly 16 bytes")]
    InvalidIdentityHash,
    #[error("contact card service hash or display name is invalid")]
    InvalidContactCard,
    #[error("key directory is already owned")]
    Busy,
    #[error("published key is missing")]
    Missing,
    #[error("public identity does not match the expected identity")]
    IdentityMismatch,
    #[error("key record is corrupt or incomplete")]
    Corrupt,
    #[error("key path is unsafe")]
    UnsafePath,
    #[error("key path permissions are unsafe")]
    UnsafePermissions,
    #[error("platform does not support the native durability contract")]
    UnsupportedPlatform,
    #[error("key storage I/O failed: {detail}")]
    Io { code: Option<i32>, detail: String },
    #[error("identity entropy failed: {detail}")]
    Entropy { detail: String },
    #[error("unexpected native key invariant failure")]
    Internal,
}

impl From<IdentityError> for KeyError {
    fn from(error: IdentityError) -> Self {
        match error {
            IdentityError::Busy => Self::Busy,
            IdentityError::MissingKey => Self::Missing,
            IdentityError::KeyMismatch => Self::IdentityMismatch,
            IdentityError::Corrupt => Self::Corrupt,
            IdentityError::UnsafePath => Self::UnsafePath,
            IdentityError::UnsafePermissions => Self::UnsafePermissions,
            IdentityError::UnsupportedPlatform => Self::UnsupportedPlatform,
            IdentityError::Io(error) => Self::Io {
                code: error.raw_os_error(),
                detail: error.to_string(),
            },
            IdentityError::Entropy(error) => Self::Entropy {
                detail: error.to_string(),
            },
            IdentityError::Codec(_)
            | IdentityError::Authority(_)
            | IdentityError::Bounds
            | IdentityError::MissingCatalog
            | IdentityError::ImportRequired
            | IdentityError::UnexpectedImport
            | IdentityError::WrongAction
            | IdentityError::UnsupportedCatalogFormat
            | IdentityError::RevisionMismatch { .. }
            | IdentityError::ForeignCatalog => Self::Internal,
        }
    }
}

/// Owns one native key directory. Outstanding KeyLease objects retain its key
/// and exclusive lock after the foreign owner is dropped.
#[derive(uniffi::Object)]
pub struct KeyOwner {
    owner: identity_keys::KeyOwner,
}

#[uniffi::export]
impl KeyOwner {
    #[uniffi::constructor]
    pub fn create_new(path: String) -> Result<Self, KeyError> {
        Ok(Self {
            owner: identity_keys::KeyOwner::create_new(path)?,
        })
    }

    /// Open only the expected published identity. Validate the public hash
    /// representation before native filesystem access; native code checks its
    /// value against the existing key without generation or stage promotion.
    #[uniffi::constructor]
    pub fn open_existing(path: String, expected_identity: Vec<u8>) -> Result<Self, KeyError> {
        let bytes: [u8; TRUNCATED_HASH_BYTE_LEN] = expected_identity
            .try_into()
            .map_err(|_| KeyError::InvalidIdentityHash)?;
        Ok(Self {
            owner: identity_keys::KeyOwner::open_existing(path, IdentityHash::new(bytes))?,
        })
    }

    /// Recover only the uncertain initial creation result. Once its public hash
    /// is recorded, use open_existing with that binding instead.
    #[uniffi::constructor]
    pub fn recover_creation(path: String) -> Result<Self, KeyError> {
        Ok(Self {
            owner: identity_keys::KeyOwner::recover_creation(path)?,
        })
    }

    pub fn identity_hash(&self) -> Vec<u8> {
        self.owner.identity_hash().as_bytes().to_vec()
    }

    pub fn lease(&self) -> KeyLease {
        KeyLease {
            lease: self.owner.lease(),
        }
    }
}

/// An opaque retained native identity capability. Only public identity evidence
/// crosses the language boundary; native domain adapters retain this lease.
#[derive(uniffi::Object)]
pub struct KeyLease {
    lease: identity_keys::KeyLease,
}

#[uniffi::export]
impl KeyLease {
    pub fn identity_hash(&self) -> Vec<u8> {
        self.lease.identity_hash().as_bytes().to_vec()
    }
}

#[cfg(feature = "contact-card")]
#[uniffi::export]
impl KeyLease {
    /// Issue canonical public card bytes using the existing retained key.
    /// No catalog, host, service selection or unrestricted signer is exposed.
    pub fn contact_card(
        &self,
        service_name_hash: Vec<u8>,
        display_name: String,
    ) -> Result<Vec<u8>, KeyError> {
        let hash: prns_identities::contact_card::ServiceNameHash = service_name_hash
            .try_into()
            .map_err(|_| KeyError::InvalidContactCard)?;
        self.lease
            .contact_card(hash, &display_name)
            .map_err(card_error)
    }
}

#[cfg(feature = "contact-card")]
fn card_error(error: prns_identities_core::ApiError) -> KeyError {
    match error {
        prns_identities_core::ApiError::InvalidInput => KeyError::InvalidContactCard,
        // Preserve the distinction between malformed input and an issuer
        // failure; e.g. output allocation failure is not a bad public claim.
        _ => KeyError::Internal,
    }
}

#[cfg(feature = "catalog")]
#[uniffi::export]
impl KeyLease {
    /// Retained import capability; no private key representation is exported.
    pub fn import_key(&self) -> std::sync::Arc<prns_identities::api::ImportKey> {
        std::sync::Arc::new(prns_identities::api::ImportKey::from_lease(
            self.lease.clone(),
        ))
    }
}

impl KeyLease {
    /// Native Rust construction adapters retain the returned lease for every
    /// domain use. This method is deliberately outside the exported impl.
    pub fn key_lease(&self) -> identity_keys::KeyLease {
        self.lease.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_hash_is_rejected_before_filesystem_access() {
        for length in [0, TRUNCATED_HASH_BYTE_LEN - 1, TRUNCATED_HASH_BYTE_LEN + 1] {
            assert!(matches!(
                KeyOwner::open_existing("invalid\0path".into(), vec![0; length]),
                Err(KeyError::InvalidIdentityHash)
            ));
        }
    }

    #[cfg(feature = "contact-card")]
    #[test]
    fn card_issuer_failure_is_not_reported_as_malformed_input() {
        assert!(matches!(
            card_error(prns_identities_core::ApiError::InvalidInput),
            KeyError::InvalidContactCard
        ));
        assert!(matches!(
            card_error(prns_identities_core::ApiError::Storage {
                reason: prns_identities_core::StorageFailure::Bounds,
            }),
            KeyError::Internal
        ));
    }

    #[test]
    fn projection_retains_actionable_native_failure_classes() {
        assert!(matches!(
            KeyError::from(IdentityError::Busy),
            KeyError::Busy
        ));
        assert!(matches!(
            KeyError::from(IdentityError::MissingKey),
            KeyError::Missing
        ));
        assert!(matches!(
            KeyError::from(IdentityError::KeyMismatch),
            KeyError::IdentityMismatch
        ));
        assert!(matches!(
            KeyError::from(IdentityError::Corrupt),
            KeyError::Corrupt
        ));
        assert!(matches!(
            KeyError::from(IdentityError::UnsafePath),
            KeyError::UnsafePath
        ));
        assert!(matches!(
            KeyError::from(IdentityError::UnsafePermissions),
            KeyError::UnsafePermissions
        ));
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::AlreadyExists,
            std::io::ErrorKind::Other,
        ] {
            assert!(matches!(
                KeyError::from(IdentityError::Io(std::io::Error::new(
                    kind,
                    "uncertain write"
                ))),
                KeyError::Io { .. }
            ));
        }
    }
}
