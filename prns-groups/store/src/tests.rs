#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use alloc::{collections::BTreeMap, format, vec};

// An atomic map fake for coordinator invariants, not persistence/power-loss evidence.
#[derive(Clone, Copy, Default)]
enum Fault {
    #[default]
    None,
    NotCommitted,
    UncertainBefore,
    UncertainAfter,
    ErrorAfter,
    WrongReceipt,
}

#[derive(Default)]
struct Backend {
    values: BTreeMap<Vec<u8>, Vec<u8>>,
    receipts: BTreeMap<OperationId, CommitReceipt>,
    revision: Revision,
    fault: Fault,
    unresolved: bool,
    recovery_error: bool,
    recovery_override: Option<RecoveryOutcome>,
    commit_rejection: Option<OperationRejection>,
    maintenance: MaintenanceMode,
    maintenance_calls: usize,
}

impl AtomicBackend for Backend {
    type Error = &'static str;

    fn head(&mut self) -> Result<Revision, Self::Error> {
        Ok(self.revision)
    }

    fn read(&mut self, key: &[u8], max_bytes: usize) -> Result<Option<Vec<u8>>, Self::Error> {
        self.values
            .get(key)
            .map(|value| {
                if value.len() > max_bytes {
                    return Err("value limit");
                }
                let mut result = Vec::new();
                result
                    .try_reserve_exact(value.len())
                    .map_err(|_| "allocation")?;
                result.extend_from_slice(value);
                Ok(result)
            })
            .transpose()
    }

    fn commit(
        &mut self,
        expected: Revision,
        operation: OperationId,
        changes: &[Mutation],
    ) -> Result<CommitOutcome, Self::Error> {
        if let Some(reason) = self.commit_rejection {
            return Ok(CommitOutcome::Rejected(reason));
        }
        if let Some(receipt) = self.receipts.get(&operation) {
            return Ok(CommitOutcome::Committed(*receipt));
        }
        if expected != self.revision || matches!(self.fault, Fault::NotCommitted) {
            return Ok(CommitOutcome::NotCommitted {
                revision: self.revision,
            });
        }
        if matches!(self.fault, Fault::UncertainBefore) {
            return Ok(CommitOutcome::Uncertain);
        }
        for change in changes {
            match change {
                Mutation::Put { key, value } => {
                    self.values.insert(key.clone(), value.clone());
                }
                Mutation::Delete { key } => {
                    self.values.remove(key);
                }
            }
        }
        self.revision = self.revision.next().ok_or("revision exhausted")?;
        let receipt = CommitReceipt {
            operation_id: operation,
            revision: self.revision,
        };
        self.receipts.insert(operation, receipt);
        match self.fault {
            Fault::UncertainAfter => Ok(CommitOutcome::Uncertain),
            Fault::ErrorAfter => Err("lost result"),
            Fault::WrongReceipt => Ok(CommitOutcome::Committed(CommitReceipt {
                operation_id: OperationId([99; 32]),
                ..receipt
            })),
            _ => Ok(CommitOutcome::Committed(receipt)),
        }
    }

    fn recover(&mut self, operation: OperationId) -> Result<RecoveryOutcome, Self::Error> {
        if self.recovery_error {
            return Err("recovery I/O");
        }
        if self.unresolved {
            return Ok(RecoveryOutcome::Unresolved);
        }
        if let Some(outcome) = self.recovery_override {
            return Ok(outcome);
        }
        Ok(self.receipts.get(&operation).map_or(
            RecoveryOutcome::NotCommitted {
                revision: self.revision,
            },
            |receipt| RecoveryOutcome::Committed(*receipt),
        ))
    }
}

#[derive(Clone, Copy, Default)]
enum MaintenanceMode {
    #[default]
    Complete,
    Pending,
    Rejected,
    Uncertain,
    Error,
    MoveHead(Revision),
}

