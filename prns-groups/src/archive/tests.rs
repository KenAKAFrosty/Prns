#![allow(clippy::unwrap_used, clippy::panic)]
use super::{schema::*, Participant};
use crate::{fixture, Provisioned};
use prns_groups_core::{ClockInterval, ClockReading, NamespaceId, SchemaId, Seconds, Timestamp};
use prns_groups_mls::Adapter;
use prns_groups_service::{Descriptor, GroupService, Incoming, Publication, PublicationValidity};
use prns_groups_store::{
    AtomicBackend, Coordinator, Mutation, OperationRejection, OperationTicket, RecoveryOutcome,
};
use prns_groups_store_redb::{
    CommitContext, RedbBackend, RedbError, RedbStoreOwner, TransactionParticipant,
};
use prns_host::IdentityHash;
use redb::{Database, ReadableDatabase, WriteTransaction};
use std::sync::Arc;
const NOW: u64 = 1_800_000_000;
type Service<'a> = GroupService<'a, RedbBackend<'a>>;
fn clock(now: u64) -> ClockReading {
    ClockReading::Trusted(ClockInterval::new(Timestamp(now), Timestamp(now)).unwrap())
}
fn limits(count: u64) -> Limits {
    Limits {
        max_records: count,
        max_bytes: 1_048_576,
    }
}
fn pair() -> (tempfile::TempDir, Vec<(Database, Provisioned)>) {
    let dir = tempfile::tempdir().unwrap();
    let ids = [IdentityHash::new([41; 16]), IdentityHash::new([42; 16])];
    let fixtures = fixture::generate("Archive", &ids, Timestamp(NOW)).unwrap();
    let mut nodes = Vec::new();
    for i in 0..2 {
        let db = Database::create(dir.path().join(format!("node-{i}"))).unwrap();
        let cfg = fixture::load_and_install(&db, &fixtures[i], ids[i]).unwrap();
        let tx = db.begin_write().unwrap();
        initialize_in(&tx, ids[i], limits(if i == 0 { 8 } else { 1 })).unwrap();
        tx.commit().unwrap();
        nodes.push((db, cfg));
    }
    (dir, nodes)
}
fn service<T>(
    db: &Database,
    cfg: &Provisioned,
    participant: Arc<dyn TransactionParticipant>,
    f: impl FnOnce(&mut Service<'_>) -> T,
) -> T {
    let mut owner = RedbStoreOwner::new(db);
    let backend = owner
        .open(cfg.namespace, cfg.incarnation, cfg.store_limits)
        .unwrap()
        .with_participant(participant);
    let coordinator =
        Coordinator::new(backend, cfg.store_limits.transaction_limits().unwrap()).unwrap();
    let adapter = Adapter::new(
        coordinator,
        cfg.identity.clone(),
        cfg.policy.clone(),
        cfg.mls_limits,
    )
    .unwrap();
    let construct = if cfg.owner_identity.is_some() {
        GroupService::new_managed
    } else {
        GroupService::new
    };
    let mut service = construct(
        adapter,
        Descriptor {
            group: cfg.group,
            control: cfg.control,
            readers: &cfg.readers,
        },
        cfg.service_limits,
    )
    .unwrap();
    f(&mut service)
}
fn next(service: &Service<'_>, cfg: &Provisioned) -> prns_groups_core::OperationId {
    OperationTicket::new(cfg.incarnation, service.coordinator().head().unwrap().0 + 1)
        .unwrap()
        .operation_id()
}
fn publish(db: &Database, cfg: &Provisioned, nonce: u8) -> (prns_groups_core::RecordId, Vec<u8>) {
    service(
        db,
        cfg,
        Arc::new(Participant::new(cfg).unwrap()),
        |service| {
            let operation = next(service, cfg);
            let targets = cfg
                .peers
                .iter()
                .map(|peer| peer.endpoint)
                .collect::<Vec<_>>();
            let record = service
                .publish(
                    operation,
                    &Publication {
                        nonce: [nonce; 32],
                        namespace: NamespaceId(0x42),
                        schema: SchemaId(7),
                        validity: PublicationValidity::new(
                            Timestamp(NOW),
                            Timestamp(NOW + 30),
                            Seconds(60),
                        )
                        .unwrap(),
                        targets: &targets,
                        body: b"opaque history\0",
                    },
                    clock(NOW),
                )
                .unwrap()
                .record;
            (
                record,
                service.outgoing(operation).unwrap().unwrap().to_vec(),
            )
        },
    )
}
fn archived(
    db: &Database,
    cfg: &Provisioned,
    id: prns_groups_core::RecordId,
) -> (Vec<u8>, Vec<u8>, u64) {
    let read = db.begin_read().unwrap();
    let table = read.open_table(RECORDS).unwrap();
    let key = record_key(
        CommitContext {
            namespace: cfg.namespace,
            incarnation: cfg.incarnation,
        },
        cfg.group,
        id,
    );
    let bytes = table.get(key.as_slice()).unwrap().unwrap();
    let row = parse_row(bytes.value()).unwrap();
    let record =
        prns_groups_service::projection::decode_record(row.record, cfg.service_limits).unwrap();
    assert_eq!(
        row.author,
        if row.authored {
            cfg.local_reticulum_identity
        } else {
            cfg.peers[0].identity
        }
    );
    (record.body.to_vec(), row.flags.to_vec(), row.position)
}
#[test]
fn actual_publish_receive_and_receipts_share_archive_commit_and_survive_expiry() {
    let (_dir, nodes) = pair();
    let (a, ac) = &nodes[0];
    let (b, bc) = &nodes[1];
    let (id, wire) = publish(a, ac, 1);
    service(b, bc, Arc::new(Participant::new(bc).unwrap()), |service| {
        assert_eq!(
            service
                .receive(next(service, bc), &wire, clock(NOW))
                .unwrap(),
            Incoming::Record(id)
        );
    });
    let before = inspect_in(&b.begin_read().unwrap(), bc.local_reticulum_identity)
        .unwrap()
        .unwrap();
    let receipt = service(b, bc, Arc::new(Participant::new(bc).unwrap()), |service| {
        let op = next(service, bc);
        service.emit_receipt(op, id).unwrap();
        service.outgoing(op).unwrap().unwrap().to_vec()
    });
    assert_eq!(
        inspect_in(&b.begin_read().unwrap(), bc.local_reticulum_identity)
            .unwrap()
            .unwrap(),
        before
    );
    service(a, ac, Arc::new(Participant::new(ac).unwrap()), |service| {
        assert!(matches!(
            service.receive(next(service, ac), &receipt, clock(NOW)),
            Ok(Incoming::Receipt(_))
        ));
    });
    assert_eq!(archived(a, ac, id).1, vec![1]);
    let expected_targets = super::reads::delivery_targets(a, ac, id).unwrap().unwrap();
    assert_eq!(
        expected_targets[0].receipt(),
        prns_groups_core::ReceiptStatus::Received
    );
    assert_eq!(
        expected_targets[0].removal(),
        prns_groups_core::RemovalStatus::NotKnownRemoved
    );

    assert_eq!(archived(b, bc, id).1, vec![1]);
    let position = archived(a, ac, id).2;
    service(a, ac, Arc::new(Participant::new(ac).unwrap()), |service| {
        let plan = service.maintenance_plan(clock(NOW + 60), 4, 16).unwrap();
        service.maintain(next(service, ac), &plan).unwrap();
        assert!(service.record(id).unwrap().is_none());
    });
    assert_eq!(
        super::reads::delivery_targets(a, ac, id).unwrap(),
        Some(expected_targets)
    );
    assert_eq!(
        archived(a, ac, id),
        (b"opaque history\0".to_vec(), vec![1], position)
    );
    assert_eq!(
        inspect_in(&a.begin_read().unwrap(), ac.local_reticulum_identity)
            .unwrap()
            .unwrap()
            .records,
        1
    );
}
#[test]
fn full_archive_aborts_receive_without_consuming_ticket_and_preserves_receipt_capacity() {
    let (_dir, nodes) = pair();
    let (a, ac) = &nodes[0];
    let (b, bc) = &nodes[1];
    let (first, wire) = publish(a, ac, 1);
    service(b, bc, Arc::new(Participant::new(bc).unwrap()), |service| {
        service
            .receive(next(service, bc), &wire, clock(NOW))
            .unwrap();
    });
    let (_, second) = publish(a, ac, 2);
    service(b, bc, Arc::new(Participant::new(bc).unwrap()), |service| {
        let head = service.coordinator().head().unwrap();
        let op = next(service, bc);
        let error = crate::Error::from(service.receive(op, &second, clock(NOW)).unwrap_err());
        assert_eq!(
            error.rejection_reason(),
            Some(crate::RejectionReason::StorageCapacity)
        );
        assert!(crate::ApiError::from(&error).is_storage_full());
        assert_eq!(service.coordinator().head().unwrap(), head);
        assert!(matches!(
            service.coordinator().recover(op).unwrap(),
            RecoveryOutcome::NotCommitted { .. }
        ));
        service.emit_receipt(op, first).unwrap();
    });
    assert_eq!(
        inspect_in(&b.begin_read().unwrap(), bc.local_reticulum_identity)
            .unwrap()
            .unwrap()
            .records,
        1
    );
    assert_eq!(archived(b, bc, first).1, vec![1]);
}
struct FailAfterStage {
    inner: Participant,
    panic: bool,
}
impl TransactionParticipant for FailAfterStage {
    fn stage(
        &self,
        tx: &WriteTransaction,
        c: CommitContext,
        values: &prns_groups_store_redb::CommitRead<'_>,
        ticket: OperationTicket,
        m: &[Mutation],
    ) -> Result<Option<OperationRejection>, RedbError> {
        self.inner.stage(tx, c, values, ticket, m)?;
        if self.panic {
            panic!("after archive staging");
        }
        Err(RedbError::Corrupt)
    }
    fn delete_in(&self, tx: &WriteTransaction, c: CommitContext) -> Result<(), RedbError> {
        self.inner.delete_in(tx, c)
    }
}
#[test]
fn failure_or_unwind_after_archive_staging_aborts_both_states_and_retry_is_exact() {
    for panic in [false, true] {
        let (_dir, nodes) = pair();
        let (db, cfg) = &nodes[0];
        let head = {
            let mut owner = RedbStoreOwner::new(db);
            owner
                .open(cfg.namespace, cfg.incarnation, cfg.store_limits)
                .unwrap()
                .head()
                .unwrap()
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service(
                db,
                cfg,
                Arc::new(FailAfterStage {
                    inner: Participant::new(cfg).unwrap(),
                    panic,
                }),
                |service| {
                    service
                        .publish(
                            next(service, cfg),
                            &Publication {
                                nonce: [9; 32],
                                namespace: NamespaceId(3),
                                schema: SchemaId(4),
                                validity: PublicationValidity::new(
                                    Timestamp(NOW),
                                    Timestamp(NOW + 30),
                                    Seconds(60),
                                )
                                .unwrap(),
                                targets: &[cfg.peers[0].endpoint],
                                body: b"not yet committed",
                            },
                            clock(NOW),
                        )
                        .is_err()
                },
            )
        }));
        assert!(matches!(result, Err(_) | Ok(true)));
        assert_eq!(
            inspect_in(&db.begin_read().unwrap(), cfg.local_reticulum_identity)
                .unwrap()
                .unwrap()
                .records,
            0
        );
        let mut owner = RedbStoreOwner::new(db);
        assert_eq!(
            owner
                .open(cfg.namespace, cfg.incarnation, cfg.store_limits)
                .unwrap()
                .head()
                .unwrap(),
            head
        );
        drop(owner);
        let (id, _) = publish(db, cfg, 9);
        assert_eq!(archived(db, cfg, id).0, b"opaque history\0");
    }
}
#[test]
fn exact_namespace_deletion_releases_budget_and_stale_backend_cannot_reopen() {
    let (_dir, nodes) = pair();
    let (db, cfg) = &nodes[0];
    publish(db, cfg, 1);
    let participant = Participant::new(cfg).unwrap();
    let mut owner = RedbStoreOwner::new(db);
    owner
        .delete_with(
            cfg.namespace,
            cfg.incarnation,
            cfg.store_limits,
            Some(&participant),
        )
        .unwrap();
    assert_eq!(
        inspect_in(&db.begin_read().unwrap(), cfg.local_reticulum_identity)
            .unwrap()
            .unwrap()
            .records,
        0
    );
    assert!(matches!(
        owner.open(cfg.namespace, cfg.incarnation, cfg.store_limits),
        Err(RedbError::MissingNamespace)
    ));
    owner
        .delete_with(
            cfg.namespace,
            cfg.incarnation,
            cfg.store_limits,
            Some(&participant),
        )
        .unwrap();
}
#[test]
fn selection_snapshot_wrong_identity_partial_schema_and_corrupt_counters_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("schema")).unwrap();
    let id = IdentityHash::new([9; 16]);
    let old = db.begin_read().unwrap();
    assert!(inspect_in(&old, id).unwrap().is_none());
    let tx = db.begin_write().unwrap();
    initialize_in(&tx, id, limits(2)).unwrap();
    tx.commit().unwrap();
    assert!(inspect_in(&old, id).unwrap().is_none());
    drop(old);
    assert!(inspect_in(&db.begin_read().unwrap(), IdentityHash::new([8; 16])).is_err());
    let tx = db.begin_write().unwrap();
    assert!(initialize_in(&tx, id, limits(2)).is_err());
    drop(tx);
    let tx = db.begin_write().unwrap();
    save(
        &tx,
        id,
        Usage {
            limits: limits(2),
            records: 1,
            bytes: 900,
        },
    )
    .unwrap();
    tx.commit().unwrap();
    assert!(inspect_in(&db.begin_read().unwrap(), id).is_err());
    let other = Database::create(dir.path().join("partial")).unwrap();
    let tx = other.begin_write().unwrap();
    tx.open_table(RECORDS).unwrap();
    tx.commit().unwrap();
    assert!(inspect_in(&other.begin_read().unwrap(), id).is_err());
}

