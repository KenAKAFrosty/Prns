use super::*;
use crate::enrollment::attempt::fault::{self as protocol_fault, Cut as ProtocolCut};
#[path = "lifecycle_carrier_tests.rs"]
mod lifecycle_carrier_tests;

fn execute_protocol(
    manager: &mut GroupManager<'_>,
    gate: &mut Gate,
    command: Command,
) -> Result<NativeReply, Error> {
    let (reply, result) = oneshot::channel();
    manager.execute_authorized(
        Job {
            _input: None,
            command,
            group: None,
            control: None,
            reply,
            wake: Arc::new(Notify::new()),
        },
        gate,
    );
    result.blocking_recv().unwrap()
}
fn attachment(database: &Database, identity: IdentityHash) -> Attachment {
    let tx = database.begin_write().unwrap();
    prns_groups_store_redb::change::initialize_in(&tx, StoreIncarnation::new([72; 24]).unwrap())
        .unwrap();
    tx.commit().unwrap();
    Attachment::open(database, identity).unwrap()
}
fn recover(database: &Database, attachment: &mut Attachment, gate: &mut Gate, cut: ProtocolCut) {
    assert_eq!(attachment.pending_carrier(), gate.pending);
    assert!(gate.pending.is_some());
    gate.paused = true;
    assert!(attachment.recover_carrier(database, gate).unwrap());
    assert_eq!(
        gate.settled,
        vec![match cut {
            ProtocolCut::BeforeCommit => Settlement::Rejected,
            ProtocolCut::AfterEffect => Settlement::Completed,
        }]
    );
    assert!(gate.pending.is_none());
    assert!(attachment.pending_carrier().is_none());
}

#[test]
fn catalog_offer_cut_recovers_exact_bytes_and_duplicate_does_not_write() {
    for cut in [ProtocolCut::BeforeCommit, ProtocolCut::AfterEffect] {
        let directory = tempfile::tempdir().unwrap();
        let paths = [directory.path().join("a"), directory.path().join("b")];
        let dbs = paths.each_ref().map(|path| Database::create(path).unwrap());
        let (work, identity) = {
            let mut a = Device::new(&dbs[0], 1);
            let mut b = Device::new(&dbs[1], 2);
            let group = a
                .engine
                .create(&mut a.owner, request(1), "catalog cut", NOW)
                .unwrap();
            a.engine
                .invite(&mut a.owner, request(2), group, b.identity, NOW)
                .unwrap();
            b.engine.maintain(&mut b.owner, NOW).unwrap();
            (a.work("offer"), b.identity)
        };
        let mut attachment = attachment(&dbs[1], identity);
        let mut gate = Gate::default();
        let command = || Command::EnrollmentHandle {
            requester: IdentityHash::new([1; 16]),
            frame: work.frame.clone(),
            now: NOW,
        };
        {
            let mut manager = GroupManager::open_attached(&dbs[1], &mut attachment).unwrap();
            protocol_fault::set(cut);
            assert!(matches!(
                execute_protocol(&mut manager, &mut gate, command()),
                Err(Error::Unavailable)
            ));
            assert!(matches!(
                execute_protocol(
                    &mut manager,
                    &mut gate,
                    Command::EnrollmentWork {
                        now: NOW,
                        after: None,
                        limit: crate::PageLimits {
                            max_rows: 2,
                            max_bytes: 2 * 1024 * 1024
                        }
                    }
                ),
                Err(Error::Busy)
            ));
            assert!(matches!(
                execute_protocol(&mut manager, &mut gate, Command::Config),
                Err(Error::Busy)
            ));
        }
        recover(&dbs[1], &mut attachment, &mut gate, cut);
        let catalog = Enrollment::open_existing(&dbs[1], identity)
            .unwrap()
            .read()
            .unwrap();
        assert_eq!(
            catalog.header.counts.invitations,
            u64::from(cut == ProtocolCut::AfterEffect)
        );
        let mut manager = GroupManager::open_attached(&dbs[1], &mut attachment).unwrap();
        assert!(matches!(
            execute_protocol(&mut manager, &mut gate, command()),
            Err(Error::Busy)
        ));
        gate.paused = false;
        assert!(matches!(
            execute_protocol(&mut manager, &mut gate, command()),
            Ok(NativeReply::EnrollmentFrame(_))
        ));
        let before = hash(&std::fs::read(&paths[1]).unwrap());
        assert!(matches!(
            execute_protocol(&mut manager, &mut gate, command()),
            Ok(NativeReply::EnrollmentFrame(_))
        ));
        assert_eq!(hash(&std::fs::read(&paths[1]).unwrap()), before);
    }
}