impl Backend {
    fn maintenance_outcome(&mut self) -> Result<MaintenanceOutcome<usize>, &'static str> {
        self.maintenance_calls += 1;
        match self.maintenance {
            MaintenanceMode::Complete => Ok(MaintenanceOutcome::Complete(self.maintenance_calls)),
            MaintenanceMode::Pending => Ok(MaintenanceOutcome::Pending(self.maintenance_calls)),
            MaintenanceMode::Rejected => Ok(MaintenanceOutcome::Rejected(self.maintenance_calls)),
            MaintenanceMode::Uncertain => Ok(MaintenanceOutcome::Uncertain),
            MaintenanceMode::Error => Err("maintenance I/O"),
            MaintenanceMode::MoveHead(revision) => {
                self.revision = revision;
                Ok(MaintenanceOutcome::Complete(self.maintenance_calls))
            }
        }
    }
}

impl MaintenanceBackend for Backend {
    type Request = MaintenanceMode;
    type Report = usize;

    fn maintain(
        &mut self,
        request: Self::Request,
    ) -> Result<MaintenanceOutcome<Self::Report>, Self::Error> {
        self.maintenance = request;
        self.maintenance_outcome()
    }

    fn recover_maintenance(&mut self) -> Result<MaintenanceOutcome<Self::Report>, Self::Error> {
        self.maintenance_outcome()
    }
}

#[test]
fn unresolved_and_failed_recovery_preserve_the_fence() {
    let store = coordinator(Fault::UncertainAfter);
    let tx = store.begin(op(1), Revision(0)).unwrap();
    tx.put(b"a", b"durable").unwrap();
    assert_eq!(tx.commit().unwrap(), CommitOutcome::Uncertain);
    store.state.lock().backend.unresolved = true;
    assert_eq!(store.recover(op(1)).unwrap(), RecoveryOutcome::Unresolved);
    assert!(matches!(store.head(), Err(StoreError::RecoveryRequired(_))));
    {
        let mut state = store.state.lock();
        state.backend.unresolved = false;
        state.backend.recovery_error = true;
    }
    assert!(matches!(
        store.recover(op(1)),
        Err(StoreError::Backend("recovery I/O"))
    ));
    assert!(matches!(
        store.read(b"a"),
        Err(StoreError::RecoveryRequired(_))
    ));
    store.state.lock().backend.recovery_error = false;
    assert!(matches!(
        store.recover(op(1)).unwrap(),
        RecoveryOutcome::Committed(_)
    ));
    assert_eq!(store.read(b"a").unwrap(), Some(b"durable".to_vec()));
}

#[test]
fn a_backend_revision_cannot_move_back_within_a_live_coordinator() {
    let store = coordinator(Fault::None);
    store.begin(op(1), Revision(0)).unwrap().commit().unwrap();
    store.state.lock().backend.revision = Revision(0);
    assert!(matches!(
        store.begin(op(2), Revision(0)),
        Err(StoreError::BackendContractViolation)
    ));
    assert!(matches!(store.head(), Err(StoreError::RecoveryRequired(_))));
    assert!(matches!(
        store.recover(op(2)),
        Err(StoreError::BackendContractViolation)
    ));
}

fn limits() -> Limits {
    Limits {
        max_changes: 8,
        max_key_bytes: 32,
        max_value_bytes: 64,
        max_staged_bytes: 256,
    }
}

fn coordinator(fault: Fault) -> Coordinator<Backend> {
    Coordinator::new(
        Backend {
            fault,
            ..Backend::default()
        },
        limits(),
    )
    .unwrap()
}

fn op(n: u8) -> OperationId {
    OperationId([n; 32])
}

