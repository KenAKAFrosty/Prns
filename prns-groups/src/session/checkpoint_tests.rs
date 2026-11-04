//! File-backed recovery at the receive, receipt, and cursor persistence cuts.
#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::fixture;
use prns_groups_service::{ClockInterval, InventoryCursor, InventorySnapshot};
use prns_host::IdentityHash;

const NOW: u64 = 1_792_000_000;
const IDS: [IdentityHash; 2] = [IdentityHash::new([11; 16]), IdentityHash::new([22; 16])];

fn clock() -> ClockReading {
    ClockReading::Trusted(ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap())
}

fn completed_cursor(
    peer: Endpoint,
    recipient: Endpoint,
    group: prns_groups_core::GroupId,
    snapshot: InventorySnapshot,
) -> SyncCursor {
    // Construct a complete receipt pass over [0, head] through the public
    // checkpoint decoder; transport cursor fields are private.
    let mut bytes = [0; prns_groups_reticulum::SYNC_CURSOR_BYTES];
    bytes[..4].copy_from_slice(b"PGC\x02");
    bytes[4..36].copy_from_slice(&peer.id.0);
    bytes[36..44].copy_from_slice(&peer.incarnation.get().to_be_bytes());
    bytes[44..76].copy_from_slice(&recipient.id.0);
    bytes[76..84].copy_from_slice(&recipient.incarnation.get().to_be_bytes());
    bytes[84..116].copy_from_slice(&group.0);
    bytes[116..148].copy_from_slice(&snapshot.generation);
    bytes[148..156].copy_from_slice(&snapshot.head.0.to_be_bytes());
    bytes[156] = 2;
    bytes[157..165].copy_from_slice(&snapshot.head.0.to_be_bytes());
    SyncCursor::from_bytes(&bytes).unwrap()
}

fn fixture_config(db: &Database, directory: &std::path::Path, index: usize) -> Provisioned {
    let fixture = std::fs::read(directory.join(format!("fixture-{index}.json"))).unwrap();
    fixture::load_and_install(db, &fixture, IDS[index]).unwrap()
}

