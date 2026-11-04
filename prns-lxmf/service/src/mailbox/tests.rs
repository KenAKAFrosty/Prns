//! Mailbox-only contracts: no runtime, host, app or network adapter is needed.
use super::*;
use prns_lxmf_wire::{compose_basic_direct_lxmf, BasicLxmfSigner};

struct Signer;
impl BasicLxmfSigner for Signer {
    fn sign_lxmf(&self, _: &[u8]) -> [u8; 64] {
        [7; 64]
    }
}
fn outbound(timestamp: u64) -> NewOutboundMessage {
    let (source, destination) = ([1; 16], [2; 16]);
    let mut output = [0; MAX_BASIC_LXMF_WIRE_BYTES];
    let wire = compose_basic_direct_lxmf(
        destination,
        source,
        timestamp,
        b"title",
        b"content",
        None,
        &Signer,
        &mut output,
    )
    .unwrap();
    NewOutboundMessage {
        message_id: wire.message_id(),
        source,
        destination,
        timestamp_unix_ms: timestamp,
        title: b"title".to_vec(),
        content: b"content".to_vec(),
        exact_wire: output[..usize::from(wire.wire_len())].to_vec(),
    }
}
fn stored(message: NewOutboundMessage) -> StoredRecord {
    StoredRecord {
        message_id: message.message_id,
        source: message.source,
        destination: message.destination,
        timestamp_unix_ms: message.timestamp_unix_ms,
        title: message.title,
        content: message.content,
        exact_wire: message.exact_wire,
        record_kind: StoredRecordKind::Outbound {
            generation: 1,
            failed_attempts: 0,
            state: StoredOutboundState::Queued,
        },
    }
}
fn database(limits: MailboxLimits) -> (tempfile::TempDir, Database) {
    let root = tempfile::tempdir().unwrap();
    let db = Database::create(root.path().join("mailbox.redb")).unwrap();
    let tx = db.begin_write().unwrap();
    initialize_mailbox_tables_with_limits(&tx, limits).unwrap();
    tx.commit().unwrap();
    (root, db)
}
fn insert(db: &Database, time: u64) -> DurableLxmfMessage {
    let MailboxReply::OutboundInserted { message, .. } =
        execute_mailbox_request(db, MailboxRequest::InsertOutbound(outbound(time))).unwrap()
    else {
        panic!("insert")
    };
    message
}
fn replay(
    db: &Database,
    after: Option<u64>,
    limit: usize,
) -> (Vec<DurableLxmfMessage>, Option<u64>) {
    let MailboxReply::QueuedPage { messages, next, .. } = execute_mailbox_request(
        db,
        MailboxRequest::Replay(MailboxReplayRequest { after, limit }),
    )
    .unwrap() else {
        panic!("page")
    };
    (messages, next)
}

#[test]
fn exact_quota_preserves_rows_and_funds_failure_retry_and_cancellation() {
    let charged = policy::charge(&stored(outbound(1))).unwrap();
    let (root, db) = database(MailboxLimits {
        max_records: 1,
        max_accounted_bytes: charged,
        ..MailboxLimits::default()
    });
    let message = insert(&db, 1);
    assert!(matches!(
        execute_mailbox_request(&db, MailboxRequest::InsertOutbound(outbound(2))),
        Err(MailboxFailure::Capacity)
    ));
    validate_mailbox_database(&db).unwrap();
    let key = AttemptKey {
        local_record_id: message.local_record_id,
        generation: 1,
    };
    assert!(matches!(
        execute_mailbox_request(
            &db,
            MailboxRequest::CompleteAttempt {
                key,
                completion: AttemptCompletion::Failed {
                    failure: DirectSendFailure::DeliveryTimedOut
                }
            }
        )
        .unwrap(),
        MailboxReply::Completion(CompletionTransition::Applied { .. })
    ));
    assert!(replay(&db, None, 1).0.is_empty());
    assert!(matches!(
        execute_mailbox_request(
            &db,
            MailboxRequest::Retry {
                local_record_id: message.local_record_id
            }
        )
        .unwrap(),
        MailboxReply::Retry(RetryTransition::Accepted { .. })
    ));
    assert_eq!(replay(&db, None, 1).0[0].generation(), Some(2));
    execute_mailbox_request(
        &db,
        MailboxRequest::Cancel {
            local_record_id: message.local_record_id,
            cancelled_at_millis: u64::MAX,
        },
    )
    .unwrap();
    assert!(replay(&db, None, 1).0.is_empty());
    validate_mailbox_database(&db).unwrap();
    drop(db);
    let db = Database::open(root.path().join("mailbox.redb")).unwrap();
    validate_mailbox_database(&db).unwrap();
    assert_eq!(
        read_record(&db.begin_write().unwrap(), message.local_record_id)
            .unwrap()
            .unwrap()
            .exact_wire,
        message.exact_wire
    );
}