#[test]
fn adapters_share_read_your_writes_and_one_commit() {
    let store = coordinator(Fault::None);
    let tx = store.begin(op(1), Revision(0)).unwrap();
    let crypto = tx.clone();
    let application = tx.clone();
    crypto.put(b"crypto", b"ratchet advanced").unwrap();
    crypto.delete(b"keypackage").unwrap();
    application.put(b"app", b"accepted").unwrap();
    application.put(b"outbox", b"exact bytes").unwrap();
    assert_eq!(
        application.read(b"crypto").unwrap(),
        Some(b"ratchet advanced".to_vec())
    );
    assert_eq!(application.read(b"keypackage").unwrap(), None);
    assert!(matches!(store.read(b"app"), Err(StoreError::Busy)));
    let receipt = CommitReceipt {
        operation_id: op(1),
        revision: Revision(1),
    };
    assert_eq!(tx.commit().unwrap(), CommitOutcome::Committed(receipt));
    assert!(matches!(
        crypto.put(b"bad", b"late"),
        Err(StoreError::StaleTransaction)
    ));
    assert!(matches!(
        application.commit(),
        Err(StoreError::StaleTransaction)
    ));
    assert_eq!(
        store.read(b"outbox").unwrap(),
        Some(b"exact bytes".to_vec())
    );
    assert_eq!(store.head().unwrap(), Revision(1));
    assert_eq!(
        store.recover(op(1)).unwrap(),
        RecoveryOutcome::Committed(receipt)
    );
}

#[test]
fn replacement_and_delete_are_coalesced_with_checked_accounting() {
    let store = coordinator(Fault::None);
    let tx = store.begin(op(1), Revision(0)).unwrap();
    tx.put(b"key", b"abc").unwrap();
    assert_eq!(
        tx.usage().unwrap(),
        Usage {
            changes: 1,
            bytes: 6
        }
    );
    tx.put(b"key", b"abcd").unwrap();
    assert_eq!(
        tx.usage().unwrap(),
        Usage {
            changes: 1,
            bytes: 7
        }
    );
    tx.delete(b"key").unwrap();
    assert_eq!(
        tx.usage().unwrap(),
        Usage {
            changes: 1,
            bytes: 3
        }
    );
    assert_eq!(tx.read(b"key").unwrap(), None);
    tx.put(b"key", b"").unwrap();
    assert_eq!(tx.read(b"key").unwrap(), Some(Vec::new()));
    tx.commit().unwrap();
    assert_eq!(store.read(b"key").unwrap(), Some(Vec::new()));
}

#[test]
fn each_capacity_failure_leaves_the_previous_attempt_unchanged() {
    let small = Limits {
        max_changes: 1,
        max_key_bytes: 3,
        max_value_bytes: 4,
        max_staged_bytes: 5,
    };
    let store = Coordinator::new(Backend::default(), small).unwrap();
    let tx = store.begin(op(1), Revision(0)).unwrap();
    tx.put(b"a", b"one").unwrap();
    for (key, value, resource) in [
        (b"long".as_slice(), b"".as_slice(), Resource::KeyBytes),
        (b"a".as_slice(), b"large".as_slice(), Resource::ValueBytes),
        (b"bb".as_slice(), b"".as_slice(), Resource::StagedBytes),
        (b"b".as_slice(), b"".as_slice(), Resource::Changes),
    ] {
        assert!(
            matches!(tx.put(key, value), Err(StoreError::Capacity(CapacityError::Exceeded { resource: r, .. })) if r == resource)
        );
        assert_eq!(
            tx.usage().unwrap(),
            Usage {
                changes: 1,
                bytes: 4
            }
        );
        assert_eq!(tx.read(b"a").unwrap(), Some(b"one".to_vec()));
    }
    tx.abort().unwrap();
    assert_eq!(store.read(b"a").unwrap(), None);
}

#[test]
fn aborted_handles_cannot_mutate_a_new_attempt_even_with_same_operation_id() {
    let store = coordinator(Fault::None);
    let old = store.begin(op(1), Revision(0)).unwrap();
    old.put(b"a", b"discard").unwrap();
    old.abort().unwrap();
    let new = store.begin(op(1), Revision(0)).unwrap();
    assert!(matches!(
        old.put(b"a", b"stale"),
        Err(StoreError::StaleTransaction)
    ));
    assert!(matches!(old.abort(), Err(StoreError::StaleTransaction)));
    assert_eq!(new.read(b"a").unwrap(), None);
    new.put(b"a", b"fresh").unwrap();
    new.commit().unwrap();
    assert_eq!(store.read(b"a").unwrap(), Some(b"fresh".to_vec()));
}