#[test]
fn process_child() {
    let Some(directory) = std::env::var_os("PRNS_GROUP_CHECKPOINT_CRASH_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    let cut = std::env::var("PRNS_GROUP_CHECKPOINT_CRASH_CUT").unwrap();
    let alice_db = Database::create(directory.join("alice.redb")).unwrap();
    let bob_db = Database::create(directory.join("bob.redb")).unwrap();
    let alice_cfg = fixture_config(&alice_db, &directory, 0);
    let bob_cfg = fixture_config(&bob_db, &directory, 1);
    let mut alice = StoreSession::open(&alice_db, alice_cfg).unwrap();
    let mut bob = StoreSession::open(&bob_db, bob_cfg).unwrap();
    let sender = alice.config().unwrap().local;
    let recipient = bob.config().unwrap().local;
    let group = alice.group();
    let Reply::Outcome(crate::Outcome::Published(record)) = alice
        .execute(crate::test_support::command(crate::test_support::text(
            group,
            crate::OriginalScope::Control {
                control: alice.control(),
                incarnation: alice.provisioned.incarnation,
            },
            crate::RequestToken::new(NOW, [9; 20]).unwrap().into_bytes(),
            "receive and cursor survive different crash cuts".into(),
            clock(),
        )))
        .unwrap()
    else {
        panic!("send reply");
    };
    let (snapshot, wire) = alice
        .with_service(|service, _| {
            let snapshot = service.snapshot()?.unwrap();
            let page = service.inventory(
                snapshot,
                InventoryCursor(0),
                InventoryKind::Record,
                recipient,
                32,
            )?;
            Ok((
                snapshot,
                service.envelope(page.items[0].id)?.unwrap().to_vec(),
            ))
        })
        .unwrap();
    bob.with_service(|service, cfg| {
        assert_eq!(
            service.receive(next(service, cfg)?, &wire, clock())?,
            Incoming::Record(record)
        );
        assert!(service.has_pending_receipt(record)?);
        Ok(())
    })
    .unwrap();
    if cut != "receive" {
        bob.execute(Command::RepairReceipts).unwrap();
    }
    if cut == "checkpoint" {
        bob.execute(Command::SaveCursor(
            sender,
            Some(completed_cursor(sender, recipient, group, snapshot)),
        ))
        .unwrap();
    }
    // No database/session destructors run. The parent independently reopens
    // both stores and reconciles the signed receipt after every cut.
    std::process::exit(73);
}

#[test]
fn receive_receipt_and_completed_checkpoint_survive_abrupt_exit_independently() {
    let fixtures = fixture::generate("Cursor recovery", &IDS, Timestamp(NOW)).unwrap();
    for cut in ["receive", "receipt", "checkpoint"] {
        let directory = tempfile::tempdir().unwrap();
        for (index, fixture) in fixtures.iter().enumerate() {
            std::fs::write(
                directory.path().join(format!("fixture-{index}.json")),
                fixture,
            )
            .unwrap();
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "session::checkpoint_tests::process_child",
                "--nocapture",
            ])
            .env("PRNS_GROUP_CHECKPOINT_CRASH_DIR", directory.path())
            .env("PRNS_GROUP_CHECKPOINT_CRASH_CUT", cut)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73), "cut {cut}");
        let alice_db = Database::open(directory.path().join("alice.redb")).unwrap();
        let bob_db = Database::open(directory.path().join("bob.redb")).unwrap();
        let alice_cfg = fixture_config(&alice_db, directory.path(), 0);
        let bob_cfg = fixture_config(&bob_db, directory.path(), 1);
        let mut alice = StoreSession::open(&alice_db, alice_cfg).unwrap();
        let mut bob = StoreSession::open(&bob_db, bob_cfg).unwrap();
        let sender = alice.config().unwrap().local;
        let Reply::Cursor(cursor) = bob.execute(Command::LoadCursor(sender)).unwrap() else {
            panic!("cursor reply");
        };
        assert_eq!(cursor.is_some(), cut == "checkpoint");
        assert!(cursor.is_none_or(|cursor| cursor.is_complete()));
        let Reply::Messages(page) = bob
            .execute(Command::Messages {
                group: bob.group(),
                before: None,
                limit: 32,
                clock: clock(),
            })
            .unwrap()
        else {
            panic!("messages reply");
        };
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].position.group, bob.group());
        assert_eq!(
            page.messages[0].position.incarnation,
            bob.provisioned.incarnation
        );
        assert!(page.messages[0].position.position <= page.visited_heads[0].position);
        let record = page.messages[0].id;
        bob.with_service(|service, _| {
            assert_eq!(service.has_pending_receipt(record)?, cut == "receive");
            Ok(())
        })
        .unwrap();
        bob.execute(Command::RepairReceipts).unwrap();
        let (revision, receipt) = bob
            .with_service(|service, _| {
                assert!(!service.has_pending_receipt(record)?);
                let snapshot = service.snapshot()?.unwrap();
                let page = service.inventory(
                    snapshot,
                    InventoryCursor(0),
                    InventoryKind::Receipt,
                    sender,
                    32,
                )?;
                assert_eq!(page.items.len(), 1);
                Ok((
                    snapshot,
                    service.envelope(page.items[0].id)?.unwrap().to_vec(),
                ))
            })
            .unwrap();
        bob.execute(Command::RepairReceipts).unwrap();
        bob.with_service(|service, _| {
            assert_eq!(service.snapshot()?, Some(revision));
            Ok(())
        })
        .unwrap();
        alice.with_service(|service, cfg| {
            assert!(matches!(service.receive(next(service, cfg)?, &receipt, clock())?, Incoming::Receipt(found) if found.record == record));
            assert_eq!(service.progress(record, clock())?.received, 1);
            Ok(())
        }).unwrap();
        if cut == "checkpoint" {
            // Local expiry cannot require refetching an authenticated-expired
            // prefix from the peer. The retained checkpoint continues to cover
            // that prefix even though our own inventory generation changes.
            let late = Timestamp(NOW + crate::profile::RETRY_POLICY.horizon_secs + 1);
            bob.execute(Command::Maintain(ClockReading::Trusted(
                ClockInterval::new(late, late).unwrap(),
            )))
            .unwrap();
            bob.with_service(|service, _| {
                assert!(service.record(record)?.is_none());
                Ok(())
            })
            .unwrap();
            assert!(
                matches!(bob.execute(Command::LoadCursor(sender)).unwrap(), Reply::Cursor(found) if found == cursor)
            );
        }
    }
}

