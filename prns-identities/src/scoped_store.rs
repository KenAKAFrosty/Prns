//! One immutable local-identity scope per native service database.
//!
//! Local record IDs, request IDs and cursors are meaningful only together with
//! [`IdentityReference`]. Keep clients pinned to this owner; never retarget a queued
//! operation using a UI's currently selected identity. Lifecycle authorization
//! remains the embedding runtime's responsibility.

use prns_identities_core::IdentityReference;
#[cfg(test)]
use prns_identities_core::{LocalIdentityId, PublicIdentityHash};
use prns_storage::{database as physical, files};
#[cfg(test)]
use redb::ReadableDatabase;
use redb::{Database, ReadTransaction, ReadableTable, TableDefinition, WriteTransaction};
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

const BINDING: TableDefinition<u8, &[u8]> = TableDefinition::new("prns_identity_scope_v1");
const MAGIC: &[u8; 8] = b"PRNSSCP1";

fn validate(scope: IdentityReference) -> Result<(), Error> {
    if scope.id.incarnation == [0; 16] || scope.id.sequence == 0 {
        return Err(Error::InvalidScope);
    }
    Ok(())
}

/// Deterministic identity-local path. Metadata and public hashes never select
/// alternate storage for the same catalog-local ID.
pub fn path_under(scope: IdentityReference, root: &Path) -> Result<PathBuf, Error> {
    validate(scope)?;
    let mut name = String::with_capacity(64);
    use std::fmt::Write;
    for byte in scope.id.incarnation {
        write!(&mut name, "{byte:02x}").map_err(|_| Error::InvalidScope)?;
    }
    write!(&mut name, "-{:016x}.redb", scope.id.sequence).map_err(|_| Error::InvalidScope)?;
    Ok(root.join(name))
}

fn encoded(scope: IdentityReference) -> [u8; 48] {
    let mut bytes = [0; 48];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..24].copy_from_slice(&scope.id.incarnation);
    bytes[24..32].copy_from_slice(&scope.id.sequence.to_be_bytes());
    bytes[32..].copy_from_slice(&scope.public_hash.0);
    bytes
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Database(redb::Error),
    InvalidScope,
    InvalidOptions,
    MissingBinding,
    WrongBinding,
    UnsafePath,
    UnsupportedPlatform,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "identity-scoped service storage: {self:?}")
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
fn db(error: impl Into<redb::Error>) -> Error {
    Error::Database(error.into())
}

fn file_error(error: files::Error) -> Error {
    match error {
        files::Error::Io(error) => Error::Io(error),
        files::Error::UnsafePath | files::Error::UnsafePermissions => Error::UnsafePath,
        files::Error::UnsupportedPlatform => Error::UnsupportedPlatform,
    }
}

/// A service database permanently bound to one local identity. Opening another
/// owner at the same path uses redb's exclusive lock. Moving/copying a file cannot
/// make it belong to another scope; open verifies its durable binding first.
pub struct ScopedDatabase {
    database: physical::Owner,
    scope: IdentityReference,
}

impl ScopedDatabase {
    /// Create only the immutable identity binding, with an explicit cache budget.
    /// Use create_initialized when the database selects domain schemas as well.
    pub fn create_new(
        path: &Path,
        scope: IdentityReference,
        options: physical::Options,
    ) -> Result<Self, Error> {
        Self::create_initialized(path, scope, options, |_| Ok::<_, Error>(()))
            .map_err(physical_error)
    }

    /// Bind the identity and initialize selected top-level schemas in one
    /// transaction. E is the embedding's typed union of its domain errors.
    /// The initializer must use the supplied transaction, never open a writer.
    pub fn create_initialized<E: From<Error>>(
        path: &Path,
        scope: IdentityReference,
        options: physical::Options,
        initialize: impl FnOnce(&WriteTransaction) -> Result<(), E>,
    ) -> Result<Self, physical::Error<E>> {
        validate(scope).map_err(|error| physical::Error::Domain(error.into()))?;
        let database = physical::Owner::create_new(path, options, |tx| {
            initialize_in(tx, scope).map_err(E::from)?;
            initialize(tx)
        })?;
        Ok(Self { database, scope })
    }

