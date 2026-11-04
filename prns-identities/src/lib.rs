//! Durable filesystem catalog and plaintext identity vault for one exclusive owner.
//!
//! Private file permissions are not encryption. Only this owner may access the
//! root while open; the advisory process lock must also cover its runtime owner.
//! Unix local filesystems with durable rename and directory sync are supported.
#![forbid(unsafe_code)]

pub mod announces;
#[cfg(feature = "owner")]
pub mod api;
#[cfg(feature = "uniffi-bindings")]
uniffi::setup_scaffolding!();
#[cfg(feature = "uniffi-bindings")]
mod bindings;
#[cfg(feature = "contact-card")]
pub mod contact_card;
pub mod creation;
mod files;
pub mod keys;
#[cfg(feature = "owner")]
pub mod owned;
mod runtime_authority;
#[cfg(feature = "redb-store")]
pub mod scoped_store;
mod vault;

use prns_core::crypto::sha256;
use prns_core::entropy::{EntropySource, RuntimeEntropy};
use prns_identities_core::{Catalog, Coordinator, EffectTicket, Fault, Storage};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use prns_core::identity::vault::IdentitySecretKey;
pub use runtime_authority::CatalogAuthority;

pub type Result<T> = std::result::Result<T, Error>;
const CATALOG_MAGIC: &[u8; 8] = b"PNICAT02";
const ENVELOPE_OVERHEAD: usize = 8 + 8 + 32;
pub const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
/// Bounds native snapshot construction/scratch independently of encoded bytes.
/// Larger catalogs require an adapter with caller-controlled scratch allocation.
pub const MAX_CATALOG_MEMORY_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Codec(prns_identities_codec::Error),
    Entropy(getrandom::Error),
    Authority(Fault),
    Busy,
    Bounds,
    Corrupt,
    MissingCatalog,
    MissingKey,
    KeyMismatch,
    ImportRequired,
    UnexpectedImport,
    WrongAction,
    UnsafePath,
    UnsafePermissions,
    UnsupportedPlatform,
    UnsupportedCatalogFormat,
    RevisionMismatch { expected: u64, found: u64 },
    ForeignCatalog,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "identity storage: {self:?}")
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<prns_identities_codec::Error> for Error {
    fn from(error: prns_identities_codec::Error) -> Self {
        Self::Codec(error)
    }
}

struct OsEntropy;
impl EntropySource for OsEntropy {
    type Error = getrandom::Error;
    fn try_fill_entropy(&mut self, output: &mut [u8]) -> std::result::Result<(), Self::Error> {
        getrandom::fill(output)
    }
}

/// Not Clone. Holding this object keeps the OS lock across catalog, vault and
/// runtime operations. Do not unlink its lock file or share this root with a
/// legacy vault. `open` never creates a missing catalog or missing key.
pub struct IdentityStore<const I: usize, const O: usize, const S: usize> {
    root: PathBuf,
    _lock: Arc<DirectoryLock>,
    maximum_body: usize,
}

// One explicit unlock when the last actual native use ends, including unwind.
// Merely closing a duplicated/inherited descriptor need not release its flock.
struct DirectoryLock(File);
impl Drop for DirectoryLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl<const I: usize, const O: usize, const S: usize> IdentityStore<I, O, S> {
    /// Create a dedicated new directory. An existing directory is an error even
    /// if it is empty; retry uncertain creation by opening, never reinitializing.
    pub fn create_new(path: impl AsRef<Path>) -> Result<Self> {
        let maximum_body = body_bound::<I, O, S>()?;
        let requested = path.as_ref();
        let parent = requested
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let root = fs::canonicalize(parent)?.join(requested.file_name().ok_or(Error::UnsafePath)?);
        let mut entropy = RuntimeEntropy::try_new(OsEntropy).map_err(Error::Entropy)?;
        let mut incarnation = [0; 16];
        entropy.fill_random(&mut incarnation);
        if incarnation == [0; 16] {
            return Err(Error::Corrupt);
        }
        files::new_directory(&root)?;
        let lock = files::new_file(&root.join("owner.lock"))?;
        lock.sync_all()?;
        lock.try_lock().map_err(lock_error)?;
        let lock = Arc::new(DirectoryLock(lock));
        files::sync_directory(&root)?;
        files::new_directory(&root.join("keys"))?;
        files::new_directory(&root.join("retained"))?;
        let owner = Self {
            root,
            _lock: lock,
            maximum_body,
        };
        owner.write_initial_catalog(incarnation)?;
        Ok(owner)
    }

