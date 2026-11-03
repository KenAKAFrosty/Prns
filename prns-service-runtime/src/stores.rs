//! Bounded physical ownership, separate from private activation key use.
//!
//! One file binds one immutable local identity. Selected domains borrow the same
//! database only inside their owner-lane dispatch; they do not open it again or
//! create a writer/worker per domain. Domain schemas remain domain-owned.
use prns_identities::scoped_store::{self, ScopedDatabase};
use prns_identities_core::IdentityReference;
use prns_storage::{database as physical, files};
use redb::{Database, WriteTransaction};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub per_identity: physical::Options,
    /// Explicit upper bound for all I possible simultaneously opened caches.
    /// This is not a bound on every domain allocation or physical file size.
    pub total_cache_bytes: usize,
}
impl Options {
    pub fn validate<const I: usize>(self) -> Result<Self, Error<std::convert::Infallible>> {
        self.per_identity
            .validate()
            .map_err(|_| Error::InvalidOptions)?;
        if I == 0
            || I.checked_mul(self.per_identity.cache_bytes)
                .is_none_or(|required| required > self.total_cache_bytes)
        {
            return Err(Error::InvalidOptions);
        }
        Ok(self)
    }
}
#[derive(Debug)]
pub enum Error<E> {
    InvalidOptions,
    Allocation,
    Capacity,
    Binding(scoped_store::Error),
    File(files::Error),
    Physical(physical::Error<E>),
    Domain(E),
}
impl<E: std::fmt::Debug> std::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native attached store: {self:?}")
    }
}
impl<E: std::fmt::Debug> std::error::Error for Error<E> {}

struct Entry {
    reference: IdentityReference,
    owner: ScopedDatabase,
}
pub struct Stores<const I: usize> {
    root: PathBuf,
    options: Options,
    entries: Vec<Entry>,
}
impl<const I: usize> Stores<I> {
    /// Attach an existing private directory. Creation belongs to the explicit
    /// outer factory; opening never repairs or creates missing directories.
    pub fn open(root: &Path, options: Options) -> Result<Self, Error<std::convert::Infallible>> {
        options.validate::<I>()?;
        files::check_private_directory(root).map_err(Error::File)?;
        let root = std::fs::canonicalize(root).map_err(|e| Error::File(files::Error::Io(e)))?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(I)
            .map_err(|_| Error::Allocation)?;
        Ok(Self {
            root,
            options,
            entries,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn existing<E>(&mut self, reference: IdentityReference) -> Result<usize, Error<E>> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.reference.id == reference.id)
        {
            if self.entries[index].reference != reference {
                return Err(Error::Binding(scoped_store::Error::WrongBinding));
            }
            return Ok(index);
        }
        if self.entries.len() == I {
            return Err(Error::Capacity);
        }
        let path = scoped_store::path_under(reference, &self.root).map_err(Error::Binding)?;
        let owner = ScopedDatabase::open(&path, reference, self.options.per_identity)
            .map_err(Error::Binding)?;
        self.entries.push(Entry { reference, owner });
        Ok(self.entries.len() - 1)
    }
    /// Borrow only a strictly reopened binding. The caller validates its selected
    /// domain and constructs at most one exclusive domain view at a time. R
    /// cannot carry a borrow of this database outside the callback.
    pub fn with_existing<E, R>(
        &mut self,
        reference: IdentityReference,
        operation: impl FnOnce(&Database) -> Result<R, E>,
    ) -> Result<R, Error<E>> {
        let index = self.existing(reference)?;
        operation(self.entries[index].owner.database()).map_err(Error::Domain)
    }
    /// Internal retained attachment handle. The domain must drop it before
    /// these physical owners close. Binding validation is shared with normal
    /// lexical views and never substitutes another identity's database.
    pub(crate) fn database_handle<E>(
        &mut self,
        reference: IdentityReference,
    ) -> Result<std::sync::Arc<Database>, Error<E>> {
        let index = self.existing(reference)?;
        Ok(self.entries[index].owner.database_handle())
    }
    /// Called only for an explicit current retained service-initialization action.
    /// A fresh file binds the identity and selected schemas atomically. Existing
    /// files are strictly reopened first; the domain reconciler may initialize
    /// only its proven-absent schema or validate complete supported prior state.
    /// Unsupported/corrupt/partial bindings are preserved, never reset.
    pub fn prepare<E: From<scoped_store::Error>>(
        &mut self,
        reference: IdentityReference,
        initialize: impl FnOnce(&WriteTransaction) -> Result<(), E>,
        reconcile: impl FnOnce(&Database) -> Result<(), E>,
    ) -> Result<(), Error<E>> {
        if self
            .entries
            .iter()
            .any(|entry| entry.reference.id == reference.id)
        {
            return self.with_existing(reference, reconcile);
        }
        if self.entries.len() == I {
            return Err(Error::Capacity);
        }
        let path = scoped_store::path_under(reference, &self.root).map_err(Error::Binding)?;
        if files::check_private_file(&path).map_err(Error::File)? {
            return self.with_existing(reference, reconcile);
        }
        let owner = ScopedDatabase::create_initialized(
            &path,
            reference,
            self.options.per_identity,
            initialize,
        )
        .map_err(Error::Physical)?;
        self.entries.push(Entry { reference, owner });
        Ok(())
    }
}
