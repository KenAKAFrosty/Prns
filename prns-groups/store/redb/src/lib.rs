//! Durable group storage in the application's existing redb database.
//!
//! The database owner must create exactly one [`RedbStoreOwner`] and must not
//! write its reserved tables directly. A backend exclusively borrows that owner;
//! unrelated application tables remain available through the shared database.
//! This backend provides atomic durability, not encryption or secure erasure.
#![forbid(unsafe_code)]

#[cfg(feature = "uniffi-bindings")]
uniffi::setup_scaffolding!();

pub mod change;
mod limits;
use limits::CheckedLimits;
pub use limits::RedbLimits;
mod witness;
pub use witness::ChangeWitness;

mod enrollment;
mod inspection;
mod metadata;
mod participant;
pub use participant::{CommitContext, CommitRead, TransactionParticipant};
mod removal;
mod reserve;
mod validation;
pub use enrollment::EnrollmentSnapshot;
pub use reserve::RedbReserve;
pub use validation::{ValidationCache, ValidationStats};
#[cfg(test)]
mod tests;

use metadata::{State, TableNames};
use prns_groups_store::{
    AtomicBackend, CommitOutcome, CommitReceipt, MaintenanceBackend, MaintenanceOutcome, Mutation,
    OperationId, OperationRejection, OperationTicket, RecoveryOutcome, Revision, StoreIncarnation,
};
use redb::{Database, Durability, ReadableDatabase};
use std::{cell::RefCell, fmt, marker::PhantomData, sync::Arc};

/// Stable application-selected namespace, distinct from a fresh store incarnation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreNamespace(pub [u8; 16]);

#[derive(Debug)]
pub enum RedbError {
    Database(redb::Error),
    AlreadyInitialized,
    MissingNamespace,
    Corrupt,
    Configuration,
    ForeignIncarnation,
    Capacity,
    Allocation,
    CounterExhausted,
}
impl fmt::Display for RedbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "redb group storage error: {self:?}")
    }
}
impl std::error::Error for RedbError {}
fn db_error(error: impl Into<redb::Error>) -> RedbError {
    RedbError::Database(error.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedbUsage {
    pub revision: Revision,
    pub retired_through: u64,
    pub live_objects: usize,
    pub live_bytes: usize,
    pub receipts: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedbMaintenanceRequest {
    /// The trusted owner attests every recovery obligation through this ticket
    /// is discharged. This is not a network receipt or physical key erasure.
    FinalizeThrough(OperationTicket),
}

/// Construct for one sequential, exclusive domain attachment. Release every
/// backend before detaching; a later attachment may construct a new owner. This
/// is an explicit ownership contract:
/// constructing a second owner or bypassing its tables violates the backend's
/// exclusive-writer requirement. No database, thread, runtime or global is created.
///
/// A second backend cannot borrow the same owner while the first is in use:
///
/// ```compile_fail
/// use prns_groups_store::{AtomicBackend, StoreIncarnation};
/// use prns_groups_store_redb::{RedbLimits, RedbStoreOwner, StoreNamespace};
/// fn overlapping(db: &redb::Database, incarnation: StoreIncarnation, limits: RedbLimits) {
///     let mut owner = RedbStoreOwner::new(db);
///     let mut first = owner.open(StoreNamespace([1; 16]), incarnation, limits).unwrap();
///     let second = owner.open(StoreNamespace([2; 16]), incarnation, limits);
///     first.head().unwrap();
/// }
/// ```
pub struct RedbStoreOwner<'db> {
    database: &'db Database,
    change_witness: Option<ChangeWitness>,
    change_stamp: Option<StoreIncarnation>,
    validation: Option<&'db mut RefCell<validation::Entries>>,
}
impl<'db> RedbStoreOwner<'db> {
    pub fn new(database: &'db Database) -> Self {
        Self {
            database,
            change_witness: None,
            change_stamp: None,
            validation: None,
        }
    }

    /// Attach an optional process-local commit witness shared with this domain's
    /// catalog and retained intents. This creates no persistent schema.
    pub fn with_change_witness(mut self, witness: ChangeWitness) -> Self {
        self.change_witness = Some(witness);
        self
    }

    pub fn change_witness(&self) -> Option<&ChangeWitness> {
        self.change_witness.as_ref()
    }

    /// Require this already-selected group-domain stamp on every commit.
    /// This is the physical domain incarnation, not a namespace incarnation.
    /// Each write validates it atomically and avoids a database-wide table scan;
    /// an absent or replaced stamp fails without committing any domain changes.
    pub fn with_change_stamp(mut self, incarnation: StoreIncarnation) -> Self {
        self.change_stamp = Some(incarnation);
        self
    }

    pub fn change_stamp(&self) -> Option<StoreIncarnation> {
        self.change_stamp
    }

    /// Initialize only a namespace with no existing tables. Even empty orphan
    /// tables are evidence that must be inspected, never silently reformatted.
    pub fn initialize<'a>(
        &'a mut self,
        namespace: StoreNamespace,
        incarnation: StoreIncarnation,
        limits: RedbLimits,
    ) -> Result<RedbBackend<'a>, RedbError> {
        validation::invalidate(self.validation.as_deref(), namespace);
        let names = TableNames::new(namespace);
        let state = State::initial(incarnation, limits.checked()?)?;
        let mut write = self.database.begin_write().map_err(db_error)?;
        write
            .set_durability(Durability::Immediate)
            .map_err(db_error)?;
        if names.present_in(&self.database.begin_read().map_err(db_error)?)? != 0 {
            return Err(RedbError::AlreadyInitialized);
        }
        {
            let mut metadata = write.open_table(names.metadata()).map_err(db_error)?;
            metadata
                .insert(0, state.encode()?.as_slice())
                .map_err(db_error)?;
            write.open_table(names.values()).map_err(db_error)?;
            write.open_table(names.receipts()).map_err(db_error)?;
        }
        change::commit_with_selection(write, self.change_witness.as_ref(), self.change_stamp)?;
        self.open(namespace, incarnation, limits)
    }

    /// Open existing state with the exact persisted limits and incarnation.
    /// Missing/corrupt evidence never becomes a newly initialized empty store.
    pub fn open<'a>(
        &'a mut self,
        namespace: StoreNamespace,
        incarnation: StoreIncarnation,
        limits: RedbLimits,
    ) -> Result<RedbBackend<'a>, RedbError> {
        let backend = RedbBackend {
            database: self.database,
            names: TableNames::new(namespace),
            namespace,
            participant: None,
            incarnation,
            limits: limits.checked()?,
            admission: reserve::Admission::Full,
            change_witness: self.change_witness.clone(),
            change_stamp: self.change_stamp,
            validation: self.validation.as_deref_mut(),
            lease: PhantomData,
        };
        backend.snapshot()?;
        Ok(backend)
    }
}