#[test]
fn delivery_at_exact_byte_quota_needs_no_slack() {
    let bytes = policy::charge(&stored(outbound(1))).unwrap();
    let (_, db) = database(MailboxLimits {
        max_accounted_bytes: bytes,
        ..MailboxLimits::default()
    });
    let message = insert(&db, 1);
    execute_mailbox_request(
        &db,
        MailboxRequest::CompleteAttempt {
            key: AttemptKey {
                local_record_id: message.local_record_id,
                generation: 1,
            },
            completion: AttemptCompletion::Delivered {
                delivered_at_millis: u64::MAX,
                rtt_millis: Some(u64::MAX),
            },
        },
    )
    .unwrap();
    validate_mailbox_database(&db).unwrap();
    assert!(matches!(
        execute_mailbox_request(&db, MailboxRequest::InsertOutbound(outbound(2))),
        Err(MailboxFailure::Capacity)
    ));
}

#[test]
fn rejected_inbound_does_not_publish_deduplication_or_advance_ids() {
    let (_, db) = database(MailboxLimits {
        max_records: 1,
        ..MailboxLimits::default()
    });
    insert(&db, 1);
    let inbound = outbound(2);
    let rejected = execute_mailbox_request(
        &db,
        MailboxRequest::InsertInbound(NewInboundMessage {
            message_id: inbound.message_id,
            source: inbound.source,
            destination: inbound.destination,
            timestamp_unix_ms: inbound.timestamp_unix_ms,
            title: inbound.title,
            content: inbound.content,
            exact_wire: inbound.exact_wire,
            verification: LxmfVerification::SourceUnknown,
        }),
    );
    assert_eq!(rejected, Err(MailboxFailure::Capacity));
    let read = db.begin_read().unwrap();
    assert_eq!(read.open_table(LXMF_INBOUND_IDS).unwrap().len().unwrap(), 1);
    assert_eq!(
        required_meta(
            &read.open_table(LXMF_META).unwrap(),
            NEXT_LXMF_RECORD_ID_KEY
        )
        .unwrap(),
        2
    );
    validate_mailbox_database(&db).unwrap();
}