#[test]
fn one_identity_budget_is_shared_across_namespaces_and_reopen_keeps_immutable_history() {
    let (dir, mut nodes) = pair();
    let (db, cfg) = nodes.remove(0);
    let tx = db.begin_write().unwrap();
    save(
        &tx,
        cfg.local_reticulum_identity,
        Usage {
            limits: limits(1),
            records: 0,
            bytes: 0,
        },
    )
    .unwrap();
    tx.commit().unwrap();
    let (id, _) = publish(&db, &cfg, 1);
    let prior = archived(&db, &cfg, id);
    let ids = [cfg.local_reticulum_identity, IdentityHash::new([43; 16])];
    let fixtures = fixture::generate("Another namespace", &ids, Timestamp(NOW)).unwrap();
    let second = fixture::load_and_install(&db, &fixtures[0], ids[0]).unwrap();
    service(
        &db,
        &second,
        Arc::new(Participant::new(&second).unwrap()),
        |service| {
            let error = crate::Error::from(
                service
                    .publish(
                        next(service, &second),
                        &Publication {
                            nonce: [7; 32],
                            namespace: NamespaceId(3),
                            schema: SchemaId(4),
                            validity: PublicationValidity::new(
                                Timestamp(NOW),
                                Timestamp(NOW + 30),
                                Seconds(60),
                            )
                            .unwrap(),
                            targets: &[second.peers[0].endpoint],
                            body: b"other group",
                        },
                        clock(NOW),
                    )
                    .unwrap_err(),
            );
            assert_eq!(
                error.rejection_reason(),
                Some(crate::RejectionReason::StorageCapacity)
            );
        },
    );
    drop(db);
    let db = Database::open(dir.path().join("node-0")).unwrap();
    assert_eq!(archived(&db, &cfg, id), prior);
    let mut owner = RedbStoreOwner::new(&db);
    owner
        .delete_with(
            cfg.namespace,
            cfg.incarnation,
            cfg.store_limits,
            Some(&Participant::new(&cfg).unwrap()),
        )
        .unwrap();
    drop(owner);
    publish(&db, &second, 8);
    assert_eq!(
        inspect_in(&db.begin_read().unwrap(), ids[0])
            .unwrap()
            .unwrap()
            .records,
        1
    );
}
#[test]
fn managed_archive_uses_existing_retired_incarnation_fence() {
    use crate::{
        enrollment::{CatalogLimits, Enrollment},
        ExecutionContext, GroupManager, Intent, OriginalScope, Outcome, Request, RequestId,
    };
    use prns_groups_store::CommitOutcome;
    use sha2::{Digest, Sha256};

    fn snapshot(backend: &mut RedbBackend<'_>) -> (prns_groups_store_redb::RedbUsage, [u8; 32]) {
        let mut digest = Sha256::new();
        let usage = backend
            .inspect_with(|key, value| {
                digest.update(key.len().to_le_bytes());
                digest.update(key);
                digest.update(value.len().to_le_bytes());
                digest.update(value);
                Ok(())
            })
            .unwrap();
        (usage, digest.finalize().into())
    }

    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("managed")).unwrap();
    let identity = IdentityHash::new([52; 16]);
    let request = |n, intent, scope| Request {
        id: RequestId::from(crate::RequestToken::new(NOW, [n; 20]).unwrap()),
        intent,
        original_scope: scope,
    };
    let context = ExecutionContext { clock: clock(NOW) };
    let tx = db.begin_write().unwrap();
    Enrollment::initialize_with_limits_in(
        &tx,
        identity,
        CatalogLimits {
            max_maintenance_rows: 1,
            ..CatalogLimits::default()
        },
    )
    .unwrap();
    initialize_in(&tx, identity, limits(8)).unwrap();
    tx.commit().unwrap();
    let (first, second) = {
        let mut manager = GroupManager::open_managed(&db, identity).unwrap();
        let mut create = |id, title: &str| {
            let Outcome::Created(group) = manager
                .execute_request(
                    request(
                        id,
                        Intent::Create {
                            profile: Default::default(),
                            title: title.into(),
                        },
                        None,
                    ),
                    context,
                )
                .unwrap()
            else {
                panic!("created");
            };
            group
        };
        (
            create(1, "First managed archive"),
            create(2, "Second managed archive"),
        )
    };
    assert_ne!(first, second);
    let group = if first.0 > second.0 { first } else { second };
    let cfg = {
        let enrollment = Enrollment::open_existing(&db, identity).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        // Create #2's admission visited the first incarnation. Normalize the
        // cursor to the lower GroupId using real bounded maintenance only.
        // If first is higher, one tick wraps and the next visits the lower row.
        if first == group {
            enrollment.maintain(&mut owner, NOW).unwrap();
            enrollment.maintain(&mut owner, NOW).unwrap();
        }
        enrollment.provisioned(&mut owner, group).unwrap()
    };
    let participant = Arc::new(Participant::new(&cfg).unwrap());
    {
        let mut owner = RedbStoreOwner::new(&db);
        let mut backend = owner
            .open(cfg.namespace, cfg.incarnation, cfg.store_limits)
            .unwrap()
            .with_participant(participant.clone());
        let head = backend.head().unwrap();
        let ticket = OperationTicket::new(cfg.incarnation, head.0.checked_add(1).unwrap()).unwrap();
        assert!(
            matches!(backend.commit(head, ticket.operation_id(), &[]).unwrap(),
            CommitOutcome::Committed(receipt)
                if receipt.operation_id == ticket.operation_id() && receipt.revision.0 == head.0 + 1)
        );
        assert_eq!(backend.head().unwrap().0, head.0 + 1);
        assert!(matches!(
            backend.recover(ticket.operation_id()).unwrap(),
            RecoveryOutcome::Committed(_)
        ));
    }
    {
        let tx = db.begin_write().unwrap();
        assert!(participant.delete_in(&tx, participant.context).is_err());
    }
    {
        let mut manager = GroupManager::open_managed(&db, identity).unwrap();
        let scope = manager.group_scope(group).unwrap();
        manager
            .execute_request(request(3, Intent::Close { group }, Some(scope)), context)
            .unwrap();
        let scope = manager.group_scope(group).unwrap();
        assert!(matches!(scope, OriginalScope::Control { .. }));
        manager
            .execute_request(
                request(4, Intent::DeleteLocal { group }, Some(scope)),
                context,
            )
            .unwrap();
    }
    // Close admission visits the higher row; Delete admission wraps. Delete's
    // first physical purge visits only the earlier live group, leaving this
    // fenced namespace present for the actual participant rejection below.
    {
        let mut owner = RedbStoreOwner::new(&db);
        let mut backend = owner
            .open(cfg.namespace, cfg.incarnation, cfg.store_limits)
            .unwrap()
            .with_participant(participant.clone());
        let before = snapshot(&mut backend);
        let head = backend.head().unwrap();
        let ticket = OperationTicket::new(cfg.incarnation, head.0.checked_add(1).unwrap()).unwrap();
        let recovery = backend.recover(ticket.operation_id()).unwrap();
        assert_eq!(recovery, RecoveryOutcome::NotCommitted { revision: head });
        let archive_before = inspect_in(&db.begin_read().unwrap(), identity).unwrap();
        assert_eq!(
            backend.commit(head, ticket.operation_id(), &[]).unwrap(),
            CommitOutcome::Rejected(OperationRejection::ForeignIncarnation)
        );
        assert_eq!(backend.head().unwrap(), head);
        assert_eq!(backend.recover(ticket.operation_id()).unwrap(), recovery);
        assert_eq!(snapshot(&mut backend), before);
        assert_eq!(
            inspect_in(&db.begin_read().unwrap(), identity).unwrap(),
            archive_before
        );
    }
    {
        let tx = db.begin_write().unwrap();
        participant.delete_in(&tx, participant.context).unwrap();
    }
    let enrollment = Enrollment::open_existing(&db, identity).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    enrollment.maintain(&mut owner, NOW).unwrap();
    assert!(matches!(
        owner.open(cfg.namespace, cfg.incarnation, cfg.store_limits),
        Err(RedbError::MissingNamespace)
    ));
}

