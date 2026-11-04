//! A standalone Reticulum identity, without a named-identity catalog or host.
//!
//! Owners and leases retain one exclusive directory lock and one zeroizing key.
//! A lease is an ordinary Rust capability; foreign adapters should expose an
//! opaque lease handle, never its private material accessor.

use crate::{files, lock_error, vault, DirectoryLock, Error, IdentitySecretKey, Result};
use prns_core::identity::{IdentityHash, PrivateIdentityMaterial, PublicIdentityMaterial};
use prns_identities_core::{KeyReference, KeySource, PublicIdentityHash};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// A standalone root has exactly one key. Reuse the vault's record format with
// a fixed reference, rather than introducing a catalog or another key codec.
const KEY_REFERENCE: KeyReference = KeyReference([0; 32]);
const KEY_FILE: &str = "identity.key";

/// The owner of a dedicated key directory. This type is deliberately not Clone.
///
/// Dropping the owner releases its interest; outstanding leases retain both the
/// key and the exclusive lock. The last owner/lease drop zeroizes the in-memory
/// key and unlocks the directory. There are no background tasks to drain.
/// Never unlink its lock file or share its directory with another vault.
pub struct KeyOwner {
    state: Arc<KeyState>,
}

/// A native domain's retained access to an identity. Cloning a lease does not
/// copy the key or relinquish exclusive ownership of its directory.
#[derive(Clone)]
pub struct KeyLease {
    state: Arc<KeyState>,
}

struct KeyState {
    root: PathBuf,
    material: PrivateIdentityMaterial,
    // Fields drop in declaration order: zeroize the key before unlocking.
    _lock: DirectoryLock,
}

impl KeyOwner {
    /// Generate and durably publish one identity in a new dedicated directory.
    ///
    /// An existing directory is always an error, including after an interrupted
    /// creation. Errors leave any created files in place for explicit recovery;
    /// retrying this method never replaces them with a different identity.
    pub fn create_new(path: impl AsRef<Path>) -> Result<Self> {
        Self::create(path.as_ref(), KeySource::Generate, None)
    }

    /// Publish exactly the reviewed imported secret in a new dedicated root.
    /// Validate its public binding before touching the filesystem. Existing or
    /// interrupted roots are preserved for explicit recovery, never replaced.
    pub fn create_imported(
        path: impl AsRef<Path>,
        secret: &IdentitySecretKey,
        expected: IdentityHash,
    ) -> Result<Self> {
        if PrivateIdentityMaterial::from(secret.clone()).identity_hash() != expected {
            return Err(Error::KeyMismatch);
        }
        Self::create(
            path.as_ref(),
            KeySource::Import {
                expected_hash: PublicIdentityHash(*expected.as_bytes()),
            },
            Some(secret),
        )
    }

    fn create(
        requested: &Path,
        source: KeySource,
        import: Option<&IdentitySecretKey>,
    ) -> Result<Self> {
        let parent = requested
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let root = fs::canonicalize(parent)?.join(requested.file_name().ok_or(Error::UnsafePath)?);
        files::new_directory(&root)?;
        let file = files::new_file(&root.join("owner.lock"))?;
        file.sync_all()?;
        file.try_lock().map_err(lock_error)?;
        let lock = DirectoryLock(file);
        files::sync_directory(&root)?;
        let secret = vault::publish_key(&root.join(KEY_FILE), KEY_REFERENCE, source, import)?;
        Ok(Self::from_locked_key(root, lock, secret))
    }

    /// Recover the uncertain result of this directory's initial creation.
    ///
    /// This explicit first-creation boundary discovers the public identity of an
    /// existing complete key, finishes publication of a verified stage if needed,
    /// and confirms file/directory durability before returning a usable owner.
    /// A missing key, corrupt record or contradictory stage fails without fresh
    /// generation or replacement. Calling it repeatedly retains the same key.
    ///
    /// Use only when the initial creation outcome was lost before recording its
    /// public identity. Persist the returned public identity binding before
    /// opening dependent domain state. Once that binding exists, always use
    /// `open_existing` with its expected hash; recovery is not a way to accept an
    /// identity mismatch or replace an application's established identity.
    pub fn recover_creation(path: impl AsRef<Path>) -> Result<Self> {
        let (root, lock) = lock_existing(path.as_ref())?;
        let secret = vault::recover_key(&root.join(KEY_FILE), KEY_REFERENCE)?;
        Ok(Self::from_locked_key(root, lock, secret))
    }