#[test]
fn revision_fences_and_committed_ids_prevent_reapplication() {
    let store = coordinator(Fault::None);
    assert!(matches!(
        store.begin(op(1), Revision(1)),
        Err(StoreError::RevisionMismatch { .. })
    ));
    let tx = store.begin(op(1), Revision(0)).unwrap();
    assert!(matches!(
        store.clone().begin(op(2), Revision(0)),
        Err(StoreError::Busy)
    ));
    tx.put(b"counter", b"once").unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        store.begin(op(2), Revision(0)),
        Err(StoreError::RevisionMismatch { .. })
    ));
    assert!(matches!(
        store.begin(op(1), Revision(1)),
        Err(StoreError::AlreadyCommitted(_))
    ));
    let second = store.begin(op(2), Revision(1)).unwrap();
    assert_eq!(second.read(b"counter").unwrap(), Some(b"once".to_vec()));
    second.abort().unwrap();
}

#[test]
fn uncertain_commit_fences_every_access_until_matching_recovery() {
    let store = coordinator(Fault::UncertainAfter);
    let tx = store.begin(op(1), Revision(0)).unwrap();
    tx.put(b"outbox", b"immutable cipher").unwrap();
    assert_eq!(tx.commit().unwrap(), CommitOutcome::Uncertain);
    assert!(matches!(
        tx.read(b"outbox"),
        Err(StoreError::RecoveryRequired(_))
    ));
    assert!(matches!(tx.abort(), Err(StoreError::RecoveryRequired(_))));
    assert!(matches!(
        store.read(b"outbox"),
        Err(StoreError::RecoveryRequired(_))
    ));
    assert!(matches!(store.head(), Err(StoreError::RecoveryRequired(_))));
    assert!(matches!(
        store.begin(op(2), Revision(1)),
        Err(StoreError::RecoveryRequired(_))
    ));
    assert!(matches!(
        store.recover(op(2)),
        Err(StoreError::WrongRecoveryOperation)
    ));
    let receipt = CommitReceipt {
        operation_id: op(1),
        revision: Revision(1),
    };
    assert_eq!(
        store.recover(op(1)).unwrap(),
        RecoveryOutcome::Committed(receipt)
    );
    assert_eq!(
        store.read(b"outbox").unwrap(),
        Some(b"immutable cipher".to_vec())
    );
    assert!(matches!(tx.commit(), Err(StoreError::StaleTransaction)));
}

#[test]
fn errors_and_malformed_receipts_are_uncertain_not_abort_success() {
    for fault in [Fault::ErrorAfter, Fault::WrongReceipt] {
        let store = coordinator(fault);
        let tx = store.begin(op(1), Revision(0)).unwrap();
        tx.put(b"a", b"durable").unwrap();
        assert!(tx.commit().is_err());
        assert!(matches!(
            store.read(b"a"),
            Err(StoreError::RecoveryRequired(_))
        ));
        assert!(matches!(
            store.recover(op(1)).unwrap(),
            RecoveryOutcome::Committed(_)
        ));
        assert_eq!(store.read(b"a").unwrap(), Some(b"durable".to_vec()));
    }
}

#[test]
fn definite_non_commit_releases_fence_and_uncertain_non_commit_recovers() {
    for fault in [Fault::NotCommitted, Fault::UncertainBefore] {
        let store = coordinator(fault);
        let tx = store.begin(op(1), Revision(0)).unwrap();
        tx.put(b"a", b"never durable").unwrap();
        let outcome = tx.commit().unwrap();
        if outcome == CommitOutcome::Uncertain {
            assert_eq!(
                store.recover(op(1)).unwrap(),
                RecoveryOutcome::NotCommitted {
                    revision: Revision(0)
                }
            );
        } else {
            assert_eq!(
                outcome,
                CommitOutcome::NotCommitted {
                    revision: Revision(0)
                }
            );
        }
        assert_eq!(store.read(b"a").unwrap(), None);
        assert!(matches!(
            tx.put(b"a", b"stale"),
            Err(StoreError::StaleTransaction)
        ));
        store.begin(op(2), Revision(0)).unwrap().abort().unwrap();
    }
}

