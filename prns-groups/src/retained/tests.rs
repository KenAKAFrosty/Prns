#![allow(clippy::unwrap_used, clippy::panic, clippy::drop_non_drop)]
use super::*;
use crate::admission::{Access, Authorization, Settlement};
use crate::{
    enrollment::Enrollment, ExecutionContext, GroupManager, OriginalScope, PublicationIntent,
    Targets,
};
use prns_groups_core::{ClockInterval, ClockReading, Timestamp};
const NOW: u64 = 1_800_000_000;
fn identity() -> IdentityHash {
    IdentityHash::new([11; 16])
}
fn id(n: u8, now: u64) -> RequestId {
    crate::RequestToken::new(now, [n; 20]).unwrap().into()
}
fn context(now: u64) -> ExecutionContext {
    ExecutionContext {
        clock: ClockReading::Trusted(ClockInterval::new(Timestamp(now), Timestamp(now)).unwrap()),
    }
}
fn unknown() -> ExecutionContext {
    ExecutionContext {
        clock: ClockReading::Unknown,
    }
}
fn limits(count: u32) -> Limits {
    Limits {
        max_operations: count,
        max_bytes: 1024 * 1024,
        ..Default::default()
    }
}
fn create() -> Intent {
    Intent::Create {
        profile: Default::default(),
        title: "Durable group".into(),
    }
}
fn initialize(db: &Database, limits: Limits) {
    let mut tx = db.begin_write().unwrap();
    tx.set_durability(redb::Durability::Immediate).unwrap();
    Enrollment::initialize_in(&tx, identity()).unwrap();
    initialize_in(&tx, identity(), limits).unwrap();
    tx.commit().unwrap();
}
#[derive(Default)]
struct Gate {
    paused: bool,
    acquired: Vec<Access>,
    settled: Vec<(Access, Settlement)>,
    live: usize,
}
impl crate::admission::CarrierRecovery for Gate {}
impl Authorization for Gate {
    type Permit = Access;
    fn acquire(&mut self, access: Access) -> Result<Access, Error> {
        self.acquired.push(access);
        if self.paused && !matches!(access, Access::Recovery | Access::History) {
            return Err(Error::Busy);
        }
        self.live += 1;
        Ok(access)
    }
    fn settle(&mut self, permit: Access, settlement: Settlement) -> Result<(), Error> {
        self.live -= 1;
        self.settled.push((permit, settlement));
        Ok(())
    }
}

#[test]
fn readiness_rejects_partial_unknown_or_wrong_identity_without_initializing() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("schema")).unwrap();
    assert!(Store::open(&db, identity()).unwrap().is_none());
    let tx = db.begin_write().unwrap();
    drop(tx.open_table(OPERATIONS).unwrap());
    tx.commit().unwrap();
    assert!(matches!(Store::open(&db, identity()), Err(Error::Corrupt)));
    let tx = db.begin_write().unwrap();
    assert!(matches!(
        initialize_in(&tx, identity(), limits(8)),
        Err(Error::Configuration)
    ));
    drop(tx);
    let db = Database::create(dir.path().join("version")).unwrap();
    initialize(&db, limits(8));
    assert!(matches!(
        Store::open(&db, IdentityHash::new([12; 16])),
        Err(Error::Configuration)
    ));
    let mut header = Store::open(&db, identity())
        .unwrap()
        .unwrap()
        .snapshot()
        .unwrap()
        .header;
    header.version = VERSION + 1;
    let bytes = encode(&header).unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, bytes.as_slice())
        .unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        GroupManager::open_managed(&db, identity()),
        Err(Error::Configuration)
    ));
    assert_eq!(
        db.begin_read()
            .unwrap()
            .open_table(HEADER)
            .unwrap()
            .get(0)
            .unwrap()
            .unwrap()
            .value(),
        bytes
    );
}