#[test]
fn correlated_welcome_cut_uses_one_namespace_ticket_and_duplicate_is_idle() {
    for cut in [ProtocolCut::BeforeCommit, ProtocolCut::AfterEffect] {
        let directory = tempfile::tempdir().unwrap();
        let dbs = ["a", "b"].map(|name| Database::create(directory.path().join(name)).unwrap());
        let (work, response, identity, group) = {
            let (mut a, mut b) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
            let group = a
                .engine
                .create(&mut a.owner, request(1), "welcome cut", NOW)
                .unwrap();
            let invitation = offer(&mut a, &mut b, group, 2);
            b.engine
                .respond(&mut b.owner, request(3), invitation, true, NOW)
                .unwrap();
            let work = b.work("join");
            let response = a.receive(b.identity, &work, NOW);
            (work, response, b.identity, group)
        };
        let mut attachment = attachment(&dbs[1], identity);
        let mut gate = Gate::default();
        let command = || Command::EnrollmentComplete {
            id: work.id,
            response: response.clone(),
            now: NOW,
        };
        {
            let mut manager = GroupManager::open_attached(&dbs[1], &mut attachment).unwrap();
            protocol_fault::set(cut);
            assert!(matches!(
                execute_protocol(&mut manager, &mut gate, command()),
                Err(Error::Unavailable)
            ));
        }
        recover(&dbs[1], &mut attachment, &mut gate, cut);
        let mut manager = GroupManager::open_attached(&dbs[1], &mut attachment).unwrap();
        if cut == ProtocolCut::BeforeCommit {
            assert!(matches!(
                execute_protocol(&mut manager, &mut gate, command()),
                Err(Error::Busy)
            ));
            gate.paused = false;
            assert!(matches!(
                execute_protocol(&mut manager, &mut gate, command()),
                Ok(NativeReply::EnrollmentCompleted(
                    CompletionActivity::Progress
                ))
            ));
        }
        gate.paused = true;
        let admitted = gate.admitted;
        assert!(matches!(
            execute_protocol(&mut manager, &mut gate, command()),
            Ok(NativeReply::EnrollmentCompleted(CompletionActivity::Idle))
        ));
        assert_eq!(gate.admitted, admitted);
        assert!(manager.group_scope(group).is_ok());
    }
}

#[test]
fn terminal_retirement_and_catalog_projection_share_the_exact_commit() {
    for cut in [ProtocolCut::BeforeCommit, ProtocolCut::AfterEffect] {
        let directory = tempfile::tempdir().unwrap();
        let dbs = ["a", "b"].map(|name| Database::create(directory.path().join(name)).unwrap());
        let (cfg, _) = enrolled_wire(&dbs);
        let work = {
            let mut a = Device::new(&dbs[0], 1);
            let head = a.descriptor(cfg.group).control.head.0;
            a.engine
                .close(&mut a.owner, request(8), cfg.group, head, NOW)
                .unwrap();
            // Close admits funded work; the owning scheduler visits terminal
            // fanout in bounded phases before the carrier attempts delivery.
            for _ in 0..6 {
                a.engine.maintain(&mut a.owner, NOW).unwrap();
            }
            a.work("terminal")
        };
        let mut attachment = attachment(&dbs[1], cfg.local_reticulum_identity);
        let mut gate = Gate::default();
        let command = || Command::EnrollmentHandle {
            requester: IdentityHash::new([1; 16]),
            frame: work.frame.clone(),
            now: NOW,
        };
        {
            let mut manager = GroupManager::open_attached(&dbs[1], &mut attachment).unwrap();
            protocol_fault::set(cut);
            assert!(matches!(
                execute_protocol(&mut manager, &mut gate, command()),
                Err(Error::Unavailable)
            ));
        }
        recover(&dbs[1], &mut attachment, &mut gate, cut);
        {
            let mut b = Device::new(&dbs[1], 2);
            let catalog = b.engine.read().unwrap();
            let entry = Enrollment::find(&catalog, cfg.group.0).unwrap();
            let closed = b
                .engine
                .adapter(&mut b.owner, &entry, None, |_, snapshot| {
                    Ok(snapshot.lifecycle == crate::GroupLifecycle::Closed)
                })
                .unwrap();
            assert_eq!(closed, cut == ProtocolCut::AfterEffect);
            assert_eq!(catalog.header.counts.terminals != 0, closed);
        }
        gate.paused = false;
        let mut manager = GroupManager::open_attached(&dbs[1], &mut attachment).unwrap();
        assert!(matches!(
            execute_protocol(&mut manager, &mut gate, command()),
            Ok(NativeReply::EnrollmentFrame(_))
        ));
    }
}