#[test]
fn canonical_session_history_survives_expiry_and_reopen_with_bounded_pages() {
    use crate::{
        client::{Command, Reply},
        ExecutionContext, Intent, OriginalScope, Outcome, PublicationIntent, Request, RequestId,
        StoreSession, Targets,
    };
    let (directory, mut nodes) = pair();
    let (database, cfg) = nodes.remove(0);
    let mut session = StoreSession::open(&database, cfg.clone()).unwrap();
    let mut ids = Vec::new();
    for n in 1..=3 {
        let Outcome::Published(id) = session
            .execute_request(
                Request {
                    id: RequestId::from(crate::RequestToken::new(NOW, [n; 20]).unwrap()),
                    intent: Intent::Publish(PublicationIntent {
                        group: cfg.group,
                        namespace: NamespaceId(0x42),
                        schema: SchemaId(7),
                        targets: Targets::OtherMembers,
                        body: vec![n, 0, 255],
                    }),
                    original_scope: Some(OriginalScope::Control {
                        control: cfg.control,
                        incarnation: cfg.incarnation,
                    }),
                },
                ExecutionContext { clock: clock(NOW) },
            )
            .unwrap()
        else {
            panic!("published")
        };
        ids.push(id);
    }
    assert_eq!(
        inspect_in(
            &database.begin_read().unwrap(),
            cfg.local_reticulum_identity
        )
        .unwrap()
        .unwrap()
        .records,
        3
    );
    session
        .execute(Command::Maintain(clock(NOW + 15 * 86400)))
        .unwrap();
    drop(session);
    drop(database);
    let database = Database::open(directory.path().join("node-0")).unwrap();
    let mut session = StoreSession::open(&database, cfg.clone()).unwrap();
    let Reply::Records(first) = session
        .execute(Command::Records {
            selection: crate::RecordSelection::All,
            group: cfg.group,
            before: None,
            limit: 1,
            clock: clock(NOW + 15 * 86400),
        })
        .unwrap()
    else {
        panic!("records")
    };
    assert_eq!(first.records.len(), 1);
    assert_eq!(first.records[0].record.metadata.record, ids[2]);
    assert_eq!(first.records[0].record.body.as_slice(), &[3, 0, 255]);
    let Reply::Records(second) = session
        .execute(Command::Records {
            selection: crate::RecordSelection::All,
            group: cfg.group,
            before: first.next_before,
            limit: 1,
            clock: clock(NOW + 15 * 86400),
        })
        .unwrap()
    else {
        panic!("records")
    };
    assert_eq!(second.records[0].record.metadata.record, ids[1]);
    let Reply::Record(Some(record)) = session
        .execute(Command::Record {
            group: cfg.group,
            incarnation: cfg.incarnation,
            record: ids[0],
            clock: clock(NOW + 15 * 86400),
        })
        .unwrap()
    else {
        panic!("record")
    };
    assert_eq!(record.record.body.as_slice(), &[1, 0, 255]);
    let Reply::Progress(Some(progress)) = session
        .execute(Command::Progress {
            group: cfg.group,
            incarnation: cfg.incarnation,
            record: ids[0],
            clock: clock(NOW + 15 * 86400),
        })
        .unwrap()
    else {
        panic!("progress")
    };
    assert_eq!(progress, record.progress);
    service(
        &database,
        &cfg,
        Arc::new(Participant::new(&cfg).unwrap()),
        |service| {
            assert!(service.record(ids[0]).unwrap().is_none());
        },
    );
    #[cfg(feature = "text")]
    {
        let Reply::Records(filtered) = session
            .execute(Command::Records {
                selection: crate::RecordSelection::Text,
                group: cfg.group,
                before: None,
                limit: 1,
                clock: clock(NOW),
            })
            .unwrap()
        else {
            panic!("records")
        };
        assert!(filtered.records.is_empty());
        assert!(
            filtered.next_before.is_some(),
            "filter must yield after its bounded scan"
        );
    }
}