#[test]
fn accepted_and_dispatched_without_effect_need_fresh_authority_after_reopen() {
    for cut in [fault::Cut::Accepted, fault::Cut::Dispatched] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("group");
        let db = Database::create(&path).unwrap();
        initialize(&db, limits(8));
        let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
        let mut gate = Gate::default();
        fault::set(cut);
        assert!(matches!(
            manager.submit_retained(id(1, NOW), create(), context(NOW), &mut gate),
            Err(Error::Unavailable)
        ));
        assert_eq!(gate.live, 0);
        assert_eq!(
            gate.settled.last(),
            Some(&(
                Access::ServiceMutation {
                    request: id(1, NOW)
                },
                Settlement::Retained
            ))
        );
        let row = manager.retained_operation(id(1, NOW)).unwrap().unwrap();
        assert!(matches!(row.state, State::Pending | State::Dispatched));
        assert!(manager
            .recover_request(row.request.clone())
            .unwrap()
            .is_none());
        assert!(matches!(
            manager.execute_request(row.request, context(NOW)),
            Err(Error::Busy)
        ));
        drop(manager);
        drop(db);
        let db = Database::open(&path).unwrap();
        let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
        let mut gate = Gate {
            paused: true,
            ..Gate::default()
        };
        assert!(matches!(
            manager.submit_retained(id(1, NOW), create(), context(NOW), &mut gate),
            Err(Error::Busy)
        ));
        assert_eq!(gate.live, 0);
        assert!(manager
            .recover_request(
                manager
                    .retained_operation(id(1, NOW))
                    .unwrap()
                    .unwrap()
                    .request
            )
            .unwrap()
            .is_none());
        gate.paused = false;
        let result = manager
            .submit_retained(id(1, NOW), create(), context(NOW), &mut gate)
            .unwrap();
        assert!(matches!(
            result.state,
            State::Completed(Outcome::Created(_))
        ));
        assert_eq!(gate.live, 0);
    }
}

#[test]
fn domain_commit_or_saved_outcome_survives_lost_reply_and_recovers_while_paused() {
    for cut in [fault::Cut::DomainFinished, fault::Cut::OutcomeSaved] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("group");
        let db = Database::create(&path).unwrap();
        initialize(&db, limits(8));
        let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
        let mut gate = Gate::default();
        fault::set(cut);
        assert!(matches!(
            manager.submit_retained(id(1, NOW), create(), context(NOW), &mut gate),
            Err(Error::Unavailable)
        ));
        let request = manager
            .retained_operation(id(1, NOW))
            .unwrap()
            .unwrap()
            .request;
        let expected = manager.recover_request(request.clone()).unwrap().unwrap();
        assert_eq!(gate.live, 0);
        drop(manager);
        drop(db);
        let db = Database::open(&path).unwrap();
        let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
        let mut gate = Gate {
            paused: true,
            ..Gate::default()
        };
        let result = manager
            .submit_retained(id(1, NOW), create(), unknown(), &mut gate)
            .unwrap();
        assert_eq!(result.state, State::Completed(expected));
        assert!(!gate.acquired.is_empty());
        assert!(gate
            .acquired
            .iter()
            .all(|access| *access == Access::Recovery));
        assert_eq!(gate.live, 0);
        assert_eq!(
            manager.execute_request(request, unknown()).unwrap(),
            expected
        );
    }
}

#[test]
fn full_input_conflict_and_definitive_rejection_are_retained() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("group")).unwrap();
    initialize(&db, limits(8));
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    let created = manager
        .submit_retained(id(1, NOW), create(), context(NOW), &mut gate)
        .unwrap();
    let State::Completed(Outcome::Created(group)) = created.state else {
        panic!("create missing")
    };
    assert!(matches!(
        manager.submit_retained(
            id(1, NOW),
            Intent::Create {
                profile: Default::default(),
                title: "different".into()
            },
            context(NOW),
            &mut gate
        ),
        Err(Error::IdempotencyConflict)
    ));
    let rejected = manager
        .submit_retained(
            id(2, NOW),
            Intent::Invite {
                group,
                recipient: identity(),
            },
            context(NOW),
            &mut gate,
        )
        .unwrap();
    assert_eq!(
        rejected.state,
        State::Rejected(RejectionReason::InvalidInput)
    );
    gate.paused = true;
    assert_eq!(
        manager
            .submit_retained(
                id(2, NOW),
                rejected.request.intent.clone(),
                unknown(),
                &mut gate
            )
            .unwrap(),
        rejected
    );
    assert!(matches!(
        manager.execute_request(rejected.request, unknown()),
        Err(Error::Api(prns_groups_core::ApiError::Rejected {
            reason: RejectionReason::InvalidInput
        }))
    ));
    assert_eq!(gate.live, 0);
}

