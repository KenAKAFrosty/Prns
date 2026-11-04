//! Optional local history in the existing native group's physical transaction.
//! Delivery expiry never evicts these rows. Only explicit fenced local deletion
//! releases them. Logical budgets cover encoded keys/values/indexes, not redb page
//! overhead, physical file size, encryption at rest, or secure media erasure.
#[cfg(feature = "archive")]
mod participant;
#[cfg(feature = "archive")]
pub(crate) mod reads;
#[cfg(feature = "archive")]
mod schema;
mod selection;
#[cfg(feature = "archive")]
pub use participant::Participant;
#[cfg(feature = "archive")]
pub use schema::{initialize_in, inspect_in, read_usage_in as usage_in, Limits, Usage};
#[cfg(all(test, feature = "archive"))]
mod tests;

use crate::Error;
use prns_host::IdentityHash;
use redb::{Database, ReadTransaction, ReadableDatabase};

/// Full bounded readiness validation occurs once per persistent owner/attachment.
pub(crate) fn validate_open_in(
    read: &ReadTransaction,
    identity: IdentityHash,
) -> Result<(), Error> {
    #[cfg(feature = "archive")]
    {
        inspect_in(read, identity)?;
    }
    #[cfg(not(feature = "archive"))]
    {
        let _ = identity;
        if selection::present(read)? {
            return Err(Error::Configuration);
        }
    }
    Ok(())
}
pub(crate) fn validate_open(database: &Database, identity: IdentityHash) -> Result<(), Error> {
    let read = database
        .begin_read()
        .map_err(|e| Error::Storage(prns_groups_store_redb::RedbError::Database(e.into())))?;
    validate_open_in(&read, identity)
}
/// Ordinary lexical views inspect only the selected header and table counts.
/// They never rescan retained rows or create a missing archive.
pub(crate) fn selected(database: &Database, identity: IdentityHash) -> Result<bool, Error> {
    let read = database
        .begin_read()
        .map_err(|e| Error::Storage(prns_groups_store_redb::RedbError::Database(e.into())))?;
    selection_in(&read, identity)
}

/// Attach the selected projection to the exact existing namespace backend.
/// Both ordinary and carrier execution use this before constructing MLS.
pub(crate) fn backend<'db>(
    database: &Database,
    cfg: &crate::Provisioned,
    backend: prns_groups_store_redb::RedbBackend<'db>,
) -> Result<prns_groups_store_redb::RedbBackend<'db>, Error> {
    let selected = selected(database, cfg.local_reticulum_identity)?;
    #[cfg(feature = "archive")]
    if selected {
        return Ok(backend.with_participant(std::sync::Arc::new(Participant::new(cfg)?)));
    }
    #[cfg(not(feature = "archive"))]
    let _ = selected;
    Ok(backend)
}

/// Inspect only selection/header/counts. An archive-free binary rejects any
/// selected or partial archive instead of silently opening around it.
pub fn selection_in(read: &ReadTransaction, identity: IdentityHash) -> Result<bool, Error> {
    #[cfg(feature = "archive")]
    {
        Ok(usage_in(read, identity)?.is_some())
    }
    #[cfg(not(feature = "archive"))]
    {
        validate_open_in(read, identity)?;
        Ok(false)
    }
}

pub(crate) fn purge_retired(
    database: &Database,
    owner: &mut prns_groups_store_redb::RedbStoreOwner<'_>,
    identity: IdentityHash,
    group: prns_groups_core::GroupId,
    context: prns_groups_store_redb::CommitContext,
    limits: prns_groups_store_redb::RedbLimits,
) -> Result<(), prns_groups_store_redb::RedbError> {
    let selected = selected(database, identity).map_err(|error| match error {
        Error::Storage(error) => error,
        _ => prns_groups_store_redb::RedbError::Configuration,
    })?;
    #[cfg(feature = "archive")]
    if selected {
        let participant = participant::Purge {
            identity,
            group,
            context,
        };
        return owner.delete_with(
            context.namespace,
            context.incarnation,
            limits,
            Some(&participant),
        );
    }
    #[cfg(not(feature = "archive"))]
    let _ = (selected, group);
    owner.delete(context.namespace, context.incarnation, limits)
}
