//! Optional group-owned durable intent retention on the existing database lane.
//! Initialize explicitly in the selected-domain startup transaction. Merely
//! opening a group manager never creates this schema. No actor or runtime lives
//! here: the native assembly must supply real admission/exclusion and own leases.
mod codec;
mod dependencies;
mod mutation;
mod schema;
use crate::{Error, Intent, Outcome, Request, RequestId};
use codec::*;
use prns_groups_core::{GroupId, RejectionReason};
use prns_host::IdentityHash;
use redb::{
    Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
    WriteTransaction,
};
pub use schema::{initialize_in, inspect_in, validate_in};
use schema::{scope_key, Header, Snapshot};
use serde::{Deserialize, Serialize};

const HEADER: TableDefinition<u8, &[u8]> = TableDefinition::new("prns_group_operations_header_v1");
const OPERATIONS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("prns_group_operations_v1");
const VERSION: u32 = 3;
const ACTIVE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("prns_group_operations_active_v3");
const TERMINAL: TableDefinition<&[u8], u64> =
    TableDefinition::new("prns_group_operations_terminal_v3");
const MARKER: &[u8] = &[3];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupRetentionLimits"))]
pub struct Limits {
    /// Selected retained operation count, including completed results.
    pub max_operations: u32,
    /// Logical reservations including result headroom, not physical disk bytes.
    pub max_bytes: u64,
    /// Encoded size of one row; decoder/encoding scratch is bounded separately
    /// from returned page allocations. Not a whole-process memory guarantee.
    pub max_row_bytes: u32,
    /// Fixed Operation plus all retained Intent allocation capacities after decode.
    pub max_operation_bytes: u64,
    /// Maximum terminal candidates pruned by one acceptance transaction.
    pub max_prune_rows: u32,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_operations: 64,
            max_bytes: 8 * 1024 * 1024,
            max_row_bytes: 128 * 1024,
            max_operation_bytes: 128 * 1024,
            max_prune_rows: 32,
        }
    }
}
impl Limits {
    pub fn validate(self) -> Result<(), Error> {
        let row = usize::try_from(self.max_row_bytes).map_err(|_| Error::InvalidInput)?;
        let owned = usize::try_from(self.max_operation_bytes).map_err(|_| Error::InvalidInput)?;
        // Bound representation arithmetic and worst-case transient JSON decoding
        // separately. Row input, parser string scratch, geometric Vec growth and
        // fixed representation fit this conservative scratch selection. Native
        // allocator metadata and redb's page cache are outside these bounds.
        if self.max_operations == 0
            || self.max_bytes < 1024
            || row < 512
            || owned < std::mem::size_of::<Operation>()
            || owned > isize::MAX as usize
            || self.max_prune_rows == 0
            || row
                .checked_mul(8)
                .and_then(|n| n.checked_add(std::mem::size_of::<Operation>()))
                .is_none_or(|n| n > isize::MAX as usize)
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupOperationState"))]
pub enum State {
    /// Durable acceptance; no dispatch marker has committed yet.
    Pending,
    /// Dispatch may have happened. Absence of its reply does not prove rejection.
    Dispatched,
    /// Cause of the failed attempt, not proof of a rejected operation.
    Uncertain(prns_groups_core::ApiError),
    Completed(Outcome),
    Rejected(RejectionReason),
}
impl State {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed(_) | Self::Rejected(_))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupOperation"))]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub request: Request,
    pub state: State,
    pub scope: OperationScope,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupOperationScope"))]
pub enum OperationScope {
    Create,
    Group(GroupId),
}
/// Borrowed schema view on the existing exclusive database lane. Strict cold
/// validation is retained by Attachment; this view never owns another writer.
#[derive(Clone)]
pub(crate) struct Store<'db> {
    database: &'db Database,
    identity: IdentityHash,
    limits: Limits,
    change_witness: Option<prns_groups_store_redb::ChangeWitness>,
    change_stamp: Option<prns_groups_store::StoreIncarnation>,
}
impl<'db> Store<'db> {
    pub(crate) fn open(
        database: &'db Database,
        identity: IdentityHash,
    ) -> Result<Option<Self>, Error> {
        let read = database.begin_read().map_err(db_error)?;
        let limits = inspect_in(&read, identity)?;
        Self::open_attached(database, identity, limits)
    }
    /// Crate-private: selection comes only from strict Attachment readiness.
    /// The embedding still guarantees exclusive writes through these owners.
    pub(crate) fn open_attached(
        database: &'db Database,
        identity: IdentityHash,
        limits: Option<Limits>,
    ) -> Result<Option<Self>, Error> {
        let read = database.begin_read().map_err(db_error)?;
        let tables = Snapshot::open(&read, identity)?;
        match (limits, tables) {
            (None, None) => Ok(None),
            (Some(limits), Some(tables)) if limits == tables.header.limits => Ok(Some(Self {
                database,
                identity,
                limits,
                change_witness: None,
                change_stamp: None,
            })),
            _ => Err(Error::Configuration),
        }
    }
    pub(crate) fn with_change_witness(
        mut self,
        witness: prns_groups_store_redb::ChangeWitness,
    ) -> Self {
        self.change_witness = Some(witness);
        self
    }
    pub(crate) fn with_change_stamp(mut self, stamp: prns_groups_store::StoreIncarnation) -> Self {
        self.change_stamp = Some(stamp);
        self
    }
    fn snapshot(&self) -> Result<Snapshot, Error> {
        let read = self.database.begin_read().map_err(db_error)?;
        let tables = Snapshot::open(&read, self.identity)?.ok_or(Error::Corrupt)?;
        if tables.header.limits != self.limits {
            return Err(Error::Corrupt);
        }
        Ok(tables)
    }
    pub(crate) fn operation(&self, id: RequestId) -> Result<Option<Operation>, Error> {
        self.snapshot()?.operation(id)
    }
    pub(crate) fn list(
        &self,
        after: Option<RequestId>,
        limits: crate::PageLimits,
    ) -> Result<Vec<Operation>, Error> {
        let (rows, bytes) = limits.checked().map_err(Error::Api)?;
        let max_owned =
            usize::try_from(self.limits.max_operation_bytes).map_err(|_| Error::InvalidInput)?;
        if bytes < max_owned {
            return Err(Error::InvalidInput);
        }
        let fixed = std::mem::size_of::<Operation>();
        let slots = rows.min((bytes - (max_owned - fixed)) / fixed);
        let mut result = Vec::new();
        result
            .try_reserve_exact(slots)
            .map_err(|_| Error::StorageFull)?;
        let mut used = result
            .capacity()
            .checked_mul(fixed)
            .ok_or(Error::InvalidInput)?;
        if used
            .checked_add(max_owned - fixed)
            .is_none_or(|n| n > bytes)
        {
            return Err(Error::InvalidInput);
        }
        let snapshot = self.snapshot()?;
        let start = after.as_ref().map_or(b"".as_slice(), |id| id.0.as_slice());
        for item in snapshot
            .operations
            .range::<&[u8]>((std::ops::Bound::Excluded(start), std::ops::Bound::Unbounded))
            .map_err(db_error)?
            .take(slots)
        {
            let (key, value) = item.map_err(db_error)?;
            let operation = decode_operation(key.value(), value.value(), self.limits)?;
            snapshot.check_index(&operation)?;
            let heap = crate::client::intent_heap_bytes(&operation.request.intent)?;
            let next = used.checked_add(heap).ok_or(Error::Corrupt)?;
            if next > bytes {
                break;
            }
            used = next;
            result.push(operation);
        }
        Ok(result)
    }
    pub(crate) fn check(&self, request: &Request, scope: OperationScope) -> Result<(), Error> {
        let snapshot = self.snapshot()?;
        if let Some(operation) = snapshot.operation(request.id)? {
            if operation.request != *request {
                return Err(Error::IdempotencyConflict);
            }
        }
        if let Some(id) = snapshot
            .active
            .get(scope_key(scope).as_slice())
            .map_err(db_error)?
        {
            if id.value() != request.id.0 {
                return Err(Error::Busy);
            }
        }
        Ok(())
    }
    pub(crate) fn validate_new(
        &self,
        id: RequestId,
        context: crate::ExecutionContext,
    ) -> Result<(), Error> {
        let snapshot = self.snapshot()?;
        if snapshot.operation(id)?.is_some() {
            return Ok(());
        }
        validate_token(id, context, snapshot.header.retired_before)
    }
}
fn validate_token(
    id: RequestId,
    context: crate::ExecutionContext,
    floor: u64,
) -> Result<(), Error> {
    let prns_groups_core::ClockReading::Trusted(clock) = context.clock else {
        return Err(Error::Clock);
    };
    crate::profile::RETRY_POLICY
        .validate_new(id.0, clock.lower().0, floor)
        .map_err(Error::Retry)
}
fn db_error(error: impl Into<redb::Error>) -> Error {
    Error::Storage(prns_groups_store_redb::RedbError::Database(error.into()))
}

#[cfg(test)]
pub(crate) mod fault {
    use super::Error;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Cut {
        Accepted,
        Dispatched,
        DomainFinished,
        OutcomeSaved,
        BeforeStorageCommit,
        AfterStorageCommit,
    }
    thread_local! {static NEXT:std::cell::Cell<Option<Cut>> = const {std::cell::Cell::new(None)};}
    pub fn set(cut: Cut) {
        NEXT.with(|next| next.set(Some(cut)));
    }
    pub fn cut(cut: Cut) -> Result<(), Error> {
        NEXT.with(|next| {
            if next.get() == Some(cut) {
                next.set(None);
                Err(Error::Unavailable)
            } else {
                Ok(())
            }
        })
    }
}
#[cfg(test)]
#[path = "retained/tests.rs"]
mod tests;

#[cfg(test)]
mod work {
    thread_local! { static COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) }; }
    pub fn decoded() {
        COUNTS.with(|c| {
            let (r, p) = c.get();
            c.set((r + 1, p));
        });
    }
    pub fn pruned() {
        COUNTS.with(|c| {
            let (r, p) = c.get();
            c.set((r, p + 1));
        });
    }
    pub fn take() -> (usize, usize) {
        COUNTS.with(|c| c.replace((0, 0)))
    }
}
#[cfg(test)]
mod indexed_tests;
