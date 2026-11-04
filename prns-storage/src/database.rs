//! Physical redb ownership shared by native domains.
//!
//! Construction is blocking. Keep this owner and its borrowing domain managers
//! in one worker's lexical scope. Only the explicit initializer shares a write
//! transaction; ordinary domain operations own their own transactions.
use crate::files;
use redb::{Database, ReadTransaction, ReadableDatabase, WriteTransaction};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Cache only, not total memory, logical domain capacity, or disk reservation.
    pub cache_bytes: usize,
}
impl Options {
    pub fn validate(self) -> Result<Self, InvalidOptions> {
        if !(1024 * 1024..=1024 * 1024 * 1024).contains(&self.cache_bytes) {
            return Err(InvalidOptions);
        }
        Ok(self)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidOptions;

/// Physical facts and the domain's own initialization/validation error. Any
/// failure after file creation leaves that file for explicit reconciliation.
/// A commit/confirmation failure can follow durable success; never replace or
/// reset a database merely because construction returned an error.
#[derive(Debug)]
pub enum Error<E> {
    InvalidOptions,
    File(files::Error),
    Database(redb::Error),
    Domain(E),
}
impl<E: std::fmt::Debug> std::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native physical storage: {self:?}")
    }
}
impl<E: std::fmt::Debug> std::error::Error for Error<E> {}
fn db<E>(error: impl Into<redb::Error>) -> Error<E> {
    Error::Database(error.into())
}
fn io<E>(error: std::io::Error) -> Error<E> {
    Error::File(files::Error::Io(error))
}

/// Resolve once before asynchronous dispatch. The immediate parent must be a
/// private directory. The embedding controls ancestors and concurrent renames.
pub fn private_path(path: &Path) -> Result<PathBuf, files::Error> {
    let absolute = std::path::absolute(path)?;
    let name = absolute.file_name().ok_or(files::Error::UnsafePath)?;
    let parent = absolute.parent().ok_or(files::Error::UnsafePath)?;
    files::check_private_directory(parent)?;
    Ok(std::fs::canonicalize(parent)?.join(name))
}

/// Exactly one redb database/lock. It does not own a runtime, domain schemas,
/// identity authority, or a worker; those remain explicit in the embedding.
pub struct Owner {
    database: Arc<Database>,
}
impl Owner {
    /// Create a fresh file and atomically initialize all selected top-level
    /// bindings/schemas. The callback must not open another write transaction.
    /// If it fails, its transaction aborts and the incomplete file is preserved.
    pub fn create_new<E>(
        path: &Path,
        options: Options,
        initialize: impl FnOnce(&WriteTransaction) -> Result<(), E>,
    ) -> Result<Self, Error<E>> {
        let options = options.validate().map_err(|_| Error::InvalidOptions)?;
        let path = private_path(path).map_err(Error::File)?;
        let file = files::create_private_file(&path).map_err(Error::File)?;
        let mut builder = Database::builder();
        builder.set_cache_size(options.cache_bytes);
        let database = builder.create_file(file).map_err(db)?;
        let mut tx = database.begin_write().map_err(db)?;
        tx.set_durability(redb::Durability::Immediate).map_err(db)?;
        initialize(&tx).map_err(Error::Domain)?;
        tx.commit().map_err(db)?;
        confirm(&path)?;
        Ok(Self {
            database: Arc::new(database),
        })
    }