/// The lifetime is the exclusive namespace-owner lease, not database ownership.
pub struct RedbBackend<'owner> {
    database: &'owner Database,
    names: TableNames,
    namespace: StoreNamespace,
    participant: Option<Arc<dyn TransactionParticipant>>,
    incarnation: StoreIncarnation,
    limits: CheckedLimits,
    admission: reserve::Admission,
    change_witness: Option<ChangeWitness>,
    change_stamp: Option<StoreIncarnation>,
    validation: Option<&'owner mut RefCell<validation::Entries>>,
    lease: PhantomData<&'owner mut ()>,
}
impl RedbBackend<'_> {
    /// Add one synchronous transaction participant for this exclusively borrowed
    /// namespace. It cannot commit independently or replace ticket recovery.
    pub fn with_participant(mut self, participant: Arc<dyn TransactionParticipant>) -> Self {
        self.participant = Some(participant);
        self
    }

    fn require_tables(&self, _write: &redb::WriteTransaction) -> Result<(), RedbError> {
        // The caller holds this database's writer lock. A read-side lookup
        // detects even missing empty tables before write-side open can create
        // them; the existing state validation still checks their contents.
        if self
            .names
            .present_in(&self.database.begin_read().map_err(db_error)?)?
            != 3
        {
            return Err(RedbError::MissingNamespace);
        }
        Ok(())
    }

    fn snapshot(&self) -> Result<State, RedbError> {
        let read = self.database.begin_read().map_err(db_error)?;
        self.snapshot_in(&read)
    }

    fn snapshot_in(&self, read: &redb::ReadTransaction) -> Result<State, RedbError> {
        let result = self.load_snapshot(read);
        if result.is_err() {
            validation::invalidate(self.validation.as_deref(), self.namespace);
        }
        result
    }

    fn load_snapshot(&self, read: &redb::ReadTransaction) -> Result<State, RedbError> {
        let meta = read.open_table(self.names.metadata()).map_err(open_error)?;
        let values = read.open_table(self.names.values()).map_err(open_error)?;
        let receipts = read.open_table(self.names.receipts()).map_err(open_error)?;
        let state = State::load(&meta, self.incarnation, self.limits)?;
        validation::validate(
            self.validation.as_deref(),
            self.namespace,
            &state,
            &values,
            &receipts,
        )?;
        Ok(state)
    }

    /// Inspect every value and receipt, even when this attachment has a cache
    /// entry. Cache hits rely on the exclusive-writer contract; raw changes that
    /// preserve metadata and counts are not a supported mutation path.
    pub fn inspect(&self) -> Result<RedbUsage, RedbError> {
        validation::invalidate(self.validation.as_deref(), self.namespace);
        self.usage()
    }
    pub fn usage(&self) -> Result<RedbUsage, RedbError> {
        Ok(self.snapshot()?.usage())
    }
    fn receipt(&self, ticket: OperationTicket) -> CommitReceipt {
        CommitReceipt {
            operation_id: ticket.operation_id(),
            revision: Revision(ticket.sequence()),
        }
    }
}
fn open_error(error: redb::TableError) -> RedbError {
    match error {
        redb::TableError::TableDoesNotExist(_) => RedbError::MissingNamespace,
        other => db_error(other),
    }
}
fn ticket(state: &State, operation: OperationId) -> Result<OperationTicket, OperationRejection> {
    let ticket = OperationTicket::from_operation_id(operation)
        .map_err(|_| OperationRejection::InvalidTicket)?;
    if ticket.incarnation() != state.incarnation {
        return Err(OperationRejection::ForeignIncarnation);
    }
    if ticket.sequence() > state.revision.0.saturating_add(1) {
        return Err(OperationRejection::OutOfOrder {
            next: state
                .revision
                .0
                .checked_add(1)
                .and_then(|seq| OperationTicket::new(state.incarnation, seq).ok()),
        });
    }
    Ok(ticket)
}

