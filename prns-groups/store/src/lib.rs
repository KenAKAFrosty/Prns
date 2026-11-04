//! A bounded, explicitly recovered transaction shared by crypto and application adapters.
//!
//! [`AtomicBackend`] owns durable
//! atomicity, receipts and bounded reads. Payload accounting excludes allocator/Vec
//! headers; staging replacement temporarily holds both old and incoming buffers.
//! The one-time `Arc` allocation in [`Coordinator::new`] is infallible. All staged
//! payload copies and mutation-vector growth use fallible reservation.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::{sync::Arc, vec::Vec};
use core::{fmt, mem};
use spin::Mutex;
use zeroize::Zeroize;

pub use prns_groups_core::{
    checked_charge, CapacityError, GroupId, OperationId, OperationTicket, Resource, Revision,
    StoreIncarnation, TicketError,
};

pub mod flash;
#[cfg(feature = "secure")]
pub mod secure;

/// Limits for one transaction and one returned value. Zero permits no usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_changes: usize,
    pub max_key_bytes: usize,
    pub max_value_bytes: usize,
    pub max_staged_bytes: usize,
}

/// Owned staged bytes, scrubbed on replacement/abort/completion. Debug omits bytes.
pub enum Mutation {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

impl Mutation {
    pub fn key(&self) -> &[u8] {
        match self {
            Self::Put { key, .. } | Self::Delete { key } => key,
        }
    }

    pub fn value(&self) -> Option<&[u8]> {
        match self {
            Self::Put { value, .. } => Some(value),
            Self::Delete { .. } => None,
        }
    }

    fn bytes(&self) -> usize {
        // Construction checked this sum before storing the mutation.
        self.key().len() + self.value().map_or(0, <[u8]>::len)
    }
}

impl fmt::Debug for Mutation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mutation")
            .field("key_bytes", &self.key().len())
            .field("value_bytes", &self.value().map(<[u8]>::len))
            .finish()
    }
}

