//! Persistent group-owned state for sequential views on a caller-owned database.
use crate::{enrollment::Enrollment, observation::Source, Error};
use prns_groups_store::StoreIncarnation;
use prns_host::IdentityHash;
use redb::{Database, ReadableDatabase};
use std::{num::NonZeroUsize, sync::Arc};

/// Keep one attachment alongside its physical store owner. Borrow it for each
/// sequential GroupManager view; subscriptions and their commit witness outlive
/// those lexical views. Dropping the attachment closes its subscriptions.
///
/// Attachment requires the explicitly selected durable group stamp. The identity
/// and store incarnation fence accidental reuse with a freshly created store,
/// including another store for the same identity. The caller still owns physical
/// exclusivity and must not attach independent copies of the same database.
pub struct Attachment {
    pub(crate) identity: IdentityHash,
    pub(crate) incarnation: StoreIncarnation,
    pub(crate) observations: Source,
    pub(crate) carrier: Option<PendingCarrier>,
    pub(crate) validation: Option<prns_groups_store_redb::ValidationCache>,
    pub(crate) retained: Option<crate::retained::Limits>,
    pub(crate) catalog_limits: crate::enrollment::CatalogLimits,
    pub(crate) profile_support: Option<crate::ProfileSupport>,
}
impl Attachment {
    /// Validate selected state in one read snapshot, without recovery or writes.
    pub fn open(database: &Database, identity: IdentityHash) -> Result<Self, Error> {
        let read = database.begin_read().map_err(|error| {
            Error::Storage(prns_groups_store_redb::RedbError::Database(error.into()))
        })?;
        let catalog_limits = Enrollment::inspect_in(&read, identity).map_err(Error::Enrollment)?;
        let retained = crate::retained::inspect_in(&read, identity)?;
        crate::archive::validate_open_in(&read, identity)?;
        let stamp = prns_groups_store_redb::change::read_in(&read)?.ok_or(Error::Configuration)?;
        Enrollment::inspect_namespaces(database, identity, catalog_limits)
            .map_err(Error::Enrollment)?;
        if prns_groups_store_redb::change::read(database)? != Some(stamp) {
            return Err(Error::Busy);
        }
        Ok(Self {
            identity,
            incarnation: stamp.incarnation,
            observations: Source::default(),
            carrier: None,
            validation: None,
            profile_support: None,
            retained,
            catalog_limits,
        })
    }

    /// Opt into a bounded working set of namespace validation evidence. This
    /// retains the exact physical database allocation across lexical views;
    /// capacity limits cached proofs, not stored groups. The embedding must drop
    /// this attachment before reporting physical store close or reopening it.
    pub fn open_cached(
        database: Arc<Database>,
        identity: IdentityHash,
        capacity: NonZeroUsize,
    ) -> Result<Self, Error> {
        let mut attachment = Self::open(database.as_ref(), identity)?;
        attachment.validation = Some(prns_groups_store_redb::ValidationCache::new(
            database, capacity,
        )?);
        Ok(attachment)
    }

    /// Work counters only; no group contents, authority or memory guarantee.
    pub fn validation_stats(&self) -> Option<prns_groups_store_redb::ValidationStats> {
        self.validation
            .as_ref()
            .map(prns_groups_store_redb::ValidationCache::stats)
    }

    /// Install carrier support before publishing network readiness. Existing
    /// profiles are validated without writes. Support only tightens during this
    /// attachment's lifetime; reopening starts a new local capability selection.
    /// A failed validation leaves the previous selection unchanged.
    pub fn set_profile_support(
        &mut self,
        database: &Database,
        support: crate::ProfileSupport,
    ) -> Result<(), Error> {
        self.require_database(database)?;
        if self.carrier.is_some() {
            return Err(Error::Busy);
        }
        let stamp = prns_groups_store_redb::change::read(database)?.ok_or(Error::Configuration)?;
        if stamp.incarnation != self.incarnation {
            return Err(Error::Configuration);
        }
        let mut enrollment =
            Enrollment::open_attached(database, self.identity, self.catalog_limits)
                .map_err(Error::Enrollment)?
                .with_profile_support(self.profile_support);
        let supported = enrollment
            .set_profile_support(support)
            .map_err(Error::Enrollment)?;
        self.profile_support = Some(supported);
        Ok(())
    }

