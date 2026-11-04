use super::*;
use groups::enrollment::{Enrollment, EnrollmentError};
use prns_groups_store::StoreIncarnation;
use prns_groups_store_redb::change;
use redb::{Database, ReadableDatabase, WriteTransaction};

#[derive(Debug)]
pub enum SchemaError {
    Binding(prns_identities::scoped_store::Error),
    Group(groups::Error),
    Database(redb::Error),
    Entropy(getrandom::Error),
}
impl From<prns_identities::scoped_store::Error> for SchemaError {
    fn from(error: prns_identities::scoped_store::Error) -> Self {
        Self::Binding(error)
    }
}
impl From<groups::Error> for SchemaError {
    fn from(error: groups::Error) -> Self {
        Self::Group(error)
    }
}
pub(super) fn db_error(error: impl Into<redb::Error>) -> SchemaError {
    SchemaError::Database(error.into())
}
fn incarnation() -> Result<StoreIncarnation, SchemaError> {
    let mut entropy = prns_core::entropy::RuntimeEntropy::try_new(getrandom::fill)
        .map_err(SchemaError::Entropy)?;
    loop {
        let mut bytes = [0; 24];
        entropy.fill_random(&mut bytes);
        if let Ok(value) = StoreIncarnation::new(bytes) {
            return Ok(value);
        }
    }
}
pub(super) fn initialize(
    tx: &WriteTransaction,
    identity: groups::IdentityHash,
    limits: groups::retained::Limits,
    catalog: groups::enrollment::CatalogLimits,
    #[cfg(feature = "archive")] archive: Option<groups::archive::Limits>,
) -> Result<(), SchemaError> {
    Enrollment::initialize_with_limits_in(tx, identity, catalog)
        .map_err(|error| SchemaError::Group(groups::Error::Enrollment(error)))?;
    groups::retained::initialize_in(tx, identity, limits)?;
    change::initialize_in(tx, incarnation()?)
        .map_err(|error| SchemaError::Group(groups::Error::Storage(error)))?;
    #[cfg(feature = "archive")]
    if let Some(limits) = archive {
        groups::archive::initialize_in(tx, identity, limits)
            .map_err(|error| SchemaError::Group(groups::Error::Storage(error)))?;
    }
    Ok(())
}
pub(super) fn validate(db: &Database, identity: groups::IdentityHash) -> Result<(), SchemaError> {
    let read = db.begin_read().map_err(db_error)?;
    Enrollment::validate_in(&read, identity)
        .map_err(|error| SchemaError::Group(groups::Error::Enrollment(error)))?;
    groups::retained::validate_in(&read, identity)?;
    groups::archive_selection_in(&read, identity)?;
    if change::read_in(&read)
        .map_err(|error| SchemaError::Group(groups::Error::Storage(error)))?
        .is_none()
    {
        return Err(SchemaError::Group(groups::Error::NotProvisioned));
    }
    Ok(())
}
/// Explicit AddService reconciliation only. All three selected schemas are one
/// unit: complete absence may initialize, complete supported state may resume,
/// and any partial/unknown state is preserved and rejected.
pub(super) fn reconcile(
    db: &Database,
    identity: groups::IdentityHash,
    limits: groups::retained::Limits,
    catalog_limits: groups::enrollment::CatalogLimits,
    #[cfg(feature = "archive")] archive: Option<groups::archive::Limits>,
) -> Result<(), SchemaError> {
    let read = db.begin_read().map_err(db_error)?;
    let catalog = match Enrollment::validate_in(&read, identity) {
        Ok(()) => true,
        Err(EnrollmentError::MissingCatalog) => false,
        Err(error) => return Err(SchemaError::Group(groups::Error::Enrollment(error))),
    };
    let retained = groups::retained::inspect_in(&read, identity)?.is_some();
    let stamp = change::read_in(&read)
        .map_err(|error| SchemaError::Group(groups::Error::Storage(error)))?
        .is_some();
    let archive_selected = groups::archive_selection_in(&read, identity)?;
    match (catalog, retained, stamp) {
        (true, true, true) => Ok(()),
        (false, false, false) if !archive_selected => {
            drop(read);
            let mut write = db.begin_write().map_err(db_error)?;
            write
                .set_durability(redb::Durability::Immediate)
                .map_err(db_error)?;
            initialize(
                &write,
                identity,
                limits,
                catalog_limits,
                #[cfg(feature = "archive")]
                archive,
            )?;
            write.commit().map_err(db_error)
        }
        _ => Err(SchemaError::Group(groups::Error::Corrupt)),
    }
}