#[test]
fn read_capacity_is_passed_to_backend_and_debug_omits_secret_material() {
    let mut backend = Backend::default();
    backend.values.insert(b"big".to_vec(), vec![0; 65]);
    let store = Coordinator::new(backend, limits()).unwrap();
    assert!(matches!(
        store.read(b"big"),
        Err(StoreError::Backend("value limit"))
    ));
    let change = Mutation::Put {
        key: b"sensitive key".to_vec(),
        value: b"SECRET".to_vec(),
    };
    let debug = format!("{change:?}");
    assert!(!debug.contains("SECRET"));
    assert!(!debug.contains("sensitive"));
}

#[test]
fn revision_and_generation_exhaustion_fail_without_starting_work() {
    let store = Coordinator::new(
        Backend {
            revision: Revision(u64::MAX),
            ..Backend::default()
        },
        limits(),
    )
    .unwrap();
    assert!(matches!(
        store.begin(op(1), Revision(u64::MAX)),
        Err(StoreError::CounterExhausted)
    ));
    let store = coordinator(Fault::None);
    store.state.lock().generation = u64::MAX;
    assert!(matches!(
        store.begin(op(1), Revision(0)),
        Err(StoreError::CounterExhausted)
    ));
    assert!(matches!(store.state.lock().phase, Phase::Idle));
}

fn ticket(sequence: u64) -> OperationTicket {
    OperationTicket::new(StoreIncarnation::new([7; 24]).unwrap(), sequence).unwrap()
}

#[test]
fn input_rejections_do_not_block_a_subsequent_valid_operation() {
    for reason in [
        OperationRejection::InvalidTicket,
        OperationRejection::ForeignIncarnation,
        OperationRejection::OutOfOrder {
            next: Some(ticket(1)),
        },
        OperationRejection::RecoveryWindowFull,
        OperationRejection::ReservedCapacity,
        OperationRejection::StorageCapacity,
        OperationRejection::RetiredTicket,
    ] {
        let store = coordinator(Fault::None);
        store.state.lock().backend.recovery_override = Some(RecoveryOutcome::Rejected(reason));
        assert!(matches!(
            store.begin(op(0), Revision(0)),
            Err(StoreError::OperationRejected(actual)) if actual == reason
        ));
        assert_eq!(
            store.recover(op(0)).unwrap(),
            RecoveryOutcome::Rejected(reason)
        );
        assert_eq!(store.head().unwrap(), Revision(0));
        store.state.lock().backend.recovery_override = None;
        store
            .begin(ticket(1).into(), Revision(0))
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(store.head().unwrap(), Revision(1));
    }
}

#[test]
fn rejected_commit_is_definite_and_revokes_the_staged_attempt() {
    let store = coordinator(Fault::None);
    let tx = store.begin(ticket(1).into(), Revision(0)).unwrap();
    tx.put(b"value", b"not installed").unwrap();
    store.state.lock().backend.commit_rejection = Some(OperationRejection::RecoveryWindowFull);
    assert_eq!(
        tx.commit().unwrap(),
        CommitOutcome::Rejected(OperationRejection::RecoveryWindowFull)
    );
    assert_eq!(store.read(b"value").unwrap(), None);
    assert!(matches!(tx.commit(), Err(StoreError::StaleTransaction)));
    store.state.lock().backend.commit_rejection = None;
    store
        .begin(ticket(1).into(), Revision(0))
        .unwrap()
        .commit()
        .unwrap();
}

