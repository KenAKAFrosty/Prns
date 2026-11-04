#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
const NOW: u64 = 1_800_000_000;
fn fixture(db: &Database) -> (Enrollment<'_>, RedbStoreOwner<'_>, Entry) {
    let engine = Enrollment::create_new(db, IdentityHash::new([77; 16])).unwrap();
    let mut owner = RedbStoreOwner::new(db);
    let id = prns_groups_core::retry::RequestToken::new(NOW, [42; 20])
        .unwrap()
        .into_bytes();
    let group = engine.create(&mut owner, id, "keyed state", NOW).unwrap();
    let catalog = engine.read().unwrap();
    let entry = Enrollment::find(&catalog, group.0).unwrap().clone();
    (engine, owner, entry)
}
fn receipt(n: u64) -> Request {
    let mut entropy = [0; 20];
    entropy[..8].copy_from_slice(&n.to_be_bytes());
    let id = prns_groups_core::retry::RequestToken::new(NOW, entropy)
        .unwrap()
        .into_bytes();
    Request {
        id,
        fingerprint: [9; 32],
        result: [10; 32],
    }
}
#[test]
fn large_history_reopens_with_exact_reads_and_streamed_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.redb");
    let db = Database::create(&path).unwrap();
    let (engine, mut owner, entry) = fixture(&db);
    const COUNT: u64 = 2048;
    for batch in 0..COUNT / 32 {
        engine
            .management_adapter(&mut owner, &entry, None, |adapter, mut state| {
                adapter
                    .maintain(Enrollment::next_maintenance(adapter, &entry)?, &[], |tx| {
                        for n in batch * 32..(batch + 1) * 32 {
                            state.stage_request(tx, &receipt(n))?;
                        }
                        state.stage(tx)
                    })
                    .map_err(composition_error)?;
                Ok(())
            })
            .unwrap();
    }
    engine
        .adapter(&mut owner, &entry, None, |adapter, state| {
            assert_eq!(state.management_count, COUNT);
            let bytes = adapter.application_read(STATE_KEY).unwrap().unwrap();
            assert!(
                bytes.len() < 4096,
                "history must not grow the current header"
            );
            READS.with(|n| n.set(0));
            assert_eq!(
                state.management(adapter, receipt(2000).id)?.unwrap().result,
                [10; 32]
            );
            assert_eq!(
                READS.with(|n| n.get()),
                2,
                "primary and reciprocal ordinal only"
            );
            READS.with(|n| n.set(0));
            state.inspect(adapter, &entry)?;
            assert_eq!(READS.with(|n| n.get()), COUNT as usize * 3);
            Ok(())
        })
        .unwrap();
    drop(owner);
    drop(engine);
    drop(db);
    let db = Database::open(&path).unwrap();
    let engine = Enrollment::open_existing(&db, IdentityHash::new([77; 16])).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    for n in 0..100 {
        READS.with(|n| n.set(0));
        engine
            .adapter(&mut owner, &entry, None, |adapter, state| {
                assert_eq!(state.management_count, COUNT);
                assert!(state.management(adapter, receipt(n).id)?.is_some());
                Ok(())
            })
            .unwrap();
        assert_eq!(READS.with(|n| n.get()), 2);
    }
}
#[test]
fn immutable_receipt_and_counter_rollback_are_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("state.redb")).unwrap();
    let (engine, mut owner, entry) = fixture(&db);
    let result = engine.management_adapter(&mut owner, &entry, None, |adapter, mut state| {
        adapter
            .maintain(Enrollment::next_maintenance(adapter, &entry)?, &[], |tx| {
                state.stage_request(tx, &receipt(1))?;
                state.stage(tx)?;
                Err::<(), _>(EnrollmentError::Conflict)
            })
            .map_err(composition_error)?;
        Ok(())
    });
    assert!(matches!(result, Err(EnrollmentError::Conflict)));
    engine
        .adapter(&mut owner, &entry, None, |adapter, state| {
            assert_eq!(state.management_count, 0);
            assert!(state.management(adapter, receipt(1).id)?.is_none());
            Ok(())
        })
        .unwrap();
    for _ in 0..2 {
        engine
            .management_adapter(&mut owner, &entry, None, |adapter, mut state| {
                adapter
                    .maintain(Enrollment::next_maintenance(adapter, &entry)?, &[], |tx| {
                        state.stage_request(tx, &receipt(1))?;
                        state.stage(tx)
                    })
                    .map_err(composition_error)?;
                Ok(())
            })
            .unwrap();
    }
    let result = engine.management_adapter(&mut owner, &entry, None, |adapter, mut state| {
        let mut changed = receipt(1);
        changed.fingerprint = [8; 32];
        adapter
            .maintain(Enrollment::next_maintenance(adapter, &entry)?, &[], |tx| {
                state.stage_request(tx, &changed)?;
                state.stage(tx)
            })
            .map_err(composition_error)?;
        Ok(())
    });
    assert!(matches!(result, Err(EnrollmentError::Conflict)));
    engine
        .adapter(&mut owner, &entry, None, |adapter, state| {
            assert_eq!(state.management_count, 1);
            assert_eq!(
                state
                    .management(adapter, receipt(1).id)?
                    .unwrap()
                    .fingerprint,
                [9; 32]
            );
            Ok(())
        })
        .unwrap();
}
#[test]
fn broken_reciprocal_index_is_not_healed_by_duplicate_write() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("state.redb")).unwrap();
    let (engine, mut owner, entry) = fixture(&db);
    engine
        .management_adapter(&mut owner, &entry, None, |adapter, mut state| {
            adapter
                .maintain(Enrollment::next_maintenance(adapter, &entry)?, &[], |tx| {
                    state.stage_request(tx, &receipt(1))?;
                    state.stage(tx)
                })
                .map_err(composition_error)?;
            let rejected =
                adapter.maintain(Enrollment::next_maintenance(adapter, &entry)?, &[], |tx| {
                    tx.delete(&key(REQUEST_ORDER, &0u64.to_be_bytes()))
                        .map_err(|_| EnrollmentError::Corrupt)
                });
            assert!(
                rejected.is_err(),
                "the participant refuses orphaning its exact receipt index"
            );
            Ok(())
        })
        .unwrap();
    // Corrupt the application index through a raw backend writer, outside the
    // participant contract, while preserving native metadata integrity.
    {
        let incarnation = StoreIncarnation::new(entry.incarnation).unwrap();
        let limits = engine.limits().unwrap().namespace;
        let backend = owner
            .open(StoreNamespace(entry.namespace), incarnation, limits)
            .unwrap();
        let coordinator = Coordinator::new(backend, limits.transaction_limits().unwrap()).unwrap();
        let head = coordinator.head().unwrap();
        let ticket = prns_groups_store::OperationTicket::new(incarnation, head.0 + 1).unwrap();
        let tx = coordinator.begin(ticket.operation_id(), head).unwrap();
        let mut raw = vec![0];
        raw.extend(key(REQUEST_ORDER, &0u64.to_be_bytes()));
        tx.delete(&raw).unwrap();
        tx.commit().unwrap();
    }
    engine
        .management_adapter(&mut owner, &entry, None, |adapter, mut state| {
            assert!(matches!(
                state.management(adapter, receipt(1).id),
                Err(EnrollmentError::Corrupt)
            ));
            assert!(matches!(
                state.inspect(adapter, &entry),
                Err(EnrollmentError::Corrupt)
            ));
            let result = adapter
                .maintain(Enrollment::next_maintenance(adapter, &entry)?, &[], |tx| {
                    state.stage_request(tx, &receipt(1))?;
                    state.stage(tx)
                })
                .map_err(composition_error);
            assert!(matches!(result, Err(EnrollmentError::Corrupt)));
            Ok(())
        })
        .unwrap();
}
#[test]
fn cold_prefix_counts_reject_orphan_and_old_or_unknown_format() {
    let mut scan = Inspection::new();
    let mut raw = vec![0];
    raw.extend(key(ACK, &[1; 32]));
    scan.observe(&raw, b"true").unwrap();
    assert!(scan.finish().is_err());
    let mut scan = Inspection::new();
    let mut state_key = vec![0];
    state_key.extend(STATE_KEY);
    scan.observe(&state_key, &encode(&Header::default()).unwrap())
        .unwrap();
    scan.observe(&raw, b"true").unwrap();
    assert!(scan.finish().is_err());
    let mut scan = Inspection::new();
    let mut invalid = vec![0];
    invalid.extend(key(99, &[]));
    assert!(scan.observe(&invalid, b"{}").is_err());
    let legacy=br#"{"current":null,"required_tip":null,"package":null,"controls":[],"added":[],"lifecycle":"Active","terminals":[],"notice_acks":[],"management":[]}"#;
    assert!(Snapshot::decode(Some(legacy), profile::store_limits()).is_err());
    let header = Header {
        version: 99,
        ..Default::default()
    };
    assert!(matches!(
        Snapshot::decode(Some(&encode(&header).unwrap()), profile::store_limits()),
        Err(EnrollmentError::UnsupportedVersion)
    ));
}
#[test]
fn bounded_row_failure_does_not_publish_index_or_header() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("state.redb")).unwrap();
    let (engine, mut owner, entry) = fixture(&db);
    let result = engine.management_adapter(&mut owner, &entry, None, |adapter, mut state| {
        state.limits.max_value_bytes = 8;
        adapter
            .maintain(Enrollment::next_maintenance(adapter, &entry)?, &[], |tx| {
                state.stage_request(tx, &receipt(1))?;
                state.stage(tx)
            })
            .map_err(composition_error)?;
        Ok(())
    });
    assert!(matches!(result, Err(EnrollmentError::Capacity)));
    engine
        .adapter(&mut owner, &entry, None, |adapter, state| {
            assert_eq!(state.management_count, 0);
            assert!(state.management(adapter, receipt(1).id)?.is_none());
            assert!(adapter
                .application_read(&key(REQUEST_ORDER, &0u64.to_be_bytes()))
                .unwrap()
                .is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn strict_native_visit_rejects_orphan_facts_even_with_valid_native_accounting() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("state.redb")).unwrap();
    let (engine, mut owner, entry) = fixture(&db);
    let scan = |owner: &mut RedbStoreOwner<'_>| {
        let mut backend = owner
            .open(
                StoreNamespace(entry.namespace),
                StoreIncarnation::new(entry.incarnation).unwrap(),
                profile::store_limits(),
            )
            .unwrap();
        let mut inspection = Inspection::new();
        backend
            .inspect_with(|key, value| inspection.observe(key, value))
            .unwrap();
        inspection.finish()
    };
    scan(&mut owner).unwrap();
    engine
        .management_adapter(&mut owner, &entry, None, |adapter, _| {
            adapter
                .maintain(Enrollment::next_maintenance(adapter, &entry)?, &[], |tx| {
                    tx.put(
                        &key(ACK, &[7; 32]),
                        &encode(&Stored {
                            ordinal: 0,
                            value: true,
                        })?,
                    )
                    .map_err(|_| EnrollmentError::Corrupt)
                })
                .map_err(composition_error)?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(scan(&mut owner), Err(RedbError::Corrupt)));
    // Readiness invokes the same owning visitor, not only direct test scans.
    assert!(matches!(
        Enrollment::open_existing(&db, IdentityHash::new([77; 16])),
        Err(EnrollmentError::Storage(RedbError::Corrupt))
    ));
}

#[test]
fn typed_projection_uses_owning_key_and_stored_row_decoder() {
    use prns_groups_store::Mutation;
    let application = |suffix: &[u8]| {
        let mut bytes = vec![0];
        bytes.extend_from_slice(suffix);
        bytes
    };
    let mutations = vec![
        Mutation::Put {
            key: application(STATE_KEY),
            value: encode(&Header::default()).unwrap(),
        },
        Mutation::Put {
            key: application(&key(REQUEST, &receipt(2).id)),
            value: encode(&Stored {
                ordinal: 0,
                value: receipt(2),
            })
            .unwrap(),
        },
        Mutation::Put {
            key: vec![3, 8],
            value: vec![1],
        },
    ];
    let mut seen = 0;
    visit_mutations(&mutations, |projection| {
        match projection {
            Projection::Header(summary) => {
                assert_eq!(summary.lifecycle, GroupLifecycle::Active);
                assert!(!summary.ready);
                assert_eq!(summary.member_count, 0);
                assert!(summary.head.is_none());
            }
            Projection::Request(request) => assert_eq!(request.id, receipt(2).id),
            Projection::Added { .. } | Projection::Terminal(_) => panic!("unexpected domain fact"),
        }
        seen += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, 2);
    let wrong = [Mutation::Put {
        key: application(&key(REQUEST, &receipt(3).id)),
        value: encode(&Stored {
            ordinal: 0,
            value: receipt(2),
        })
        .unwrap(),
    }];
    assert!(visit_mutations(&wrong, |_| Ok(())).is_err());
    let removed = [Mutation::Delete {
        key: application(STATE_KEY),
    }];
    assert!(visit_mutations(&removed, |_| Ok(())).is_err());
}

#[test]
fn owning_envelope_bound_also_covers_every_namespace_row() {
    for members in [1, 8, 256, 512] {
        let profile = crate::GroupProfile {
            max_members: members,
            ..Default::default()
        };
        let bound = required_frame_bytes(profile).unwrap();
        let member = Member {
            rns: [255; 16],
            endpoint: [255; 32],
            incarnation: u64::MAX,
            key: [255; 32],
        };
        let signed = Signed {
            manifest: Manifest {
                version: WIRE_VERSION,
                group: [255; 32],
                owner: [255; 16],
                title: "\0".repeat(profile::MAX_TITLE_BYTES),
                profile,
                parent: [255; 32],
                revision: u64::MAX,
                epoch: u64::MAX,
                commit_digest: [255; 32],
                kind: ControlKind::Remove(member.clone()),
                members: vec![member.clone(); members as usize],
            },
            signature: vec![255; 64],
        };
        let opaque = vec![255; profile.max_wire_bytes as usize];
        let mut header = Header {
            current: Some(signed.clone()),
            required_tip: Some(signed.clone()),
            control_count: u64::MAX,
            added_count: u64::MAX,
            added_member_count: u64::MAX,
            terminal_count: u64::MAX,
            terminal_subject_count: u64::MAX,
            ack_count: u64::MAX,
            management_count: u64::MAX,
            past_member_count: u64::MAX,
            ..Default::default()
        };
        assert!(encode(&header).unwrap().len() <= bound);
        header.current = None;
        header.required_tip = None;
        header.package = Some(opaque.clone());
        let encoded = encode(&header).unwrap();
        assert!(encoded.len() <= bound);
        let decoded: Header = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.package, header.package);
        let added = Stored {
            ordinal: u64::MAX,
            value: Added {
                member: member.clone(),
                invitation: [255; 32],
                request_hash: [255; 32],
                signed: signed.clone(),
                welcome: opaque.clone(),
            },
        };
        let control = Stored {
            ordinal: u64::MAX,
            value: Control {
                signed: signed.clone(),
                commit: opaque,
            },
        };
        let terminal = Stored {
            ordinal: u64::MAX,
            value: Terminal {
                statement: TerminalStatement {
                    version: WIRE_VERSION,
                    group: [255; 32],
                    title: signed.manifest.title.clone(),
                    owner: member.clone(),
                    head: [255; 32],
                    revision: u64::MAX,
                    kind: TerminalKind::Removed(member.clone()),
                },
                signature: vec![255; 64],
            },
        };
        for row in [
            encode(&added).unwrap(),
            encode(&control).unwrap(),
            encode(&terminal).unwrap(),
            encode(&Stored {
                ordinal: u64::MAX,
                value: PastMember {
                    member,
                    removed_epoch: u64::MAX,
                },
            })
            .unwrap(),
            encode(&Stored {
                ordinal: u64::MAX,
                value: receipt(u64::MAX),
            })
            .unwrap(),
        ] {
            assert!(row.len() <= bound, "row {} > bound {}", row.len(), bound);
        }
    }
}

#[test]
fn managed_cleanup_funds_actual_wide_roster_controls_not_only_opaque_mls_bytes() {
    let profile = crate::GroupProfile {
        max_members: 256,
        max_enrollment_frame_bytes: 524_288,
        ..Default::default()
    };
    let native = profile::store_limits();
    let legacy = profile::management_reserve(native, profile.mls_limits().unwrap(), 256).unwrap();
    let reserve = profile::managed_cleanup_reserve(native, profile, 256).unwrap();
    let member = Member {
        rns: [255; 16],
        endpoint: [255; 32],
        incarnation: u64::MAX,
        key: [255; 32],
    };
    let mut actual = 0usize;
    for remaining in 1..256 {
        let signed = Signed {
            manifest: Manifest {
                version: WIRE_VERSION,
                group: [255; 32],
                owner: [255; 16],
                title: "\0".repeat(profile::MAX_TITLE_BYTES),
                profile,
                parent: [255; 32],
                revision: u64::MAX,
                epoch: u64::MAX,
                commit_digest: [255; 32],
                kind: ControlKind::Remove(member.clone()),
                members: vec![member.clone(); remaining],
            },
            signature: vec![255; 64],
        };
        let control = Stored {
            ordinal: u64::MAX,
            value: Control {
                signed: signed.clone(),
                commit: vec![255; profile.max_wire_bytes as usize],
            },
        };
        let terminal = Stored {
            ordinal: u64::MAX,
            value: Terminal {
                statement: TerminalStatement {
                    version: WIRE_VERSION,
                    group: [255; 32],
                    title: signed.manifest.title.clone(),
                    owner: member.clone(),
                    head: [255; 32],
                    revision: u64::MAX,
                    kind: TerminalKind::Removed(member.clone()),
                },
                signature: vec![255; 64],
            },
        };
        let request = Stored {
            ordinal: u64::MAX,
            value: receipt(u64::MAX),
        };
        // Actual retained payloads alone already exceed the old reservation;
        // the new bound also funds the key/index and receipt allowances.
        actual += encode(&control).unwrap().len()
            + encode(&terminal).unwrap().len()
            + encode(&request).unwrap().len();
    }
    assert!(
        actual > legacy.bytes,
        "regression must expose the omitted roster/base64 liability"
    );
    assert!(actual < reserve.bytes);
    let two = profile::managed_cleanup_reserve(native, profile, 2).unwrap();
    assert!(
        two.bytes < reserve.bytes / 10,
        "unused supported memberships must not be reserved"
    );
}

#[test]
fn small_roster_wide_wire_reserves_separate_raw_output_without_allocating_it() {
    let wire = 16 * 1024 * 1024;
    let profile = crate::GroupProfile {
        max_members: 4,
        max_wire_bytes: wire,
        max_enrollment_frame_bytes: 64 * 1024 * 1024,
        ..Default::default()
    };
    profile.validate().unwrap();
    let control_bound = required_frame_bytes_for_members(profile, 4).unwrap();
    let native = prns_groups_store_redb::RedbLimits {
        max_value_bytes: control_bound as u64,
        ..profile::store_limits()
    };
    let reserve = profile::managed_cleanup_reserve(native, profile, 4).unwrap();
    let raw = prns_groups_mls::Output::control_retention_bytes(wire as usize).unwrap();
    let member = Member {
        rns: [255; 16],
        endpoint: [255; 32],
        incarnation: u64::MAX,
        key: [255; 32],
    };
    let mut actual = 0usize;
    for members in 1..4 {
        let signed = Signed {
            manifest: Manifest {
                version: WIRE_VERSION,
                group: [255; 32],
                owner: [255; 16],
                title: "\0".repeat(profile::MAX_TITLE_BYTES),
                profile,
                parent: [255; 32],
                revision: u64::MAX,
                epoch: u64::MAX,
                commit_digest: [255; 32],
                kind: ControlKind::Remove(member.clone()),
                members: vec![member.clone(); members],
            },
            signature: vec![255; 64],
        };
        let control = Stored {
            ordinal: u64::MAX,
            value: Control {
                signed,
                commit: Vec::new(),
            },
        };
        // The owning byte codec replaces the empty string with this exact
        // base64 length; no 16 MiB payload/MLS group is allocated by this test.
        actual += encode(&control).unwrap().len()
            + base64::encoded_len(wire as usize, true).unwrap()
            + raw;
    }
    let missing_raw_output = reserve.bytes - raw * 4;
    assert!(
        actual > missing_raw_output,
        "keyed Control alone cannot reserve the separate MLS output"
    );
    assert!(actual < reserve.bytes);
}

#[test]
fn cleanup_fact_margin_covers_maximum_payloads_indexes_and_keys() {
    let member = Member {
        rns: [255; 16],
        endpoint: [255; 32],
        incarnation: u64::MAX,
        key: [255; 32],
    };
    let terminal = Stored {
        ordinal: u64::MAX,
        value: Terminal {
            statement: TerminalStatement {
                version: WIRE_VERSION,
                group: [255; 32],
                title: "\0".repeat(profile::MAX_TITLE_BYTES),
                owner: member.clone(),
                head: [255; 32],
                revision: u64::MAX,
                kind: TerminalKind::Removed(member.clone()),
            },
            signature: vec![255; 64],
        },
    };
    let request = Stored {
        ordinal: u64::MAX,
        value: receipt(u64::MAX),
    };
    let past = Stored {
        ordinal: u64::MAX,
        value: PastMember {
            member,
            removed_epoch: u64::MAX,
        },
    };
    let ack = Stored {
        ordinal: u64::MAX,
        value: true,
    };
    // Payloads not already funded by the Control/raw Output/snapshot/header
    // terms. Index values are exact maximum-width owning serde encodings.
    let payloads = encode(&terminal).unwrap().len()
        + encode(&request).unwrap().len()
        + encode(&past).unwrap().len()
        + encode(&ack).unwrap().len()
        + 2 * encode(&u64::MAX).unwrap().len() // Control head and parent.
        + encode(&[255u8; 8]).unwrap().len() // Control ordinal.
        + encode(&[255u8; 40].as_slice()).unwrap().len() // Past-member ordinal.
        + 4 * encode(&[255u8; 32]).unwrap().len(); // Terminal subject/order, request/ack order.
                                                   // All application keys are at most 256 bytes including their partition;
                                                   // MLS output/snapshot keys are shorter. Fund all sixteen cleanup slots,
                                                   // including replacement slots, rather than relying on their current names.
    let keys = 16 * (prns_groups_mls::MAX_APPLICATION_KEY_BYTES + 1);
    assert!(
        payloads + keys <= profile::CLEANUP_FACT_OVERHEAD,
        "{} > {}",
        payloads + keys,
        profile::CLEANUP_FACT_OVERHEAD
    );
}

#[test]
fn cold_missing_state_header_accepts_empty_initialization_but_rejects_partial_owned_values() {
    Inspection::new().finish().unwrap();
    let mut inspection = Inspection::new();
    inspection
        .observe(b"some MLS-owned native key", b"bounded value")
        .unwrap();
    assert!(matches!(inspection.finish(), Err(RedbError::Corrupt)));
}

#[test]
fn cold_readiness_distinguishes_absent_empty_and_partial_admitted_namespaces() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("pending.redb")).unwrap();
    let local = IdentityHash::new([77; 16]);
    let engine = Enrollment::create_new(&db, local).unwrap();
    let entry = engine
        .entry([7; 32], "pending", Default::default(), None, NOW)
        .unwrap();
    let read = engine.read().unwrap();
    let edit = super::super::indexed::Edit::new(&read)
        .unwrap()
        .put_current(
            &read,
            entry.group,
            &super::super::indexed::rows::Current::pending(&entry),
        )
        .unwrap()
        .put_incarnation(
            &read,
            super::super::indexed::keys::incarnation(entry.group, entry.generation),
            &super::super::indexed::rows::Incarnation::Live(entry.clone()),
        )
        .unwrap();
    engine.write(edit.finish().unwrap()).unwrap();
    let before = engine.read().unwrap().header.fingerprint().unwrap();
    Enrollment::open_existing(&db, local).unwrap();
    assert_eq!(engine.read().unwrap().header.fingerprint().unwrap(), before);
    let limits = engine.limits().unwrap().namespace;
    let incarnation = StoreIncarnation::new(entry.incarnation).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    drop(
        owner
            .initialize(StoreNamespace(entry.namespace), incarnation, limits)
            .unwrap(),
    );
    Enrollment::open_existing(&db, local).unwrap();
    assert_eq!(engine.read().unwrap().header.fingerprint().unwrap(), before);
    {
        let backend = owner
            .open(StoreNamespace(entry.namespace), incarnation, limits)
            .unwrap();
        let coordinator = Coordinator::new(backend, limits.transaction_limits().unwrap()).unwrap();
        let ticket = prns_groups_store::OperationTicket::new(incarnation, 1).unwrap();
        let tx = coordinator
            .begin(ticket.operation_id(), coordinator.head().unwrap())
            .unwrap();
        tx.put(&[3, 99], b"partial owned crypto state").unwrap();
        tx.commit().unwrap();
    }
    assert!(matches!(
        Enrollment::open_existing(&db, local),
        Err(EnrollmentError::Storage(RedbError::Corrupt))
    ));
    assert_eq!(engine.read().unwrap().header.fingerprint().unwrap(), before);
}