    /// Reopen only the identity binding. Selected schemas require open_validated.
    pub fn open(
        path: &Path,
        scope: IdentityReference,
        options: physical::Options,
    ) -> Result<Self, Error> {
        Self::open_validated(path, scope, options, |_| Ok::<_, Error>(())).map_err(physical_error)
    }

    /// Validate the immutable binding before each selected domain validates its
    /// schema in the same read transaction. Missing state is never initialized.
    pub fn open_validated<E: From<Error>>(
        path: &Path,
        scope: IdentityReference,
        options: physical::Options,
        validate_domains: impl FnOnce(&ReadTransaction) -> Result<(), E>,
    ) -> Result<Self, physical::Error<E>> {
        validate(scope).map_err(|error| physical::Error::Domain(error.into()))?;
        let database = physical::Owner::open_existing(path, options, |read| {
            validate_in(read, scope).map_err(E::from)?;
            validate_domains(read)
        })?;
        Ok(Self { database, scope })
    }

    pub const fn scope(&self) -> IdentityReference {
        self.scope
    }
    pub fn database(&self) -> &Database {
        self.database.database()
    }
    /// Retain this exact bound allocation for a domain attachment. Release all
    /// handles before reporting physical close; they keep its file lock alive.
    pub fn database_handle(&self) -> std::sync::Arc<Database> {
        self.database.database_handle()
    }
}

/// Initialize this binding in the physical owner's creation transaction.
/// Existing or partial binding tables are rejected, never repaired.
pub fn initialize_in(tx: &WriteTransaction, scope: IdentityReference) -> Result<(), Error> {
    use redb::TableHandle;
    validate(scope)?;
    if tx
        .list_tables()
        .map_err(db)?
        .any(|table| table.name() == BINDING.name())
    {
        return Err(Error::WrongBinding);
    }
    tx.open_table(BINDING)
        .map_err(db)?
        .insert(0, encoded(scope).as_slice())
        .map_err(db)?;
    Ok(())
}

