//! Exact live carrier evidence; no input journal or independent authority.
use super::*;
use crate::admission::{CarrierEffectId, Settlement};
use crate::attachment::PendingReceive;
use prns_groups_store::{Mutation, OperationRejection};
use prns_groups_store_redb::{CommitContext, TransactionParticipant};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
enum Evidence {
    NotDispatched,
    Catalog {
        before: [u8; 32],
        after: [u8; 32],
    },
    Namespace {
        context: CommitContext,
        ticket: OperationTicket,
        limits: prns_groups_store_redb::RedbLimits,
    },
    Completed,
}
/// One fixed-size witness, shared only with the lexical enrollment/backend
/// views that execute this attempt. The attachment keeps it through waiter loss
/// or unwind. It owns neither permission nor a second operation registry.
#[derive(Clone)]
pub(crate) struct Attempt(Arc<Mutex<Evidence>>);
impl Attempt {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(Evidence::NotDispatched)))
    }
    fn record(&self, evidence: Evidence) -> Result<(), RedbError> {
        let mut current = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        if !matches!(*current, Evidence::NotDispatched) {
            // Never replace evidence of an earlier possible commit with a later
            // failed phase. Raw protocol effects have one transaction boundary.
            return Err(RedbError::Corrupt);
        }
        *current = evidence;
        Ok(())
    }
    pub(super) fn catalog(&self, before: [u8; 32], after: [u8; 32]) -> Result<(), RedbError> {
        self.record(Evidence::Catalog { before, after })
    }
    pub(crate) fn completed(&self) {
        let mut current = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        if !matches!(*current, Evidence::NotDispatched) {
            *current = Evidence::Completed;
        }
    }
    fn settlement(
        &self,
        database: &Database,
        local: IdentityHash,
        effect: CarrierEffectId,
    ) -> Result<Settlement, crate::Error> {
        let evidence = *self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        match evidence {
            Evidence::NotDispatched => Ok(Settlement::NotDispatched),
            Evidence::Completed => Ok(Settlement::Completed),
            Evidence::Catalog { before, after } => {
                let read = database
                    .begin_read()
                    .map_err(|error| crate::Error::Storage(RedbError::Database(error.into())))?;
                let catalog =
                    Enrollment::read_in(&read, local).map_err(crate::Error::Enrollment)?;
                let actual = catalog
                    .header
                    .fingerprint()
                    .map_err(crate::Error::Enrollment)?;
                if actual == after {
                    Ok(Settlement::Completed)
                } else if actual == before {
                    Ok(Settlement::Rejected)
                } else {
                    Err(crate::Error::Corrupt)
                }
            }
            Evidence::Namespace {
                context,
                ticket,
                limits,
            } => PendingReceive {
                namespace: context.namespace,
                ticket,
                limits,
                effect,
                known: None,
            }
            .settlement(database),
        }
    }
}

pub(crate) struct PendingEnrollment {
    pub(super) local: IdentityHash,
    pub(crate) effect: CarrierEffectId,
    pub(crate) attempt: Attempt,
}
impl PendingEnrollment {
    pub(crate) fn new(local: IdentityHash, effect: CarrierEffectId, attempt: Attempt) -> Self {
        Self {
            local,
            effect,
            attempt,
        }
    }
    pub(crate) fn settlement(&self, database: &Database) -> Result<Settlement, crate::Error> {
        self.attempt.settlement(database, self.local, self.effect)
    }
}

/// The same namespace transaction projects exact catalog indexes. A read view
/// lives only for this lexical operation, not in the retained attachment.
pub(super) struct Participant {
    pub local: IdentityHash,
    pub attempt: Option<Attempt>,
    pub departure_ack: Option<[u8; 32]>,
    pub catalog: Arc<Catalog>,
    pub limits: prns_groups_store_redb::RedbLimits,
}
impl TransactionParticipant for Participant {
    fn stage(
        &self,
        transaction: &WriteTransaction,
        context: CommitContext,
        _values: &prns_groups_store_redb::CommitRead<'_>,
        ticket: OperationTicket,
        changes: &[Mutation],
    ) -> Result<Option<OperationRejection>, RedbError> {
        if ticket.incarnation() != context.incarnation {
            return Err(RedbError::ForeignIncarnation);
        }
        let write = match Edit::new(&self.catalog)
            .and_then(|edit| edit.namespace_projection(&self.catalog, context, changes))
            .and_then(|edit| match self.departure_ack {
                Some(id) => edit.clear_departure_id(&self.catalog, id),
                None => Ok(edit),
            })
            .and_then(Edit::finish)
        {
            Ok(write) => write,
            Err(EnrollmentError::Capacity) => {
                return Ok(Some(OperationRejection::ReservedCapacity))
            }
            Err(error) => return Err(store_error(error)),
        };
        write.stage(transaction, self.local).map_err(store_error)?;
        if let Some(attempt) = &self.attempt {
            attempt.record(Evidence::Namespace {
                context,
                ticket,
                limits: self.limits,
            })?;
        }
        #[cfg(test)]
        if self.attempt.is_some() {
            fault::before_commit()?;
        }
        Ok(None)
    }
    fn delete_in(&self, _: &WriteTransaction, _: CommitContext) -> Result<(), RedbError> {
        Err(RedbError::Configuration)
    }
}

fn store_error(error: EnrollmentError) -> RedbError {
    match error {
        EnrollmentError::Storage(error) => error,
        EnrollmentError::Database(error) => RedbError::Database(error),
        _ => RedbError::Corrupt,
    }
}

#[cfg(test)]
pub(crate) mod fault {
    use std::cell::Cell;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Cut {
        BeforeCommit,
        AfterEffect,
    }
    thread_local! {
        static NEXT: Cell<Option<Cut>> = const { Cell::new(None) };
        static INTERRUPTED: Cell<bool> = const { Cell::new(false) };
    }
    pub(crate) fn set(cut: Cut) {
        NEXT.with(|next| next.set(Some(cut)));
        INTERRUPTED.with(|v| v.set(false));
    }
    fn cut(at: Cut) -> bool {
        NEXT.with(|next| {
            if next.get() == Some(at) {
                next.set(None);
                true
            } else {
                false
            }
        })
    }
    pub(crate) fn before_commit() -> Result<(), prns_groups_store_redb::RedbError> {
        if cut(Cut::BeforeCommit) {
            INTERRUPTED.with(|v| v.set(true));
            Err(prns_groups_store_redb::RedbError::Corrupt)
        } else {
            Ok(())
        }
    }
    pub(crate) fn after_effect() -> Result<(), crate::Error> {
        if INTERRUPTED.with(|v| v.replace(false)) || cut(Cut::AfterEffect) {
            Err(crate::Error::Unavailable)
        } else {
            Ok(())
        }
    }
}
