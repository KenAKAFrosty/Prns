//! Optional synchronous participants in one existing physical transaction.
use crate::{db_error, CheckedLimits, RedbError, StoreNamespace};
use prns_groups_store::{Mutation, OperationRejection, OperationTicket, StoreIncarnation};
use redb::{ReadableTable, Table, WriteTransaction};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitContext {
    pub namespace: StoreNamespace,
    pub incarnation: StoreIncarnation,
}

/// Borrowed access to the exact namespace after its mutations have been staged
/// in the participant's physical transaction. The backend creates this view
/// only after validating the namespace, incarnation, limits and commit ticket.
/// It exposes no mutation or transaction ownership and performs no table scans.
pub struct CommitRead<'txn> {
    pub(crate) values: Table<'txn, &'static [u8], &'static [u8]>,
    pub(crate) limits: CheckedLimits,
}
impl CommitRead<'_> {
    /// Inspect one bounded value without copying it or opening another database
    /// snapshot. Borrowed bytes cannot outlive the callback. Application key
    /// partitioning remains the owning layer's responsibility.
    pub fn with_value<T>(
        &self,
        key: &[u8],
        inspect: impl FnOnce(Option<&[u8]>) -> T,
    ) -> Result<T, RedbError> {
        if key.len() > self.limits.transaction.max_key_bytes {
            return Err(RedbError::Capacity);
        }
        let value = self.values.get(key).map_err(db_error)?;
        if value
            .as_ref()
            .is_some_and(|value| value.value().len() > self.limits.transaction.max_value_bytes)
        {
            return Err(RedbError::Corrupt);
        }
        Ok(inspect(value.as_ref().map(|value| value.value())))
    }
}

/// A trusted same-database participant, not a callback/event or recovery bus.
/// Called only for a fresh transaction after ticket/revision admission. Store
/// mutation guards are released; `values` provides read-only access to the
/// staged namespace. `ticket` is the exact already-validated
/// attempt whose receipt is staged in this transaction. The caller owns the sole commit. Never open
/// a nested write transaction, await, reenter the owner, or publish external work.
/// Returning a rejection aborts *all* staged writes without consuming the ticket;
/// errors retain the backend's conservative uncertain-outcome contract. A panic
/// unwinds the transaction and must never be translated into a committed result.
/// Participant tables are separately owned: never edit this backend's namespace
/// metadata, values or receipts. Its accounting and validation evidence cover
/// only mutations staged by the backend itself.
pub trait TransactionParticipant: Send + Sync {
    fn stage(
        &self,
        transaction: &WriteTransaction,
        context: CommitContext,
        values: &CommitRead<'_>,
        ticket: OperationTicket,
        changes: &[Mutation],
    ) -> Result<Option<OperationRejection>, RedbError>;

    /// The domain must durably fence the exact incarnation before calling delete.
    /// This is also called when native tables are absent, so a retried cleanup
    /// cannot silently leave independently selected participant state behind.
    fn delete_in(
        &self,
        transaction: &WriteTransaction,
        context: CommitContext,
    ) -> Result<(), RedbError>;
}