#[test]
fn replay_pages_bound_owned_bytes_and_resume_after_exclusive_id() {
    let bytes =
        (policy::maximum_owned_row_bytes() + std::mem::size_of::<Vec<DurableLxmfMessage>>()) as u64;
    let (_, db) = database(MailboxLimits {
        max_page_rows: 2,
        max_page_bytes: bytes,
        ..MailboxLimits::default()
    });
    for time in 1..=13 {
        insert(&db, time);
    }
    let mut after = None;
    let mut ids = Vec::new();
    loop {
        let (messages, next) = replay(&db, after, 2);
        assert_eq!(messages.len(), 1);
        let owned = std::mem::size_of::<Vec<DurableLxmfMessage>>()
            + messages.capacity() * std::mem::size_of::<DurableLxmfMessage>()
            + messages
                .iter()
                .map(|row| {
                    row.title.capacity() + row.content.capacity() + row.exact_wire.capacity()
                })
                .sum::<usize>();
        assert!(owned as u64 <= bytes);
        assert!(after.is_none_or(|after| messages[0].local_record_id > after));
        ids.push(messages[0].local_record_id);
        if next.is_none() {
            break;
        }
        after = next;
    }
    assert_eq!(ids, (1..=13).collect::<Vec<_>>());
    assert!(matches!(
        execute_mailbox_request(
            &db,
            MailboxRequest::Replay(MailboxReplayRequest {
                after: None,
                limit: 0
            })
        ),
        Err(MailboxFailure::Capacity)
    ));
    assert!(matches!(
        execute_mailbox_request(
            &db,
            MailboxRequest::Replay(MailboxReplayRequest {
                after: None,
                limit: 3
            })
        ),
        Err(MailboxFailure::Capacity)
    ));
}

#[test]
fn cold_inspection_rejects_missing_or_orphan_queue_and_wrong_generation() {
    for corruption in 0..4 {
        let (_, db) = database(MailboxLimits::default());
        insert(&db, 1);
        let tx = db.begin_write().unwrap();
        {
            let mut queue = tx.open_table(policy::QUEUED).unwrap();
            match corruption {
                0 => {
                    queue.remove(1).unwrap();
                }
                1 => {
                    queue.insert(2, 1).unwrap();
                }
                2 => {
                    queue.insert(1, 2).unwrap();
                }
                _ => {
                    queue.remove(0).unwrap();
                }
            }
        }
        tx.commit().unwrap();
        assert!(matches!(
            validate_mailbox_database(&db),
            Err(MailboxFailure::ResetRequired(_))
        ));
    }
}

#[test]
fn mutation_cannot_recreate_a_missing_empty_index() {
    let (_, db) = database(MailboxLimits::default());
    let tx = db.begin_write().unwrap();
    tx.delete_table(policy::QUEUED).unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        execute_mailbox_request(&db, MailboxRequest::InsertOutbound(outbound(1))),
        Err(MailboxFailure::ResetRequired(_))
    ));
    assert!(db.begin_read().unwrap().open_table(policy::QUEUED).is_err());
    assert_eq!(
        required_meta(
            &db.begin_read().unwrap().open_table(LXMF_META).unwrap(),
            NEXT_LXMF_RECORD_ID_KEY
        )
        .unwrap(),
        1
    );
}

#[test]
fn old_format_is_not_reinitialized_or_silently_migrated() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::create(root.path().join("old.redb")).unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(LXMF_META)
        .unwrap()
        .insert(NEXT_LXMF_RECORD_ID_KEY, 7)
        .unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        validate_mailbox_database(&db),
        Err(MailboxFailure::ResetRequired(_))
    ));
    let tx = db.begin_write().unwrap();
    assert!(initialize_mailbox_tables(&tx).is_err());
    drop(tx);
    assert_eq!(
        required_meta(
            &db.begin_read().unwrap().open_table(LXMF_META).unwrap(),
            NEXT_LXMF_RECORD_ID_KEY
        )
        .unwrap(),
        7
    );
}

#[test]
fn bounded_decoder_rejects_oversized_arrays_before_growing_them() {
    let mut row = stored(outbound(1));
    row.title = vec![1; MAX_BASIC_LXMF_WIRE_BYTES + 1];
    let encoded = serde_json::to_vec(&row).unwrap();
    assert!(matches!(
        decode_record(1, &encoded),
        Err(MailboxFailure::ResetRequired(_))
    ));
    assert!(decode_record(1, &vec![b' '; policy::maximum_encoded_bytes().unwrap() + 1]).is_err());
}