    /// Strict open, followed by domain validation in one read transaction. Never
    /// creates a missing file or table. redb may recover its committed state.
    /// Confirmation also reconciles a previously lost creation success reply.
    pub fn open_existing<E>(
        path: &Path,
        options: Options,
        validate: impl FnOnce(&ReadTransaction) -> Result<(), E>,
    ) -> Result<Self, Error<E>> {
        let options = options.validate().map_err(|_| Error::InvalidOptions)?;
        let path = private_path(path).map_err(Error::File)?;
        if !files::check_private_file(&path).map_err(Error::File)? {
            return Err(io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "native database is missing",
            )));
        }
        let mut builder = Database::builder();
        builder.set_cache_size(options.cache_bytes);
        let database = builder.open(&path).map_err(db)?;
        {
            let read = database.begin_read().map_err(db)?;
            validate(&read).map_err(Error::Domain)?;
        }
        confirm(&path)?;
        Ok(Self {
            database: Arc::new(database),
        })
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    /// Retain the exact physical allocation for a domain attachment. This keeps
    /// the file lock alive: drop all such handles before reporting owner close.
    /// Domain managers should continue borrowing `database()` for normal work.
    pub fn database_handle(&self) -> Arc<Database> {
        Arc::clone(&self.database)
    }
}
fn confirm<E>(path: &Path) -> Result<(), Error<E>> {
    File::open(path).map_err(io)?.sync_all().map_err(io)?;
    files::sync_directory(path.parent().ok_or(Error::File(files::Error::UnsafePath))?)
        .map_err(Error::File)
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use redb::TableDefinition;
    use std::sync::atomic::{AtomicU64, Ordering};
    const A: TableDefinition<u8, u64> = TableDefinition::new("test_domain_a");
    const B: TableDefinition<u8, u64> = TableDefinition::new("test_domain_b");
    const OPTIONS: Options = Options {
        cache_bytes: 1024 * 1024,
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "prns-physical-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            files::create_private_directory(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf {
            self.0.join("state.redb")
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn initialize(tx: &WriteTransaction) -> Result<(), redb::Error> {
        tx.open_table(A)?.insert(0, 7)?;
        tx.open_table(B)?.insert(0, 9)?;
        Ok(())
    }
    fn validate(read: &ReadTransaction) -> Result<(), redb::Error> {
        assert_eq!(read.open_table(A)?.get(0)?.unwrap().value(), 7);
        assert_eq!(read.open_table(B)?.get(0)?.unwrap().value(), 9);
        Ok(())
    }
    #[test]
    fn selected_schemas_commit_together_and_reopen_under_one_lock() {
        let root = Root::new();
        let owner = Owner::create_new(&root.file(), OPTIONS, initialize).unwrap();
        assert!(Owner::open_existing(&root.file(), OPTIONS, validate).is_err());
        assert!(Owner::create_new(&root.file(), OPTIONS, initialize).is_err());
        // No implicit surrounding transaction: handlers open their own writers.
        let tx = owner.database().begin_write().unwrap();
        tx.open_table(A).unwrap().insert(1, 11).unwrap();
        tx.commit().unwrap();
        drop(owner);
        let owner = Owner::open_existing(&root.file(), OPTIONS, validate).unwrap();
        let read = owner.database().begin_read().unwrap();
        assert_eq!(
            read.open_table(A).unwrap().get(1).unwrap().unwrap().value(),
            11
        );
    }
    #[test]
    fn retained_attachment_handle_keeps_exact_allocation_and_lock_until_dropped() {
        let root = Root::new();
        let owner = Owner::create_new(&root.file(), OPTIONS, initialize).unwrap();
        let handle = owner.database_handle();
        assert!(std::ptr::eq(owner.database(), handle.as_ref()));
        drop(owner);
        assert!(Owner::open_existing(&root.file(), OPTIONS, validate).is_err());
        drop(handle);
        Owner::open_existing(&root.file(), OPTIONS, validate).unwrap();
    }
    #[test]
    fn initializer_failure_leaves_no_partly_committed_schema_and_never_recreates() {
        let root = Root::new();
        let result = Owner::create_new(&root.file(), OPTIONS, |tx| {
            tx.open_table(A).unwrap().insert(0, 7).unwrap();
            Err("second domain failed")
        });
        assert!(matches!(result, Err(Error::Domain("second domain failed"))));
        assert!(root.file().exists());
        assert!(Owner::create_new(&root.file(), OPTIONS, initialize).is_err());
        assert!(matches!(
            Owner::open_existing(&root.file(), OPTIONS, validate),
            Err(Error::Domain(_))
        ));
        let raw = Database::open(root.file()).unwrap();
        assert_eq!(raw.begin_read().unwrap().list_tables().unwrap().count(), 0);
    }
    #[test]
    fn absent_empty_and_rejected_state_are_preserved() {
        let root = Root::new();
        assert!(Owner::open_existing(&root.file(), OPTIONS, validate).is_err());
        assert!(!root.file().exists());
        drop(files::create_private_file(&root.file()).unwrap());
        assert!(Owner::open_existing(&root.file(), OPTIONS, validate).is_err());
        assert_eq!(std::fs::metadata(root.file()).unwrap().len(), 0);
        let another = root.0.join("valid.redb");
        drop(Owner::create_new(&another, OPTIONS, initialize).unwrap());
        assert!(matches!(
            Owner::open_existing(&another, OPTIONS, |_| Err("wrong binding")),
            Err(Error::Domain("wrong binding"))
        ));
        drop(Owner::open_existing(&another, OPTIONS, validate).unwrap());
        let invalid = root.0.join("invalid.redb");
        assert!(matches!(
            Owner::create_new(&invalid, Options { cache_bytes: 0 }, initialize),
            Err(Error::InvalidOptions)
        ));
        assert!(!invalid.exists());
    }
}