/// Read-only immutable binding verification for selected domain attachment.
pub fn validate_in(read: &ReadTransaction, scope: IdentityReference) -> Result<(), Error> {
    validate(scope)?;
    let binding = match read.open_table(BINDING) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Err(Error::MissingBinding),
        Err(error) => return Err(db(error)),
    };
    let value = binding.get(0).map_err(db)?.ok_or(Error::MissingBinding)?;
    if value.value() != encoded(scope) {
        return Err(Error::WrongBinding);
    }
    let mut entries = binding.iter().map_err(db)?;
    let (key, _) = entries.next().ok_or(Error::MissingBinding)?.map_err(db)?;
    if key.value() != 0 || entries.next().transpose().map_err(db)?.is_some() {
        return Err(Error::WrongBinding);
    }
    Ok(())
}
fn physical_error(error: physical::Error<Error>) -> Error {
    match error {
        physical::Error::InvalidOptions => Error::InvalidOptions,
        physical::Error::File(error) => file_error(error),
        physical::Error::Database(error) => Error::Database(error),
        physical::Error::Domain(error) => error,
    }
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    const OPTIONS: physical::Options = physical::Options {
        cache_bytes: 1024 * 1024,
    };

    #[test]
    fn binding_survives_reopen_and_rejects_other_local_owner_or_key() {
        let root = std::env::temp_dir().join(format!("prns-identity-scope-{}", std::process::id()));
        files::create_private_directory(&root).unwrap();
        let scope = IdentityReference {
            id: LocalIdentityId {
                incarnation: [7; 16],
                sequence: 1,
            },
            public_hash: PublicIdentityHash([8; 16]),
        };
        let path = path_under(scope, &root).unwrap();
        let store = ScopedDatabase::create_new(&path, scope, OPTIONS).unwrap();
        assert_eq!(store.scope(), scope);
        assert!(ScopedDatabase::create_new(&path, scope, OPTIONS).is_err());
        drop(store);
        for wrong in [
            IdentityReference {
                public_hash: PublicIdentityHash([9; 16]),
                ..scope
            },
            IdentityReference {
                id: LocalIdentityId {
                    sequence: 2,
                    ..scope.id
                },
                ..scope
            },
            IdentityReference {
                id: LocalIdentityId {
                    incarnation: [9; 16],
                    ..scope.id
                },
                ..scope
            },
        ] {
            assert!(matches!(
                ScopedDatabase::open(&path, wrong, OPTIONS),
                Err(Error::WrongBinding)
            ));
        }
        let store = ScopedDatabase::open(&path, scope, OPTIONS).unwrap();
        assert_eq!(store.scope(), scope);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn absent_binding_and_invalid_scope_never_initialize_an_existing_database() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "prns-identity-scope-missing-{}",
            std::process::id()
        ));
        files::create_private_directory(&root).unwrap();
        let scope = IdentityReference {
            id: LocalIdentityId {
                incarnation: [7; 16],
                sequence: 1,
            },
            public_hash: PublicIdentityHash([8; 16]),
        };
        let path = path_under(scope, &root).unwrap();
        drop(Database::create(&path).unwrap());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            ScopedDatabase::open(&path, scope, OPTIONS),
            Err(Error::MissingBinding)
        ));
        assert!(ScopedDatabase::create_new(&path, scope, OPTIONS).is_err());
        let raw = Database::open(&path).unwrap();
        assert!(matches!(
            raw.begin_read().unwrap().open_table(BINDING),
            Err(redb::TableError::TableDoesNotExist(_))
        ));
        drop(raw);
        for identity in [
            LocalIdentityId {
                incarnation: [0; 16],
                sequence: 1,
            },
            LocalIdentityId {
                sequence: 0,
                ..scope.id
            },
        ] {
            let invalid = IdentityReference {
                id: identity,
                ..scope
            };
            assert!(matches!(
                path_under(invalid, &root),
                Err(Error::InvalidScope)
            ));
            assert!(matches!(
                ScopedDatabase::create_new(&root.join("invalid.redb"), invalid, OPTIONS),
                Err(Error::InvalidScope)
            ));
        }
        assert!(!root.join("invalid.redb").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn identity_and_selected_schema_share_atomic_creation_and_ordered_validation() {
        const SELECTED: TableDefinition<u8, u64> = TableDefinition::new("selected_service");
        let root =
            std::env::temp_dir().join(format!("prns-identity-atomic-{}", std::process::id()));
        files::create_private_directory(&root).unwrap();
        let scope = IdentityReference {
            id: LocalIdentityId {
                incarnation: [7; 16],
                sequence: 1,
            },
            public_hash: PublicIdentityHash([8; 16]),
        };
        let path = path_under(scope, &root).unwrap();
        let result = ScopedDatabase::create_initialized(&path, scope, OPTIONS, |tx| {
            tx.open_table(SELECTED)
                .map_err(db)?
                .insert(0, 42)
                .map_err(db)?;
            Err::<(), Error>(Error::InvalidScope)
        });
        assert!(matches!(
            result,
            Err(physical::Error::Domain(Error::InvalidScope))
        ));
        assert!(matches!(
            ScopedDatabase::open(&path, scope, OPTIONS),
            Err(Error::MissingBinding)
        ));
        let raw = Database::open(&path).unwrap();
        assert_eq!(raw.begin_read().unwrap().list_tables().unwrap().count(), 0);
        drop(raw);
        let complete = root.join("complete.redb");
        drop(
            ScopedDatabase::create_initialized(&complete, scope, OPTIONS, |tx| {
                tx.open_table(SELECTED)
                    .map_err(db)?
                    .insert(0, 42)
                    .map_err(db)?;
                Ok::<_, Error>(())
            })
            .unwrap(),
        );
        let called = std::cell::Cell::new(false);
        let wrong = IdentityReference {
            public_hash: PublicIdentityHash([9; 16]),
            ..scope
        };
        let result = ScopedDatabase::open_validated(&complete, wrong, OPTIONS, |_| {
            called.set(true);
            Ok::<_, Error>(())
        });
        assert!(matches!(
            result,
            Err(physical::Error::Domain(Error::WrongBinding))
        ));
        assert!(!called.get());
        drop(
            ScopedDatabase::open_validated(&complete, scope, OPTIONS, |read| {
                assert_eq!(
                    read.open_table(SELECTED)
                        .map_err(db)?
                        .get(0)
                        .map_err(db)?
                        .unwrap()
                        .value(),
                    42
                );
                Ok::<_, Error>(())
            })
            .unwrap(),
        );
        fs::remove_dir_all(root).unwrap();
    }
}