struct ExitAfterStage(Participant);
impl TransactionParticipant for ExitAfterStage {
    fn stage(
        &self,
        tx: &WriteTransaction,
        context: CommitContext,
        values: &prns_groups_store_redb::CommitRead<'_>,
        ticket: OperationTicket,
        changes: &[Mutation],
    ) -> Result<Option<OperationRejection>, RedbError> {
        assert_eq!(self.0.stage(tx, context, values, ticket, changes)?, None);
        std::process::exit(73)
    }
    fn delete_in(&self, tx: &WriteTransaction, context: CommitContext) -> Result<(), RedbError> {
        self.0.delete_in(tx, context)
    }
}
#[test]
fn process_exit_before_commit_and_after_commit_keep_protocol_and_archive_atomic() {
    const ENV: &str = "PRNS_ARCHIVE_CRASH_CASE";
    let identity = IdentityHash::new([61; 16]);
    if let Some(case) = std::env::var_os(ENV) {
        let path = std::path::PathBuf::from(std::env::var_os("PRNS_ARCHIVE_CRASH_DIR").unwrap());
        let database = Database::open(path.join("store")).unwrap();
        let cfg = fixture::load_and_install(
            &database,
            &std::fs::read(path.join("fixture")).unwrap(),
            identity,
        )
        .unwrap();
        let participant: Arc<dyn TransactionParticipant> = if case == "before" {
            Arc::new(ExitAfterStage(Participant::new(&cfg).unwrap()))
        } else {
            Arc::new(Participant::new(&cfg).unwrap())
        };
        service(&database, &cfg, participant, |service| {
            service
                .publish(
                    next(service, &cfg),
                    &Publication {
                        nonce: [7; 32],
                        namespace: NamespaceId(3),
                        schema: SchemaId(4),
                        validity: PublicationValidity::new(
                            Timestamp(NOW),
                            Timestamp(NOW + 30),
                            Seconds(60),
                        )
                        .unwrap(),
                        targets: &[cfg.peers[0].endpoint],
                        body: b"crash boundary",
                    },
                    clock(NOW),
                )
                .unwrap();
            // Exit without destructors after the single durable commit but before
            // the caller could retain a reply or release the database normally.
            std::process::exit(73)
        });
        unreachable!();
    }
    for case in ["before", "after"] {
        let directory = tempfile::tempdir().unwrap();
        let fixtures = fixture::generate(
            "Crash archive",
            &[identity, IdentityHash::new([62; 16])],
            Timestamp(NOW),
        )
        .unwrap();
        std::fs::write(directory.path().join("fixture"), &fixtures[0]).unwrap();
        let database = Database::create(directory.path().join("store")).unwrap();
        let cfg = fixture::load_and_install(&database, &fixtures[0], identity).unwrap();
        let tx = database.begin_write().unwrap();
        initialize_in(&tx, identity, limits(2)).unwrap();
        tx.commit().unwrap();
        let next_sequence = {
            let mut owner = RedbStoreOwner::new(&database);
            owner
                .open(cfg.namespace, cfg.incarnation, cfg.store_limits)
                .unwrap()
                .head()
                .unwrap()
                .0
                + 1
        };
        drop(database);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "archive::tests::process_exit_before_commit_and_after_commit_keep_protocol_and_archive_atomic", "--nocapture"])
            .env(ENV, case).env("PRNS_ARCHIVE_CRASH_DIR", directory.path()).status().unwrap();
        assert_eq!(status.code(), Some(73));
        let database = Database::open(directory.path().join("store")).unwrap();
        let usage = inspect_in(&database.begin_read().unwrap(), identity)
            .unwrap()
            .unwrap();
        let mut owner = RedbStoreOwner::new(&database);
        let mut backend = owner
            .open(cfg.namespace, cfg.incarnation, cfg.store_limits)
            .unwrap();
        let recovered = backend
            .recover(
                OperationTicket::new(cfg.incarnation, next_sequence)
                    .unwrap()
                    .operation_id(),
            )
            .unwrap();
        if case == "before" {
            assert_eq!(usage.records, 0);
            assert!(matches!(recovered, RecoveryOutcome::NotCommitted { .. }));
        } else {
            assert_eq!(usage.records, 1);
            assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
        }
    }
}