impl Drop for Mutation {
    fn drop(&mut self) {
        match self {
            Self::Put { key, value } => {
                key.zeroize();
                value.zeroize();
            }
            Self::Delete { key } => key.zeroize(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitReceipt {
    pub operation_id: OperationId,
    pub revision: Revision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitOutcome {
    Committed(CommitReceipt),
    NotCommitted {
        revision: Revision,
    },
    /// Input was rejected before applying any changes or consuming the ticket.
    Rejected(OperationRejection),
    Uncertain,
}

/// A definite input rejection, never evidence about an uncertain prior commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationRejection {
    InvalidTicket,
    ForeignIncarnation,
    OutOfOrder {
        next: Option<OperationTicket>,
    },
    RecoveryWindowFull,
    /// The caller's admission reserve would be consumed by this new write.
    ReservedCapacity,
    /// A selected transaction participant cannot retain this new data. No
    /// changes took effect and the operation ticket remains unconsumed.
    StorageCapacity,
    RetiredTicket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryOutcome {
    Committed(CommitReceipt),
    NotCommitted {
        revision: Revision,
    },
    /// The ticket is permanently consumed but its result is no longer retained.
    /// This must never be interpreted as permission to execute it again.
    Retired {
        revision: Revision,
        retired_through: OperationTicket,
    },
    Rejected(OperationRejection),
    Unresolved,
}

/// Exclusively owned durable storage. No other writer may bypass this coordinator.
///
/// A successful commit atomically installs ALL changes and a receipt at exactly
/// `expected.next()`. `NotCommitted` guarantees none of the changes took effect.
/// Any error or `Uncertain` has an unknown outcome, resolved only by `recover`.
/// Receipts are durable and operation IDs must not be reused for different work.
/// Recovery must not mistake missing/corrupt evidence for definite non-commit.
/// A reused committed ID must never apply a second set of changes.
/// `Rejected` guarantees no new mutation, and cannot replace recovery evidence
/// for an earlier uncertain commit. Scoped backends return `Retired` for consumed
/// tickets below their durable receipt floor, never `NotCommitted`.
///
/// `read` must bound the encoded value before allocating, honor `max_bytes`, and
/// return fallibly allocated owned bytes. It must not reveal uncommitted changes.
/// The caller owns zeroization of the returned buffer. Implementations must also
/// account for their own indexing/journal/compaction allocations and secret copies.
pub trait AtomicBackend {
    type Error;

    fn head(&mut self) -> Result<Revision, Self::Error>;
    fn read(&mut self, key: &[u8], max_bytes: usize) -> Result<Option<Vec<u8>>, Self::Error>;
    fn commit(
        &mut self,
        expected: Revision,
        operation: OperationId,
        changes: &[Mutation],
    ) -> Result<CommitOutcome, Self::Error>;
    fn recover(&mut self, operation: OperationId) -> Result<RecoveryOutcome, Self::Error>;
}

/// Maintenance changes storage representation or finalized receipt retention,
/// never application values or the application revision. Backend-owned request
/// types expose only supported maintenance, not unrestricted mutable access.
pub trait MaintenanceBackend: AtomicBackend {
    type Request;
    type Report;

    fn maintain(
        &mut self,
        request: Self::Request,
    ) -> Result<MaintenanceOutcome<Self::Report>, Self::Error>;

    /// Resolve any interrupted maintenance, including after reopening. Errors or
    /// `Uncertain` preserve the fence. A known uncertain attempt cannot resolve
    /// to `Rejected`; its publication and required cleanup must be examined.
    fn recover_maintenance(&mut self) -> Result<MaintenanceOutcome<Self::Report>, Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaintenanceOutcome<R> {
    Complete(R),
    /// A bounded step completed definitely; storage is usable between steps.
    Pending(R),
    /// Rejected before any maintenance mutation.
    Rejected(R),
    Uncertain,
}

#[derive(Debug)]
pub enum StoreError<E> {
    Backend(E),
    Capacity(CapacityError),
    AllocationFailed,
    Busy,
    StaleTransaction,
    RecoveryRequired(OperationId),
    MaintenanceRequired,
    WrongRecoveryOperation,
    RevisionMismatch {
        expected: Revision,
        actual: Revision,
    },
    AlreadyCommitted(CommitReceipt),
    OperationRejected(OperationRejection),
    OutsideRecoveryWindow {
        retired_through: OperationTicket,
    },
    CounterExhausted,
    BackendContractViolation,
}

impl<E: fmt::Debug> fmt::Display for StoreError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "group storage error: {self:?}")
    }
}

impl<E: fmt::Debug> core::error::Error for StoreError<E> {}

impl<E> From<CapacityError> for StoreError<E> {
    fn from(error: CapacityError) -> Self {
        Self::Capacity(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub changes: usize,
    pub bytes: usize,
}

struct Active {
    generation: u64,
    operation: OperationId,
    expected: Revision,
    changes: Vec<Mutation>,
    bytes: usize,
}

enum Phase {
    Idle,
    Active(Active),
    Poisoned {
        operation: OperationId,
        expected: Option<Revision>,
        known_uncertain: bool,
    },
    MaintenancePoisoned,
}

struct State<B> {
    backend: B,
    limits: Limits,
    revision: Revision,
    generation: u64,
    phase: Phase,
}

/// One serialized owner of a backend. Do not call it reentrantly from the backend
/// or an interrupt: the portable mutex spins. Platform scheduling remains external.
pub struct Coordinator<B> {
    state: Arc<Mutex<State<B>>>,
}

impl<B> Clone for Coordinator<B> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

/// Clones share an exclusive transaction token, not copies of its state. A handle
/// is invalid after abort/commit/recovery; dropping it does NOT implicitly commit
/// or abort. Call `abort` before abandoning an active operation.
pub struct Transaction<B> {
    state: Arc<Mutex<State<B>>>,
    generation: u64,
    operation: OperationId,
}

impl<B> Clone for Transaction<B> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            generation: self.generation,
            operation: self.operation,
        }
    }
}

impl<B: AtomicBackend> Coordinator<B> {
    pub fn new(mut backend: B, limits: Limits) -> Result<Self, StoreError<B::Error>> {
        let revision = backend.head().map_err(StoreError::Backend)?;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                backend,
                limits,
                revision,
                generation: 0,
                phase: Phase::Idle,
            })),
        })
    }

    pub fn head(&self) -> Result<Revision, StoreError<B::Error>> {
        let state = self.state.lock();
        match state.phase {
            Phase::Poisoned { operation, .. } => {
                return Err(StoreError::RecoveryRequired(operation))
            }
            Phase::MaintenancePoisoned => return Err(StoreError::MaintenanceRequired),
            Phase::Idle | Phase::Active(_) => {}
        }
        Ok(state.revision)
    }

    /// Read committed bytes for retry/reload without opening a transaction.
    /// Active attempts and uncertain commits are fenced. Returned secret bytes
    /// belong to the caller and should be wrapped in a zeroizing buffer promptly.
    pub fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError<B::Error>> {
        let mut state = self.state.lock();
        match state.phase {
            Phase::Active(_) => return Err(StoreError::Busy),
            Phase::MaintenancePoisoned => return Err(StoreError::MaintenanceRequired),
            Phase::Poisoned { operation, .. } => {
                return Err(StoreError::RecoveryRequired(operation))
            }
            Phase::Idle => {}
        }
        checked_charge(
            0,
            0,
            key.len(),
            state.limits.max_key_bytes,
            Resource::KeyBytes,
        )?;
        read_backend(&mut state, key)
    }

    pub fn begin(
        &self,
        operation: OperationId,
        expected: Revision,
    ) -> Result<Transaction<B>, StoreError<B::Error>> {
        let mut state = self.state.lock();
        match state.phase {
            Phase::Active(_) => return Err(StoreError::Busy),
            Phase::MaintenancePoisoned => return Err(StoreError::MaintenanceRequired),
            Phase::Poisoned { operation, .. } => {
                return Err(StoreError::RecoveryRequired(operation))
            }
            Phase::Idle => {}
        }
        let actual = state.backend.head().map_err(StoreError::Backend)?;
        if actual < state.revision {
            state.phase = Phase::Poisoned {
                operation,
                expected: Some(state.revision),
                known_uncertain: true,
            };
            return Err(StoreError::BackendContractViolation);
        }
        state.revision = actual;
        if actual != expected {
            return Err(StoreError::RevisionMismatch { expected, actual });
        }
        state.phase = Phase::Poisoned {
            operation,
            expected: None,
            known_uncertain: false,
        };
        match state
            .backend
            .recover(operation)
            .map_err(StoreError::Backend)?
        {
            RecoveryOutcome::Committed(receipt)
                if receipt.operation_id == operation
                    && receipt.revision.0 != 0
                    && receipt.revision <= actual =>
            {
                state.phase = Phase::Idle;
                return Err(StoreError::AlreadyCommitted(receipt));
            }
            RecoveryOutcome::NotCommitted { revision } if revision == actual => {
                state.phase = Phase::Idle
            }
            RecoveryOutcome::Retired {
                revision,
                retired_through,
            } if revision == actual && valid_retirement(operation, retired_through) => {
                state.phase = Phase::Idle;
                return Err(StoreError::OutsideRecoveryWindow { retired_through });
            }
            RecoveryOutcome::Rejected(reason) => {
                state.phase = Phase::Idle;
                return Err(StoreError::OperationRejected(reason));
            }
            RecoveryOutcome::Unresolved => {
                state.phase = Phase::Poisoned {
                    operation,
                    expected: None,
                    known_uncertain: true,
                };
                return Err(StoreError::RecoveryRequired(operation));
            }
            _ => {
                state.phase = Phase::Poisoned {
                    operation,
                    expected: None,
                    known_uncertain: true,
                };
                return Err(StoreError::BackendContractViolation);
            }
        }
        if actual.next().is_none() {
            return Err(StoreError::CounterExhausted);
        }
        let generation = state
            .generation
            .checked_add(1)
            .ok_or(StoreError::CounterExhausted)?;
        state.generation = generation;
        state.phase = Phase::Active(Active {
            generation,
            operation,
            expected,
            changes: Vec::new(),
            bytes: 0,
        });
        Ok(Transaction {
            state: self.state.clone(),
            generation,
            operation,
        })
    }

    /// Resolve the uncertain operation, or inspect one after reopening the backend.
    /// An error/Unresolved retains the fence; a definitive result revokes old handles.
    pub fn recover(&self, operation: OperationId) -> Result<RecoveryOutcome, StoreError<B::Error>> {
        let mut state = self.state.lock();
        let (expected, known_uncertain) = match state.phase {
            Phase::Active(_) => return Err(StoreError::Busy),
            Phase::MaintenancePoisoned => return Err(StoreError::MaintenanceRequired),
            Phase::Poisoned {
                operation: pending,
                expected,
                known_uncertain,
            } => {
                if operation != pending {
                    return Err(StoreError::WrongRecoveryOperation);
                }
                (expected, known_uncertain)
            }
            Phase::Idle => (None, false),
        };
        state.phase = Phase::Poisoned {
            operation,
            expected,
            known_uncertain,
        };
        let outcome = state
            .backend
            .recover(operation)
            .map_err(StoreError::Backend)?;
        let revision = match outcome {
            RecoveryOutcome::Committed(receipt) => {
                if receipt.operation_id != operation
                    || receipt.revision.0 == 0
                    || expected.is_some_and(|r| r.next() != Some(receipt.revision))
                {
                    return Err(StoreError::BackendContractViolation);
                }
                let head = state.backend.head().map_err(StoreError::Backend)?;
                if head < receipt.revision || head < state.revision {
                    return Err(StoreError::BackendContractViolation);
                }
                head
            }
            RecoveryOutcome::NotCommitted { revision } => {
                if revision < state.revision || expected.is_some_and(|r| revision < r) {
                    return Err(StoreError::BackendContractViolation);
                }
                let head = state.backend.head().map_err(StoreError::Backend)?;
                if head != revision {
                    return Err(StoreError::BackendContractViolation);
                }
                revision
            }
            RecoveryOutcome::Retired {
                revision,
                retired_through,
            } => {
                if known_uncertain
                    || revision < state.revision
                    || !valid_retirement(operation, retired_through)
                {
                    return Err(StoreError::BackendContractViolation);
                }
                if state.backend.head().map_err(StoreError::Backend)? != revision {
                    return Err(StoreError::BackendContractViolation);
                }
                revision
            }
            RecoveryOutcome::Rejected(_) => {
                if known_uncertain {
                    return Err(StoreError::BackendContractViolation);
                }
                let head = state.backend.head().map_err(StoreError::Backend)?;
                if head < state.revision {
                    return Err(StoreError::BackendContractViolation);
                }
                head
            }
            RecoveryOutcome::Unresolved => {
                state.phase = Phase::Poisoned {
                    operation,
                    expected,
                    known_uncertain: true,
                };
                return Ok(outcome);
            }
        };
        state.revision = revision;
        state.phase = Phase::Idle;
        Ok(outcome)
    }
}