    /// Open only a complete existing store. A corrupt current catalog is never
    /// replaced with a staged or empty snapshot, avoiding silent rollback.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let maximum_body = body_bound::<I, O, S>()?;
        files::check_directory(path.as_ref())?;
        let root = fs::canonicalize(path)?;
        let lock_path = root.join("owner.lock");
        if !files::check_file(&lock_path)? {
            return Err(Error::MissingCatalog);
        }
        let lock = OpenOptions::new().read(true).write(true).open(lock_path)?;
        lock.try_lock().map_err(lock_error)?;
        let lock = Arc::new(DirectoryLock(lock));
        files::check_directory(&root.join("keys"))?;
        files::check_directory(&root.join("retained"))?;
        let owner = Self {
            root,
            _lock: lock,
            maximum_body,
        };
        owner.validate_catalog()?;
        Ok(owner)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    // Keep large typed temporaries out of open/create_new frames: their bounds
    // checks must run before entering any capacity-dependent stack frame.
    #[inline(never)]
    fn write_initial_catalog(&self, incarnation: [u8; 16]) -> Result<()> {
        self.write_catalog(
            &Catalog::new(incarnation),
            &creation::Evidence::new(),
            "catalog",
        )
    }

    #[inline(never)]
    fn validate_catalog(&self) -> Result<()> {
        let mut snapshot = Box::new(Catalog::new([0; 16]));
        self.read_catalog(&mut snapshot)
    }

    fn read_catalog(&self, into: &mut Catalog<I, O, S>) -> Result<()> {
        self.read_envelope(into).map(|_| ())
    }

    fn read_envelope(&self, into: &mut Catalog<I, O, S>) -> Result<creation::Evidence<S>> {
        let bytes = files::read_bounded(
            &self.root.join("catalog.bin"),
            self.maximum_body + ENVELOPE_OVERHEAD,
        )?
        .ok_or(Error::MissingCatalog)?;
        if bytes.get(..8) == Some(b"PNICAT01".as_slice()) {
            return Err(Error::UnsupportedCatalogFormat);
        }
        let body = unwrap(&bytes, CATALOG_MAGIC, self.maximum_body)?;
        creation::decode_envelope(body, into)
    }

    fn write_catalog(
        &self,
        catalog: &Catalog<I, O, S>,
        evidence: &creation::Evidence<S>,
        family: &str,
    ) -> Result<()> {
        let body = creation::encode_envelope(catalog, evidence, self.maximum_body)?;
        let wrapped = wrap(CATALOG_MAGIC, &body)?;
        files::replace(
            &self.root.join("catalog.bin"),
            &self.root.join("catalog.next"),
            &wrapped,
            family,
        )
    }

    /// The in-memory coordinator must still be the exact durable owner before
    /// any key/runtime effect. This catches a stale coordinator even if its own
    /// snapshot would otherwise accept the ticket.
    pub fn validate_effect(
        &self,
        authority: &Coordinator<'_, I, O, S>,
        ticket: EffectTicket,
    ) -> Result<()> {
        authority
            .validate_effect(ticket)
            .map_err(Error::Authority)?;
        self.validate_owner(authority)
    }

    fn validate_owner(&self, authority: &Coordinator<'_, I, O, S>) -> Result<()> {
        if authority.is_poisoned() {
            return Err(Error::Authority(Fault::Poisoned));
        }
        let mut current = Box::new(Catalog::new([0; 16]));
        self.read_catalog(&mut current)?;
        let known = authority.snapshot();
        if current.incarnation != known.incarnation {
            return Err(Error::ForeignCatalog);
        }
        if current.revision != known.revision || current.session != known.session {
            return Err(Error::RevisionMismatch {
                expected: known.revision,
                found: current.revision,
            });
        }
        Ok(())
    }
}