impl AtomicBackend for RedbBackend<'_> {
    type Error = RedbError;
    fn head(&mut self) -> Result<Revision, Self::Error> {
        Ok(self.snapshot()?.revision)
    }
    fn read(&mut self, key: &[u8], max_bytes: usize) -> Result<Option<Vec<u8>>, Self::Error> {
        if key.len() > self.limits.transaction.max_key_bytes {
            return Err(RedbError::Capacity);
        }
        let read = self.database.begin_read().map_err(db_error)?;
        self.snapshot_in(&read)?;
        let values = read.open_table(self.names.values()).map_err(open_error)?;
        let Some(value) = values.get(key).map_err(db_error)? else {
            return Ok(None);
        };
        let bytes = value.value();
        if bytes.len() > self.limits.transaction.max_value_bytes {
            validation::invalidate(self.validation.as_deref(), self.namespace);
            return Err(RedbError::Corrupt);
        }
        if bytes.len() > max_bytes {
            return Err(RedbError::Capacity);
        }
        let mut result = Vec::new();
        result
            .try_reserve_exact(bytes.len())
            .map_err(|_| RedbError::Allocation)?;
        result.extend_from_slice(bytes);
        Ok(Some(result))
    }
    fn recover(&mut self, operation: OperationId) -> Result<RecoveryOutcome, Self::Error> {
        let read = self.database.begin_read().map_err(db_error)?;
        let state = self.snapshot_in(&read)?;
        let ticket = match ticket(&state, operation) {
            Ok(t) => t,
            Err(r) => return Ok(RecoveryOutcome::Rejected(r)),
        };
        if ticket.sequence() <= state.floor {
            return Ok(RecoveryOutcome::Retired {
                revision: state.revision,
                retired_through: OperationTicket::new(self.incarnation, state.floor)
                    .map_err(|_| RedbError::Corrupt)?,
            });
        }
        if ticket.sequence() <= state.revision.0 {
            let receipts = read.open_table(self.names.receipts()).map_err(open_error)?;
            validation::receipt(
                self.validation.as_deref(),
                self.namespace,
                &receipts,
                ticket.sequence(),
            )?;
            return Ok(RecoveryOutcome::Committed(self.receipt(ticket)));
        }
        if state.receipt_count()? == self.limits.max_receipts {
            return Ok(RecoveryOutcome::Rejected(
                OperationRejection::RecoveryWindowFull,
            ));
        }
        Ok(RecoveryOutcome::NotCommitted {
            revision: state.revision,
        })
    }
    fn commit(
        &mut self,
        expected: Revision,
        operation: OperationId,
        changes: &[Mutation],
    ) -> Result<CommitOutcome, Self::Error> {
        let mut write = self.database.begin_write().map_err(db_error)?;
        self.require_tables(&write)?;
        write
            .set_durability(Durability::Immediate)
            .map_err(db_error)?;
        let receipt;
        let participant_ticket;
        let committed_state;
        {
            let mut meta = write.open_table(self.names.metadata()).map_err(db_error)?;
            let mut values = write.open_table(self.names.values()).map_err(db_error)?;
            let mut receipts = write.open_table(self.names.receipts()).map_err(db_error)?;
            let mut state = State::load(&meta, self.incarnation, self.limits)?;
            validation::validate(
                self.validation.as_deref(),
                self.namespace,
                &state,
                &values,
                &receipts,
            )?;
            let ticket = match ticket(&state, operation) {
                Ok(t) => t,
                Err(r) => return Ok(CommitOutcome::Rejected(r)),
            };
            if ticket.sequence() <= state.floor {
                return Ok(CommitOutcome::Rejected(OperationRejection::RetiredTicket));
            }
            if ticket.sequence() <= state.revision.0 {
                validation::receipt(
                    self.validation.as_deref(),
                    self.namespace,
                    &receipts,
                    ticket.sequence(),
                )?;
                return Ok(CommitOutcome::Committed(self.receipt(ticket)));
            }
            if expected != state.revision {
                return Ok(CommitOutcome::NotCommitted {
                    revision: state.revision,
                });
            }
            if state.receipt_count()? == self.limits.max_receipts {
                return Ok(CommitOutcome::Rejected(
                    OperationRejection::RecoveryWindowFull,
                ));
            }
            if changes.len() > self.limits.transaction.max_changes {
                return Err(RedbError::Capacity);
            }
            // No cache entry survives a possible edit, participant failure,
            // panic or ambiguous commit. Only confirmed success re-seeds it.
            validation::invalidate(self.validation.as_deref(), self.namespace);
            let mut staged_bytes = 0usize;
            for change in changes {
                let value_bytes = change.value().map_or(0, <[u8]>::len);
                staged_bytes = staged_bytes
                    .checked_add(change.key().len())
                    .and_then(|n| n.checked_add(value_bytes))
                    .ok_or(RedbError::Capacity)?;
                if change.key().len() > self.limits.transaction.max_key_bytes
                    || value_bytes > self.limits.transaction.max_value_bytes
                    || staged_bytes > self.limits.transaction.max_staged_bytes
                {
                    return Err(RedbError::Capacity);
                }
                if let Some(old) = values.remove(change.key()).map_err(db_error)? {
                    if old.value().len() > self.limits.transaction.max_value_bytes {
                        return Err(RedbError::Corrupt);
                    }
                    state.objects = state.objects.checked_sub(1).ok_or(RedbError::Corrupt)?;
                    state.bytes = state
                        .bytes
                        .checked_sub(
                            change
                                .key()
                                .len()
                                .checked_add(old.value().len())
                                .ok_or(RedbError::Corrupt)?,
                        )
                        .ok_or(RedbError::Corrupt)?;
                }
                if let Some(value) = change.value() {
                    state.objects = state.objects.checked_add(1).ok_or(RedbError::Capacity)?;
                    state.bytes = state
                        .bytes
                        .checked_add(change.key().len())
                        .and_then(|n| n.checked_add(value.len()))
                        .ok_or(RedbError::Capacity)?;
                    values.insert(change.key(), value).map_err(db_error)?;
                }
            }
            if let Err(reason) = self.admission.check(
                state.objects,
                state.bytes,
                state
                    .receipt_count()?
                    .checked_add(1)
                    .ok_or(RedbError::Capacity)?,
            ) {
                return Ok(CommitOutcome::Rejected(reason));
            }
            if state.objects > self.limits.max_live_objects
                || state.bytes > self.limits.max_live_bytes
            {
                return Err(RedbError::Capacity);
            }
            state.revision = state.revision.next().ok_or(RedbError::CounterExhausted)?;
            receipt = self.receipt(ticket);
            participant_ticket = ticket;
            if receipts
                .insert(ticket.sequence(), state.revision.0)
                .map_err(db_error)?
                .is_some()
            {
                return Err(RedbError::Corrupt);
            }
            meta.insert(0, state.encode()?.as_slice())
                .map_err(db_error)?;
            committed_state = state;
        }
        if let Some(participant) = &self.participant {
            let values = CommitRead {
                values: write.open_table(self.names.values()).map_err(db_error)?,
                limits: self.limits,
            };
            if let Some(reason) = participant.stage(
                &write,
                CommitContext {
                    namespace: self.namespace,
                    incarnation: self.incarnation,
                },
                &values,
                participant_ticket,
                changes,
            )? {
                return Ok(CommitOutcome::Rejected(reason));
            }
        }
        #[cfg(test)]
        tests::crash_point("before-commit");
        #[cfg(test)]
        validation::tests::fail_point("before-commit")?;
        change::commit_with_selection(write, self.change_witness.as_ref(), self.change_stamp)?;
        #[cfg(test)]
        validation::tests::fail_point("after-commit")?;
        #[cfg(test)]
        tests::crash_point("after-commit");
        validation::remember(self.validation.as_deref(), self.namespace, committed_state);
        Ok(CommitOutcome::Committed(receipt))
    }
}