impl<B: MaintenanceBackend> Coordinator<B> {
    /// Run one explicitly supported maintenance step while no transaction or
    /// recovery is outstanding. Backend errors fence every other access.
    pub fn maintain(
        &self,
        request: B::Request,
    ) -> Result<MaintenanceOutcome<B::Report>, StoreError<B::Error>> {
        let mut state = self.state.lock();
        match state.phase {
            Phase::Idle => {}
            Phase::Active(_) => return Err(StoreError::Busy),
            Phase::Poisoned { operation, .. } => {
                return Err(StoreError::RecoveryRequired(operation))
            }
            Phase::MaintenancePoisoned => return Err(StoreError::MaintenanceRequired),
        }
        state.phase = Phase::MaintenancePoisoned;
        let outcome = state
            .backend
            .maintain(request)
            .map_err(StoreError::Backend)?;
        finish_maintenance(&mut state, outcome, false)
    }

    pub fn recover_maintenance(
        &self,
    ) -> Result<MaintenanceOutcome<B::Report>, StoreError<B::Error>> {
        let mut state = self.state.lock();
        let uncertain = match state.phase {
            Phase::Idle => false,
            Phase::MaintenancePoisoned => true,
            Phase::Active(_) => return Err(StoreError::Busy),
            Phase::Poisoned { operation, .. } => {
                return Err(StoreError::RecoveryRequired(operation))
            }
        };
        state.phase = Phase::MaintenancePoisoned;
        let outcome = state
            .backend
            .recover_maintenance()
            .map_err(StoreError::Backend)?;
        finish_maintenance(&mut state, outcome, uncertain)
    }
}