    /// Recover only existing complete/staged imported material. Both its public
    /// binding and exact private bytes must match the reviewed source. Absence
    /// never imports again or generates a replacement key.
    pub fn recover_import(
        path: impl AsRef<Path>,
        secret: &IdentitySecretKey,
        expected: IdentityHash,
    ) -> Result<Self> {
        if PrivateIdentityMaterial::from(secret.clone()).identity_hash() != expected {
            return Err(Error::KeyMismatch);
        }
        let (root, lock) = lock_existing(path.as_ref())?;
        let path = root.join(KEY_FILE);
        let expected_hash = PublicIdentityHash(*expected.as_bytes());
        if vault::read_key(&path, KEY_REFERENCE, Some(expected_hash))?.is_none()
            && vault::read_key(
                &path.with_extension("pending"),
                KEY_REFERENCE,
                Some(expected_hash),
            )?
            .is_none()
        {
            return Err(Error::MissingKey);
        }
        let secret = vault::publish_key(
            &path,
            KEY_REFERENCE,
            KeySource::Import { expected_hash },
            Some(secret),
        )?;
        Ok(Self::from_locked_key(root, lock, secret))
    }

    /// Open a published identity and check its expected public identity hash.
    ///
    /// This never generates a key, promotes a stage, repairs a file, or creates
    /// missing state. Missing/corrupt/unsafe state and identity mismatches fail
    /// without changing the directory. The expected hash comes from the caller's
    /// immutable identity binding, not from the file being opened.
    pub fn open_existing(path: impl AsRef<Path>, expected: IdentityHash) -> Result<Self> {
        let (root, lock) = lock_existing(path.as_ref())?;
        let secret = vault::read_key(
            &root.join(KEY_FILE),
            KEY_REFERENCE,
            Some(PublicIdentityHash(*expected.as_bytes())),
        )?
        .ok_or(Error::MissingKey)?;
        Ok(Self::from_locked_key(root, lock, secret))
    }

    fn from_locked_key(root: PathBuf, lock: DirectoryLock, secret: IdentitySecretKey) -> Self {
        Self {
            state: Arc::new(KeyState {
                root,
                material: PrivateIdentityMaterial::from(secret),
                _lock: lock,
            }),
        }
    }

    pub fn root(&self) -> &Path {
        &self.state.root
    }

    pub fn public_identity(&self) -> PublicIdentityMaterial {
        self.state.material.public()
    }

    pub fn identity_hash(&self) -> IdentityHash {
        self.state.material.identity_hash()
    }

    pub fn lease(&self) -> KeyLease {
        KeyLease {
            state: Arc::clone(&self.state),
        }
    }
}

impl KeyLease {
    /// Issue a bounded, service-neutral signed claim using this retained key.
    /// This neither registers a service nor proves availability or peer trust.
    #[cfg(feature = "contact-card")]
    pub fn contact_card(
        &self,
        service_name_hash: crate::contact_card::ServiceNameHash,
        display_name: &str,
    ) -> std::result::Result<Vec<u8>, prns_identities_core::ApiError> {
        crate::contact_card::issue(&self.state.material, service_name_hash, display_name)
    }

    pub fn public_identity(&self) -> PublicIdentityMaterial {
        self.state.material.public()
    }

    pub fn identity_hash(&self) -> IdentityHash {
        self.state.material.identity_hash()
    }

    /// Borrow the existing core identity capability for a native Rust domain.
    ///
    /// Retain this lease for every domain use of the identity. This accessor is
    /// not a foreign API: bindings should pass an opaque lease into their native
    /// construction adapters, without exporting private material or key bytes.
    pub fn material(&self) -> &PrivateIdentityMaterial {
        &self.state.material
    }
}

fn lock_existing(path: &Path) -> Result<(PathBuf, DirectoryLock)> {
    files::check_directory(path)?;
    let root = fs::canonicalize(path)?;
    let lock_path = root.join("owner.lock");
    if !files::check_file(&lock_path)? {
        return Err(Error::MissingKey);
    }
    let file = OpenOptions::new().read(true).write(true).open(lock_path)?;
    file.try_lock().map_err(lock_error)?;
    Ok((root, DirectoryLock(file)))
}