#[test]
fn unresolved_scope_blocks_other_mutations_and_cannot_be_evicted_or_dismissed() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("group")).unwrap();
    initialize(&db, limits(2));
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    let State::Completed(Outcome::Created(group)) = manager
        .submit_retained(id(1, NOW), create(), context(NOW), &mut gate)
        .unwrap()
        .state
    else {
        panic!("create")
    };
    fault::set(fault::Cut::Accepted);
    assert!(manager
        .submit_retained(
            id(2, NOW + 1),
            Intent::Close { group },
            context(NOW + 1),
            &mut gate
        )
        .is_err());
    let pending = manager.retained_operation(id(2, NOW + 1)).unwrap().unwrap();
    let direct = Request {
        id: id(3, NOW + 1),
        original_scope: pending.request.original_scope,
        intent: Intent::Invite {
            group,
            recipient: IdentityHash::new([12; 16]),
        },
    };
    assert!(matches!(
        manager.execute_request(direct, context(NOW + 1)),
        Err(Error::Busy)
    ));
    assert!(matches!(
        manager.dismiss_retained(id(2, NOW + 1), &mut gate),
        Err(Error::Busy)
    ));
    // A different group's creation may evict only the completed Create result.
    let other = manager
        .submit_retained(id(4, NOW + 2), create(), context(NOW + 2), &mut gate)
        .unwrap();
    assert!(other.state.is_terminal());
    assert!(manager.retained_operation(id(1, NOW)).unwrap().is_none());
    assert_eq!(
        manager.retained_operation(id(2, NOW + 1)).unwrap().unwrap(),
        pending
    );
    assert_eq!(gate.live, 0);
}

#[test]
fn dismissing_a_result_retires_same_second_ids_without_capacity_pressure() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("group")).unwrap();
    initialize(&db, limits(256));
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    let first = manager
        .submit_retained(id(1, NOW), create(), context(NOW), &mut gate)
        .unwrap();
    assert!(first.state.is_terminal());
    manager
        .dismiss_retained(first.request.id, &mut gate)
        .unwrap();

    // A fresh random suffix cannot bypass the inclusive retirement floor.
    for now in [NOW, NOW + 1] {
        assert!(matches!(
            manager.submit_retained(id(2, NOW), create(), context(now), &mut gate),
            Err(Error::Retry(prns_groups_core::retry::RetryError::Expired))
        ));
        assert!(manager.retained_operation(id(2, NOW)).unwrap().is_none());
    }
    let next = manager
        .submit_retained(id(3, NOW + 1), create(), context(NOW + 1), &mut gate)
        .unwrap();
    assert!(next.state.is_terminal());
    assert_eq!(gate.live, 0);
}

#[test]
fn atomic_retirement_floor_rejects_forgotten_ids_after_clock_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("group");
    let db = Database::create(&path).unwrap();
    initialize(&db, limits(1));
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    manager
        .submit_retained(id(1, NOW), create(), context(NOW), &mut gate)
        .unwrap();
    // Pruning has second precision: it cannot accept another token in the same
    // second while forgetting the prior result under an inclusive floor.
    assert!(matches!(
        manager.submit_retained(id(2, NOW), create(), context(NOW), &mut gate),
        Err(Error::Retry(prns_groups_core::retry::RetryError::Expired))
    ));
    assert!(manager.retained_operation(id(1, NOW)).unwrap().is_some());
    manager
        .submit_retained(id(2, NOW + 1), create(), context(NOW + 1), &mut gate)
        .unwrap();
    manager.dismiss_retained(id(2, NOW + 1), &mut gate).unwrap();
    drop(manager);
    drop(db);
    let db = Database::open(path).unwrap();
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    for request in [id(1, NOW), id(2, NOW + 1)] {
        assert!(matches!(
            manager.submit_retained(request, create(), context(NOW), &mut gate),
            Err(Error::Retry(prns_groups_core::retry::RetryError::Expired))
        ));
    }
    assert!(manager
        .retained_operations(
            None,
            crate::PageLimits {
                max_rows: 1,
                max_bytes: 1024 * 1024
            }
        )
        .unwrap()
        .is_empty());
    assert_eq!(gate.live, 0);
}