fn finish_maintenance<B: MaintenanceBackend>(
    state: &mut State<B>,
    outcome: MaintenanceOutcome<B::Report>,
    uncertain: bool,
) -> Result<MaintenanceOutcome<B::Report>, StoreError<B::Error>> {
    match outcome {
        MaintenanceOutcome::Uncertain => return Ok(outcome),
        MaintenanceOutcome::Rejected(_) if uncertain => {
            return Err(StoreError::BackendContractViolation)
        }
        MaintenanceOutcome::Complete(_)
        | MaintenanceOutcome::Pending(_)
        | MaintenanceOutcome::Rejected(_) => {}
    }
    if state.backend.head().map_err(StoreError::Backend)? != state.revision {
        return Err(StoreError::BackendContractViolation);
    }
    state.phase = Phase::Idle;
    Ok(outcome)
}

fn valid_retirement(operation: OperationId, retired_through: OperationTicket) -> bool {
    OperationTicket::from_operation_id(operation).is_ok_and(|ticket| {
        ticket.incarnation() == retired_through.incarnation()
            && ticket.sequence() <= retired_through.sequence()
    })
}

impl<B: AtomicBackend> Transaction<B> {
    pub fn operation_id(&self) -> OperationId {
        self.operation
    }

    fn active<'a>(&self, state: &'a State<B>) -> Result<&'a Active, StoreError<B::Error>> {
        match &state.phase {
            Phase::Active(active)
                if active.generation == self.generation && active.operation == self.operation =>
            {
                Ok(active)
            }
            Phase::Poisoned { operation, .. } => Err(StoreError::RecoveryRequired(*operation)),
            Phase::MaintenancePoisoned => Err(StoreError::MaintenanceRequired),
            _ => Err(StoreError::StaleTransaction),
        }
    }

    pub fn usage(&self) -> Result<Usage, StoreError<B::Error>> {
        let state = self.state.lock();
        let active = self.active(&state)?;
        Ok(Usage {
            changes: active.changes.len(),
            bytes: active.bytes,
        })
    }

    pub fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError<B::Error>> {
        let mut state = self.state.lock();
        let active = self.active(&state)?;
        checked_charge(
            0,
            0,
            key.len(),
            state.limits.max_key_bytes,
            Resource::KeyBytes,
        )?;
        if let Some(change) = active.changes.iter().find(|change| change.key() == key) {
            return change.value().map(copy_bytes).transpose();
        }
        read_backend(&mut state, key)
    }

    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<(), StoreError<B::Error>> {
        self.stage(key, Some(value))
    }

    pub fn delete(&self, key: &[u8]) -> Result<(), StoreError<B::Error>> {
        self.stage(key, None)
    }

    fn stage(&self, key: &[u8], value: Option<&[u8]>) -> Result<(), StoreError<B::Error>> {
        let mut state = self.state.lock();
        let active = self.active(&state)?;
        let limits = state.limits;
        checked_charge(0, 0, key.len(), limits.max_key_bytes, Resource::KeyBytes)?;
        let value_len = value.map_or(0, <[u8]>::len);
        checked_charge(
            0,
            0,
            value_len,
            limits.max_value_bytes,
            Resource::ValueBytes,
        )?;
        let bytes = key
            .len()
            .checked_add(value_len)
            .ok_or(CapacityError::ArithmeticOverflow(Resource::StagedBytes))?;
        let index = active.changes.iter().position(|change| change.key() == key);
        let removed = index.map_or(0, |i| active.changes[i].bytes());
        let total = checked_charge(
            active.bytes,
            removed,
            bytes,
            limits.max_staged_bytes,
            Resource::StagedBytes,
        )?;
        checked_charge(
            active.changes.len(),
            0,
            usize::from(index.is_none()),
            limits.max_changes,
            Resource::Changes,
        )?;
        // Own a zeroizing mutation before the second allocation can fail.
        let mut change = Mutation::Delete {
            key: copy_bytes(key)?,
        };
        if let Some(value) = value {
            let copied_value = copy_bytes(value)?;
            if let Mutation::Delete { key } = &mut change {
                change = Mutation::Put {
                    key: mem::take(key),
                    value: copied_value,
                };
            }
        }
        let Phase::Active(active) = &mut state.phase else {
            return Err(StoreError::StaleTransaction);
        };
        if let Some(index) = index {
            active.changes[index] = change;
        } else {
            active
                .changes
                .try_reserve_exact(1)
                .map_err(|_| StoreError::AllocationFailed)?;
            active.changes.push(change);
        }
        active.bytes = total;
        Ok(())
    }

    pub fn abort(&self) -> Result<(), StoreError<B::Error>> {
        let mut state = self.state.lock();
        self.active(&state)?;
        state.phase = Phase::Idle;
        Ok(())
    }

    pub fn commit(&self) -> Result<CommitOutcome, StoreError<B::Error>> {
        let mut state = self.state.lock();
        let expected = self.active(&state)?.expected;
        // Fence before calling the backend: errors and unwinding cannot permit reuse.
        let Phase::Active(active) = mem::replace(
            &mut state.phase,
            Phase::Poisoned {
                operation: self.operation,
                expected: Some(expected),
                known_uncertain: true,
            },
        ) else {
            return Err(StoreError::StaleTransaction);
        };
        let outcome = state
            .backend
            .commit(expected, self.operation, &active.changes)
            .map_err(StoreError::Backend)?;
        match outcome {
            CommitOutcome::Committed(receipt) => {
                if receipt.operation_id != self.operation
                    || expected.next() != Some(receipt.revision)
                {
                    return Err(StoreError::BackendContractViolation);
                }
                state.revision = receipt.revision;
                state.phase = Phase::Idle;
            }
            CommitOutcome::NotCommitted { revision } => {
                if revision < expected {
                    return Err(StoreError::BackendContractViolation);
                }
                state.revision = revision;
                state.phase = Phase::Idle;
            }
            CommitOutcome::Rejected(_) => {
                if state.backend.head().map_err(StoreError::Backend)? != expected {
                    return Err(StoreError::BackendContractViolation);
                }
                state.phase = Phase::Idle;
            }
            CommitOutcome::Uncertain => {}
        }
        Ok(outcome)
    }
}

fn read_backend<B: AtomicBackend>(
    state: &mut State<B>,
    key: &[u8],
) -> Result<Option<Vec<u8>>, StoreError<B::Error>> {
    let max = state.limits.max_value_bytes;
    let mut value = state.backend.read(key, max).map_err(StoreError::Backend)?;
    if value.as_ref().is_some_and(|value| value.len() > max) {
        if let Some(value) = &mut value {
            value.zeroize();
        }
        return Err(StoreError::BackendContractViolation);
    }
    Ok(value)
}

fn copy_bytes<E>(bytes: &[u8]) -> Result<Vec<u8>, StoreError<E>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| StoreError::AllocationFailed)?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

#[cfg(test)]
mod tests;