    pub(crate) fn require_database(&self, database: &Database) -> Result<(), Error> {
        if self
            .validation
            .as_ref()
            .is_some_and(|cache| !cache.matches_database(database))
        {
            return Err(Error::Configuration);
        }
        Ok(())
    }
}

pub(crate) enum PendingCarrier {
    Receive(PendingReceive),
    Enrollment(crate::enrollment::attempt::PendingEnrollment),
}
impl PendingCarrier {
    pub(crate) fn effect(&self) -> crate::admission::CarrierEffectId {
        match self {
            Self::Receive(receive) => receive.effect,
            Self::Enrollment(enrollment) => enrollment.effect,
        }
    }
    pub(crate) fn settlement(
        &self,
        database: &Database,
    ) -> Result<crate::admission::Settlement, Error> {
        match self {
            Self::Receive(receive) => receive.settlement(database),
            Self::Enrollment(enrollment) => enrollment.settlement(database),
        }
    }
}

/// One bounded physical attempt. This contains no input payload, replacement
/// intent or authority; the embedding retains the original permit separately.
#[derive(Clone, Copy)]
pub(crate) struct PendingReceive {
    pub namespace: prns_groups_store_redb::StoreNamespace,
    pub ticket: prns_groups_store::OperationTicket,
    pub limits: prns_groups_store_redb::RedbLimits,
    pub effect: crate::admission::CarrierEffectId,
    // Positive evidence from this exact synchronous attempt can survive a
    // later failed lease settlement. None requires exact backend recovery.
    pub known: Option<crate::admission::Settlement>,
}
impl Attachment {
    pub fn pending_carrier(&self) -> Option<crate::admission::CarrierEffectId> {
        self.carrier.as_ref().map(PendingCarrier::effect)
    }
    /// Probe only the exact existing physical attempt. No catalog recovery,
    /// namespace initialization, maintenance or new domain writes are allowed.
    /// Returns true after resolving one continuation, false when none exists.
    /// Every error retains both the continuation and the embedding's permit.
    pub fn recover_carrier<A: crate::admission::CarrierRecovery + ?Sized>(
        &mut self,
        database: &Database,
        authority: &mut A,
    ) -> Result<bool, Error> {
        self.require_database(database)?;
        let Some(pending) = self.carrier.as_ref() else {
            return Ok(false);
        };
        let stamp = prns_groups_store_redb::change::read(database)?.ok_or(Error::Configuration)?;
        if stamp.incarnation != self.incarnation {
            return Err(Error::Configuration);
        }
        authority.validate_carrier(pending.effect())?;
        let settlement = pending.settlement(database)?;
        authority.settle_carrier(pending.effect(), settlement)?;
        self.carrier = None;
        Ok(true)
    }
}
impl PendingReceive {
    pub(crate) fn settlement(
        self,
        database: &Database,
    ) -> Result<crate::admission::Settlement, Error> {
        use crate::admission::Settlement;
        use prns_groups_store::{AtomicBackend, RecoveryOutcome};
        if let Some(known) = self.known {
            return Ok(known);
        }
        let mut owner = prns_groups_store_redb::RedbStoreOwner::new(database);
        let mut backend = owner.open(self.namespace, self.ticket.incarnation(), self.limits)?;
        match backend.recover(self.ticket.operation_id())? {
            RecoveryOutcome::Committed(receipt)
                if receipt.operation_id == self.ticket.operation_id()
                    && receipt.revision.0 == self.ticket.sequence() =>
            {
                Ok(Settlement::Completed)
            }
            RecoveryOutcome::NotCommitted { revision }
                if revision.0.checked_add(1) == Some(self.ticket.sequence()) =>
            {
                Ok(Settlement::Rejected)
            }
            RecoveryOutcome::Unresolved => Err(Error::Unavailable),
            // A missing/pruned receipt or a rejected lookup does not prove the
            // uncertain original attempt failed. Fences must remain intact.
            _ => Err(Error::Corrupt),
        }
    }
}

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod tests;