#[test]
fn bounded_capacity_never_forgets_unresolved_and_invalid_requests_never_accept() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("group")).unwrap();
    initialize(&db, limits(1));
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    for title in [" ".into(), "x".repeat(129)] {
        assert!(matches!(
            manager.submit_retained(
                id(1, NOW),
                Intent::Create {
                    title,
                    profile: Default::default()
                },
                context(NOW),
                &mut gate
            ),
            Err(Error::InvalidInput)
        ));
        assert!(manager.retained_operation(id(1, NOW)).unwrap().is_none());
    }
    fault::set(fault::Cut::Accepted);
    assert!(manager
        .submit_retained(id(1, NOW), create(), context(NOW), &mut gate)
        .is_err());
    assert!(matches!(
        manager.submit_retained(id(2, NOW + 1), create(), context(NOW + 1), &mut gate),
        Err(Error::Busy)
    ));
    let store = Store::open(&db, identity()).unwrap().unwrap();
    let mut pending = store.operation(id(1, NOW)).unwrap().unwrap();
    pending.request.id = id(2, NOW + 1);
    pending.scope = OperationScope::Group(GroupId([9; 32]));
    pending.request.intent = Intent::DeleteLocal {
        group: GroupId([9; 32]),
    };
    pending.request.original_scope = Some(OriginalScope::Unjoined(
        prns_groups_core::StoreIncarnation::new([7; 24]).unwrap(),
    ));
    assert!(matches!(
        store.accept(&pending, context(NOW + 1)),
        Err(Error::StorageFull)
    ));
    assert_eq!(
        store
            .list(
                None,
                crate::PageLimits {
                    max_rows: 1,
                    max_bytes: 1024 * 1024
                }
            )
            .unwrap()
            .len(),
        1
    );
    assert_eq!(gate.live, 0);
}

#[test]
fn canonical_codec_preserves_all_intents_and_rejects_invalid_incarnations() {
    let group = GroupId([9; 32]);
    let invitation = crate::InvitationId([8; 32]);
    let endpoint = prns_groups_core::Endpoint {
        id: prns_groups_core::EndpointId([7; 32]),
        incarnation: prns_groups_core::EndpointIncarnation::new(1).unwrap(),
    };
    let control = prns_groups_core::ControlReference {
        head: prns_groups_core::ControlId([6; 32]),
        era: prns_groups_core::AuthorizationEra(prns_groups_core::ControlId([5; 32])),
    };
    let intents = [
        create(),
        Intent::Publish(PublicationIntent {
            group,
            namespace: prns_groups_core::NamespaceId(23),
            schema: prns_groups_core::SchemaId(1),
            targets: Targets::Explicit(vec![endpoint]),
            body: vec![0, 255, 17],
        }),
        Intent::Invite {
            group,
            recipient: identity(),
        },
        Intent::Respond {
            invitation,
            accept: true,
        },
        Intent::CancelInvitation { invitation },
        Intent::DismissInvitation { invitation },
        Intent::RemoveMember {
            group,
            member: endpoint,
        },
        Intent::Leave { group },
        Intent::Close { group },
        Intent::DeleteLocal { group },
    ];
    for intent in intents {
        let request = Request {
            id: id(1, NOW),
            original_scope: intent.group().map(|_| OriginalScope::Control {
                control,
                incarnation: prns_groups_store::StoreIncarnation::new([1; 24]).unwrap(),
            }),
            intent,
        };
        request.validate().unwrap();
        assert_eq!(
            decode::<Request>(&encode(&request).unwrap()).unwrap(),
            request
        );
    }
    assert!(serde_json::from_str::<prns_groups_core::EndpointIncarnation>("0").is_err());
    assert!(decode::<prns_groups_core::StoreIncarnation>(&encode(&[0u8; 24]).unwrap()).is_err());
}

#[test]
fn publication_exact_retry_uses_original_record_and_session_cannot_bypass_retainer() {
    let identities = [identity(), IdentityHash::new([12; 16])];
    let fixtures =
        crate::fixture::generate("Generic retained publication", &identities, Timestamp(NOW))
            .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("group")).unwrap();
    let cfg = crate::fixture::load_and_install(&db, &fixtures[0], identity()).unwrap();
    let mut tx = db.begin_write().unwrap();
    tx.set_durability(redb::Durability::Immediate).unwrap();
    initialize_in(&tx, identity(), limits(8)).unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        crate::StoreSession::open(&db, cfg.clone()),
        Err(Error::Configuration)
    ));
    let mut manager = GroupManager::open_fixed(&db, cfg.clone()).unwrap();
    let mut gate = Gate::default();
    let intent = Intent::Publish(PublicationIntent {
        group: cfg.group,
        namespace: prns_groups_core::NamespaceId(23),
        schema: prns_groups_core::SchemaId(1),
        targets: Targets::OtherMembers,
        body: vec![0, 255, 17],
    });
    fault::set(fault::Cut::DomainFinished);
    assert!(matches!(
        manager.submit_retained(id(1, NOW), intent.clone(), context(NOW), &mut gate),
        Err(Error::Unavailable)
    ));
    let request = manager
        .retained_operation(id(1, NOW))
        .unwrap()
        .unwrap()
        .request;
    let expected = manager.recover_request(request.clone()).unwrap().unwrap();
    assert!(matches!(expected, Outcome::Published(_)));
    drop(manager);
    let mut manager = GroupManager::open_fixed(&db, cfg).unwrap();
    gate.paused = true;
    assert_eq!(
        manager
            .submit_retained(id(1, NOW), intent, unknown(), &mut gate)
            .unwrap()
            .state,
        State::Completed(expected)
    );
    let mut changed = request;
    changed.original_scope = Some(OriginalScope::Control {
        control: prns_groups_core::ControlReference {
            head: prns_groups_core::ControlId([55; 32]),
            era: prns_groups_core::AuthorizationEra(prns_groups_core::ControlId([55; 32])),
        },
        incarnation: changed.original_scope.unwrap().incarnation(),
    });
    assert!(matches!(
        manager.execute_request(changed, unknown()),
        Err(Error::IdempotencyConflict)
    ));
    assert_eq!(gate.live, 0);
}