#[test]
fn cold_accounting_and_serializer_bounds_are_exact() {
    let (_, db) = database(MailboxLimits::default());
    insert(&db, 1);
    validate_mailbox_database(&db).unwrap();
    let tx = db.begin_write().unwrap();
    {
        let mut meta = tx.open_table(LXMF_META).unwrap();
        let count = required_meta(&meta, "lxmf_accounted_bytes").unwrap();
        meta.insert("lxmf_accounted_bytes", count + 1).unwrap();
    }
    tx.commit().unwrap();
    assert!(matches!(
        validate_mailbox_database(&db),
        Err(MailboxFailure::ResetRequired(_))
    ));
    let row = stored(outbound(1));
    let charged = policy::charge(&row).unwrap();
    for state in [
        StoredOutboundState::Queued,
        StoredOutboundState::Delivered {
            delivered_at_millis: u64::MAX,
            rtt_millis: RequiredNullableU64(Some(u64::MAX)),
        },
        StoredOutboundState::Failed {
            last_failure: StoredFailure::LocalNodeStopped,
        },
        StoredOutboundState::Cancelled {
            cancelled_at_millis: u64::MAX,
        },
    ] {
        let mut candidate = row.clone();
        candidate.record_kind = StoredRecordKind::Outbound {
            generation: u64::MAX,
            failed_attempts: u64::MAX,
            state,
        };
        assert!(
            serde_json::to_vec(&candidate).unwrap().len()
                <= policy::maximum_encoded_bytes().unwrap()
        );
        assert!(serde_json::to_vec(&candidate).unwrap().len() as u64 + 24 <= charged);
        assert_eq!(policy::charge(&candidate).unwrap(), charged);
    }
}

#[test]
fn page_budget_accounts_owned_capacity_not_only_lengths() {
    let mut message = public_message(1, stored(outbound(1)));
    message.content.reserve(4096);
    let rows = vec![message];
    let limits = MailboxLimits {
        max_page_rows: 1,
        max_page_bytes: (policy::maximum_owned_row_bytes()
            + std::mem::size_of::<Vec<DurableLxmfMessage>>()) as u64,
        ..MailboxLimits::default()
    };
    assert!(matches!(
        policy::check_page(limits, &rows),
        Err(MailboxFailure::Capacity)
    ));
}

#[test]
fn mailbox_metadata_coexists_with_an_unrelated_differently_typed_application_table() {
    const OTHER: TableDefinition<u64, &str> = TableDefinition::new("development_meta");
    let root = tempfile::tempdir().unwrap();
    let db = Database::create(root.path().join("shared.redb")).unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(OTHER)
        .unwrap()
        .insert(17, "unrelated application schema")
        .unwrap();
    initialize_mailbox_tables(&tx).unwrap();
    tx.commit().unwrap();
    insert(&db, 1);
    validate_mailbox_database(&db).unwrap();
    assert_eq!(
        db.begin_read()
            .unwrap()
            .open_table(OTHER)
            .unwrap()
            .get(17)
            .unwrap()
            .unwrap()
            .value(),
        "unrelated application schema"
    );
}

#[test]
fn first_create_rejects_empty_partial_mailbox_tables_without_staging_repair() {
    for multimap in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let db = Database::create(root.path().join("partial.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        if multimap {
            tx.open_multimap_table(redb::MultimapTableDefinition::<u64, u64>::new(
                "lxmf_messages",
            ))
            .unwrap();
        } else {
            tx.open_table(LXMF_MESSAGES).unwrap();
        }
        tx.commit().unwrap();
        let tx = db.begin_write().unwrap();
        assert!(matches!(
            initialize_mailbox_tables(&tx),
            Err(MailboxFailure::ResetRequired(_))
        ));
        // Even an erroneous caller committing after rejection cannot publish a
        // replacement metadata table: rejection preceded all table writes.
        tx.commit().unwrap();
        assert!(db.begin_read().unwrap().open_table(LXMF_META).is_err());
    }
}