#[test]
fn persisted_checkpoint_is_bound_to_control_and_store_and_migrates_old_rows() {
    let fixtures = fixture::generate("Cursor scope", &IDS, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("scope.redb")).unwrap();
    let cfg = fixture::load_and_install(&db, &fixtures[0], IDS[0]).unwrap();
    let local = configuration(&cfg).unwrap().local;
    let peer = cfg.peers[0].endpoint;
    let cursor = completed_cursor(
        peer,
        local,
        cfg.group,
        InventorySnapshot {
            generation: [4; 32],
            head: InventoryCursor(2),
        },
    );
    let mut session = StoreSession::open(&db, cfg.clone()).unwrap();
    session
        .execute(Command::SaveCursor(peer, Some(cursor)))
        .unwrap();
    drop(session);
    let mut session = StoreSession::open(&db, cfg.clone()).unwrap();
    assert!(
        matches!(session.execute(Command::LoadCursor(peer)).unwrap(), Reply::Cursor(Some(found)) if found == cursor)
    );
    // A completed idle poll must not commit a new database version. Compare
    // the actual file bytes while the same exclusive session remains open.
    let path = directory.path().join("scope.redb");
    let before = std::fs::read(&path).unwrap();
    session
        .execute(Command::SaveCursor(peer, Some(cursor)))
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Scope mismatch invalidates the row without allocating another row on each
    // control change. The manager also fences old in-flight commands.
    session.provisioned.control.head.0[0] ^= 1;
    assert!(matches!(
        session.execute(Command::LoadCursor(peer)).unwrap(),
        Reply::Cursor(None)
    ));
    session.provisioned = cfg.clone();
    session.provisioned.control.era.0 .0[0] ^= 1;
    assert!(matches!(
        session.execute(Command::LoadCursor(peer)).unwrap(),
        Reply::Cursor(None)
    ));
    session.provisioned = cfg.clone();
    session.provisioned.incarnation = prns_groups_store::StoreIncarnation::new([8; 24]).unwrap();
    assert!(matches!(
        session.execute(Command::LoadCursor(peer)).unwrap(),
        Reply::Cursor(None)
    ));
    session.provisioned = cfg.clone();
    let mut key = cfg.namespace.0.to_vec();
    key.extend_from_slice(&peer.id.0);
    key.extend_from_slice(&peer.incarnation.get().to_be_bytes());
    let write = db.begin_write().unwrap();
    write
        .open_table(CURSORS)
        .unwrap()
        .insert(key.as_slice(), cursor.to_bytes().as_slice())
        .unwrap();
    write.commit().unwrap();
    assert!(matches!(
        session.execute(Command::LoadCursor(peer)).unwrap(),
        Reply::Cursor(None)
    ));
    session
        .execute(Command::SaveCursor(peer, Some(cursor)))
        .unwrap();
    let read = db.begin_read().unwrap();
    let table = read.open_table(CURSORS).unwrap();
    use redb::ReadableTableMetadata;
    assert_eq!(table.len().unwrap(), 1);
    assert!(table
        .get(key.as_slice())
        .unwrap()
        .unwrap()
        .value()
        .starts_with(CURSOR_CONTEXT_MAGIC));
    drop(table);
    drop(read);
    session.execute(Command::SaveCursor(peer, None)).unwrap();
    let before = std::fs::read(&path).unwrap();
    session.execute(Command::SaveCursor(peer, None)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