#[test]
fn expired_unresolved_attempt_remains_uncertain_and_cannot_be_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("group")).unwrap();
    initialize(&db, limits(8));
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    fault::set(fault::Cut::Dispatched);
    assert!(manager
        .submit_retained(id(1, NOW), create(), context(NOW), &mut gate)
        .is_err());
    let operation = manager
        .submit_retained(id(1, NOW), create(), context(NOW + 15 * 86400), &mut gate)
        .unwrap();
    assert_eq!(
        operation.state,
        State::Uncertain(prns_groups_core::ApiError::Retry {
            reason: prns_groups_core::retry::RetryError::Expired
        })
    );
    assert!(matches!(
        manager.dismiss_retained(id(1, NOW), &mut gate),
        Err(Error::Busy)
    ));
    assert!(matches!(
        manager.submit_retained(
            id(2, NOW + 15 * 86400),
            create(),
            context(NOW + 15 * 86400),
            &mut gate
        ),
        Err(Error::Busy)
    ));
    assert_eq!(gate.live, 0);
    assert!(manager
        .recover_request(operation.request)
        .unwrap()
        .is_none());
}

#[test]
fn failed_receipt_recovery_does_not_authorize_another_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("group")).unwrap();
    initialize(&db, limits(8));
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    fault::set(fault::Cut::Dispatched);
    assert!(manager
        .submit_retained(id(1, NOW), create(), context(NOW), &mut gate)
        .is_err());
    // Simulate unavailable/corrupt authoritative evidence after acceptance. A
    // failed receipt lookup must not be treated as proof of absence.
    let catalog: redb::TableDefinition<u8, &[u8]> =
        redb::TableDefinition::new("prns_group_enrollment_v1");
    let tx = db.begin_write().unwrap();
    tx.open_table(catalog)
        .unwrap()
        .insert(0, b"{}".as_slice())
        .unwrap();
    tx.commit().unwrap();
    let mut gate = Gate::default();
    let operation = manager
        .submit_retained(id(1, NOW), create(), context(NOW), &mut gate)
        .unwrap();
    assert!(matches!(operation.state, State::Uncertain(_)));
    assert!(!gate.acquired.is_empty());
    assert!(gate
        .acquired
        .iter()
        .all(|access| *access == Access::Recovery));
    assert_eq!(
        gate.settled.last(),
        Some(&(Access::Recovery, Settlement::Retained))
    );
    assert_eq!(gate.live, 0);
}

#[test]
fn transaction_validators_share_full_decoder_and_distinguish_complete_absence() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("inspection")).unwrap();
    let before = db.begin_read().unwrap();
    assert_eq!(inspect_in(&before, identity()).unwrap(), None);
    assert!(matches!(
        validate_in(&before, identity()),
        Err(Error::NotProvisioned)
    ));
    initialize(&db, limits(8));
    assert_eq!(inspect_in(&before, identity()).unwrap(), None);
    drop(before);
    assert_eq!(
        validate_in(&db.begin_read().unwrap(), identity()).unwrap(),
        limits(8)
    );
    let tx = db.begin_write().unwrap();
    tx.open_table(OPERATIONS)
        .unwrap()
        .insert(b"bad".as_slice(), b"not an operation".as_slice())
        .unwrap();
    tx.commit().unwrap();
    assert!(validate_in(&db.begin_read().unwrap(), identity()).is_err());
    assert!(Store::open(&db, identity()).is_err());
    let partial = Database::create(dir.path().join("partial")).unwrap();
    let tx = partial.begin_write().unwrap();
    drop(tx.open_table(OPERATIONS).unwrap());
    tx.commit().unwrap();
    assert!(matches!(
        inspect_in(&partial.begin_read().unwrap(), identity()),
        Err(Error::Corrupt)
    ));
}