#[test]
fn retired_ticket_cannot_be_executed_and_floor_proof_is_checked() {
    let retired = RecoveryOutcome::Retired {
        revision: Revision(3),
        retired_through: ticket(2),
    };
    let store = Coordinator::new(
        Backend {
            revision: Revision(3),
            recovery_override: Some(retired),
            ..Backend::default()
        },
        limits(),
    )
    .unwrap();
    assert_eq!(store.recover(ticket(1).into()).unwrap(), retired);
    assert!(matches!(
        store.begin(ticket(2).into(), Revision(3)),
        Err(StoreError::OutsideRecoveryWindow { retired_through }) if retired_through == ticket(2)
    ));
    assert_eq!(store.head().unwrap(), Revision(3));
    for operation in [
        ticket(3).into(),
        OperationId([0; 32]),
        StoreIncarnation::new([8; 24]).unwrap().first().into(),
    ] {
        let store = Coordinator::new(
            Backend {
                revision: Revision(3),
                recovery_override: Some(retired),
                ..Backend::default()
            },
            limits(),
        )
        .unwrap();
        assert!(matches!(
            store.recover(operation),
            Err(StoreError::BackendContractViolation)
        ));
        assert!(matches!(
            store.read(b"secret"),
            Err(StoreError::RecoveryRequired(_))
        ));
    }
}

#[test]
fn rejection_or_retirement_cannot_clear_a_known_uncertain_commit() {
    for recovery in [
        RecoveryOutcome::Rejected(OperationRejection::ForeignIncarnation),
        RecoveryOutcome::Retired {
            revision: Revision(1),
            retired_through: ticket(1),
        },
    ] {
        let store = coordinator(Fault::UncertainAfter);
        let tx = store.begin(ticket(1).into(), Revision(0)).unwrap();
        tx.put(b"outbox", b"exact original bytes").unwrap();
        assert_eq!(tx.commit().unwrap(), CommitOutcome::Uncertain);
        store.state.lock().backend.recovery_override = Some(recovery);
        assert!(matches!(
            store.recover(ticket(1).into()),
            Err(StoreError::BackendContractViolation)
        ));
        assert!(matches!(
            store.maintain(MaintenanceMode::Complete),
            Err(StoreError::RecoveryRequired(_))
        ));
        assert!(matches!(
            store.recover_maintenance(),
            Err(StoreError::RecoveryRequired(_))
        ));
        assert!(matches!(
            store.read(b"outbox"),
            Err(StoreError::RecoveryRequired(_))
        ));
        store.state.lock().backend.recovery_override = None;
        assert!(matches!(
            store.recover(ticket(1).into()).unwrap(),
            RecoveryOutcome::Committed(_)
        ));
        assert_eq!(
            store.read(b"outbox").unwrap(),
            Some(b"exact original bytes".to_vec())
        );
    }
}

#[test]
fn unresolved_recovery_after_reopen_requires_evidence_even_without_an_expected_revision() {
    for preflight in [false, true] {
        let store = coordinator(Fault::None);
        store.state.lock().backend.unresolved = true;
        if preflight {
            assert!(matches!(
                store.begin(ticket(1).into(), Revision(0)),
                Err(StoreError::RecoveryRequired(_))
            ));
        } else {
            assert_eq!(
                store.recover(ticket(1).into()).unwrap(),
                RecoveryOutcome::Unresolved
            );
        }
        {
            let mut state = store.state.lock();
            state.backend.unresolved = false;
            state.backend.recovery_override =
                Some(RecoveryOutcome::Rejected(OperationRejection::InvalidTicket));
        }
        assert!(matches!(
            store.recover(ticket(1).into()),
            Err(StoreError::BackendContractViolation)
        ));
        assert!(matches!(store.head(), Err(StoreError::RecoveryRequired(_))));
        store.state.lock().backend.recovery_override = None;
        assert_eq!(
            store.recover(ticket(1).into()).unwrap(),
            RecoveryOutcome::NotCommitted {
                revision: Revision(0)
            }
        );
    }
}

#[test]
fn failed_input_lookup_can_resolve_to_rejection_without_poisoning_the_store() {
    let store = coordinator(Fault::None);
    store.state.lock().backend.recovery_error = true;
    assert!(matches!(
        store.recover(op(0)),
        Err(StoreError::Backend("recovery I/O"))
    ));
    {
        let mut state = store.state.lock();
        state.backend.recovery_error = false;
        state.backend.recovery_override =
            Some(RecoveryOutcome::Rejected(OperationRejection::InvalidTicket));
    }
    assert_eq!(
        store.recover(op(0)).unwrap(),
        RecoveryOutcome::Rejected(OperationRejection::InvalidTicket)
    );
    assert_eq!(store.head().unwrap(), Revision(0));
}