impl<const I: usize, const O: usize, const S: usize> Storage<I, O, S> for IdentityStore<I, O, S> {
    type Error = Error;
    fn load(&mut self, into: &mut Catalog<I, O, S>) -> Result<()> {
        self.read_catalog(into)
    }
    fn compare_and_store(&mut self, expected_revision: u64, next: &Catalog<I, O, S>) -> Result<()> {
        self.compare_and_store_creation(expected_revision, next, None)
    }
}

fn body_bound<const I: usize, const O: usize, const S: usize>() -> Result<usize> {
    // Zero-capacity arrays can hide very large element types. The codec still
    // monomorphizes per-record temporaries, so bound those before entering it.
    if std::mem::size_of::<Catalog<I, O, S>>() > MAX_CATALOG_MEMORY_BYTES
        || std::mem::size_of::<prns_identities_core::IdentityRecord<S>>() > MAX_CATALOG_MEMORY_BYTES
        || std::mem::size_of::<prns_identities_core::OperationRecord<S>>()
            > MAX_CATALOG_MEMORY_BYTES
    {
        return Err(Error::Bounds);
    }
    let native_memory = O
        .checked_mul(std::mem::size_of::<creation::Record<S>>())
        .ok_or(Error::Bounds)?;
    if native_memory
        .checked_add(std::mem::size_of::<Catalog<I, O, S>>())
        .ok_or(Error::Bounds)?
        > MAX_CATALOG_MEMORY_BYTES
    {
        return Err(Error::Bounds);
    }
    let maximum = creation::envelope_bound::<I, O, S>()?;
    if maximum > MAX_CATALOG_BYTES {
        return Err(Error::Bounds);
    }
    Ok(maximum)
}

fn lock_error(error: TryLockError) -> Error {
    match error {
        TryLockError::WouldBlock => Error::Busy,
        TryLockError::Error(error) => Error::Io(error),
    }
}

pub(crate) fn wrap(magic: &[u8; 8], body: &[u8]) -> Result<Vec<u8>> {
    let size = body
        .len()
        .checked_add(ENVELOPE_OVERHEAD)
        .ok_or(Error::Bounds)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| Error::Bounds)?;
    bytes.extend_from_slice(magic);
    bytes.extend_from_slice(&(body.len() as u64).to_be_bytes());
    bytes.extend_from_slice(body);
    bytes.extend_from_slice(&sha256(&bytes));
    Ok(bytes)
}

pub(crate) fn unwrap<'a>(bytes: &'a [u8], magic: &[u8; 8], maximum: usize) -> Result<&'a [u8]> {
    if bytes.len() < ENVELOPE_OVERHEAD || &bytes[..8] != magic {
        return Err(Error::Corrupt);
    }
    let length = u64::from_be_bytes(bytes[8..16].try_into().map_err(|_| Error::Corrupt)?);
    let length = usize::try_from(length).map_err(|_| Error::Bounds)?;
    if length > maximum || length.checked_add(ENVELOPE_OVERHEAD) != Some(bytes.len()) {
        return Err(Error::Corrupt);
    }
    let end = bytes.len() - 32;
    if sha256(&bytes[..end]).as_slice() != &bytes[end..] {
        return Err(Error::Corrupt);
    }
    Ok(&bytes[16..end])
}

pub(crate) fn reference_name(reference: prns_identities_core::KeyReference) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(64);
    for byte in reference.0 {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    result
}

#[cfg(all(test, unix))]
mod lock_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn dropping_owner_releases_lock_even_with_inherited_file_description() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "prns-identities-lock-{}-{suffix}",
            std::process::id()
        ));
        let store = IdentityStore::<1, 4, 1>::create_new(&root).unwrap();
        // A concurrent fork briefly inherits the same open-file description
        // before exec closes CLOEXEC descriptors. try_clone reproduces that
        // lifetime deterministically without unsafe fork/pre_exec callbacks.
        let inherited = store._lock.0.try_clone().unwrap();
        assert!(matches!(
            IdentityStore::<1, 4, 1>::open(&root),
            Err(Error::Busy)
        ));
        drop(store);
        let reopened = IdentityStore::<1, 4, 1>::open(&root);
        // Clean up even when asserting the regression against an unfixed owner.
        drop(inherited);
        let reopened_ok = reopened.is_ok();
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
        assert!(
            reopened_ok,
            "the dropped owner's inherited descriptor retained its lock"
        );
    }
}