impl MaintenanceBackend for RedbBackend<'_> {
    type Request = RedbMaintenanceRequest;
    type Report = RedbUsage;
    fn maintain(
        &mut self,
        request: Self::Request,
    ) -> Result<MaintenanceOutcome<Self::Report>, Self::Error> {
        let mut write = self.database.begin_write().map_err(db_error)?;
        self.require_tables(&write)?;
        write
            .set_durability(Durability::Immediate)
            .map_err(db_error)?;
        let usage;
        let committed_state;
        {
            let mut meta = write.open_table(self.names.metadata()).map_err(db_error)?;
            let values = write.open_table(self.names.values()).map_err(db_error)?;
            let mut receipts = write.open_table(self.names.receipts()).map_err(db_error)?;
            let mut state = State::load(&meta, self.incarnation, self.limits)?;
            validation::validate(
                self.validation.as_deref(),
                self.namespace,
                &state,
                &values,
                &receipts,
            )?;
            let RedbMaintenanceRequest::FinalizeThrough(ticket) = request;
            if ticket.incarnation() != self.incarnation
                || ticket.sequence() > state.revision.0
                || ticket.sequence() < state.floor
            {
                return Ok(MaintenanceOutcome::Rejected(state.usage()));
            }
            if ticket.sequence() == state.floor {
                return Ok(MaintenanceOutcome::Complete(state.usage()));
            }
            validation::invalidate(self.validation.as_deref(), self.namespace);
            for seq in state.floor..ticket.sequence() {
                let removed = receipts.remove(seq + 1).map_err(db_error)?;
                if removed.as_ref().map(|value| value.value()) != Some(seq + 1) {
                    return Err(RedbError::Corrupt);
                }
            }
            state.floor = ticket.sequence();
            meta.insert(0, state.encode()?.as_slice())
                .map_err(db_error)?;
            usage = state.usage();
            committed_state = state;
        }
        #[cfg(test)]
        tests::crash_point("before-retirement");
        #[cfg(test)]
        validation::tests::fail_point("before-retirement")?;
        change::commit_with_selection(write, self.change_witness.as_ref(), self.change_stamp)?;
        #[cfg(test)]
        validation::tests::fail_point("after-retirement")?;
        #[cfg(test)]
        tests::crash_point("after-retirement");
        validation::remember(self.validation.as_deref(), self.namespace, committed_state);
        Ok(MaintenanceOutcome::Complete(usage))
    }
    fn recover_maintenance(&mut self) -> Result<MaintenanceOutcome<Self::Report>, Self::Error> {
        Ok(MaintenanceOutcome::Complete(self.snapshot()?.usage()))
    }
}