#[test]
fn maintenance_is_exclusive_and_uncertainty_fences_every_access() {
    let store = coordinator(Fault::None);
    let tx = store.begin(ticket(1).into(), Revision(0)).unwrap();
    assert!(matches!(
        store.maintain(MaintenanceMode::Complete),
        Err(StoreError::Busy)
    ));
    assert!(matches!(store.recover_maintenance(), Err(StoreError::Busy)));
    assert_eq!(store.state.lock().backend.maintenance_calls, 0);
    tx.abort().unwrap();
    assert_eq!(
        store.maintain(MaintenanceMode::Uncertain).unwrap(),
        MaintenanceOutcome::Uncertain
    );
    assert!(matches!(store.head(), Err(StoreError::MaintenanceRequired)));
    assert!(matches!(
        store.read(b"secret"),
        Err(StoreError::MaintenanceRequired)
    ));
    assert!(matches!(
        store.begin(ticket(1).into(), Revision(0)),
        Err(StoreError::MaintenanceRequired)
    ));
    assert!(matches!(
        store.recover(ticket(1).into()),
        Err(StoreError::MaintenanceRequired)
    ));
    assert!(matches!(
        tx.put(b"secret", b"stale"),
        Err(StoreError::MaintenanceRequired)
    ));
    assert!(matches!(
        store.maintain(MaintenanceMode::Complete),
        Err(StoreError::MaintenanceRequired)
    ));
    store.state.lock().backend.maintenance = MaintenanceMode::Rejected;
    assert!(matches!(
        store.recover_maintenance(),
        Err(StoreError::BackendContractViolation)
    ));
    assert!(matches!(store.head(), Err(StoreError::MaintenanceRequired)));
    store.state.lock().backend.maintenance = MaintenanceMode::Error;
    assert!(matches!(
        store.recover_maintenance(),
        Err(StoreError::Backend("maintenance I/O"))
    ));
    assert!(matches!(store.head(), Err(StoreError::MaintenanceRequired)));
    store.state.lock().backend.maintenance = MaintenanceMode::Complete;
    assert_eq!(
        store.recover_maintenance().unwrap(),
        MaintenanceOutcome::Complete(4)
    );
    assert_eq!(store.head().unwrap(), Revision(0));
    assert!(matches!(
        tx.put(b"secret", b"stale"),
        Err(StoreError::StaleTransaction)
    ));
    store
        .begin(ticket(1).into(), Revision(0))
        .unwrap()
        .abort()
        .unwrap();
}

#[test]
fn definite_maintenance_steps_preserve_revision_and_release_the_fence() {
    for mode in [
        MaintenanceMode::Complete,
        MaintenanceMode::Pending,
        MaintenanceMode::Rejected,
    ] {
        let store = coordinator(Fault::None);
        store
            .begin(ticket(1).into(), Revision(0))
            .unwrap()
            .commit()
            .unwrap();
        let outcome = store.maintain(mode).unwrap();
        assert!(matches!(
            outcome,
            MaintenanceOutcome::Complete(1)
                | MaintenanceOutcome::Pending(1)
                | MaintenanceOutcome::Rejected(1)
        ));
        assert_eq!(store.head().unwrap(), Revision(1));
        store
            .begin(ticket(2).into(), Revision(1))
            .unwrap()
            .abort()
            .unwrap();
    }
    for revision in [Revision(0), Revision(2)] {
        let store = coordinator(Fault::None);
        store
            .begin(ticket(1).into(), Revision(0))
            .unwrap()
            .commit()
            .unwrap();
        assert!(matches!(
            store.maintain(MaintenanceMode::MoveHead(revision)),
            Err(StoreError::BackendContractViolation)
        ));
        assert!(matches!(store.head(), Err(StoreError::MaintenanceRequired)));
    }
}
