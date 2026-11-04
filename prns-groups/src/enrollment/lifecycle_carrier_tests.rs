use super::*;

#[test]
fn old_leave_terminal_and_outbox_ack_share_one_carrier_commit_after_delete_and_rejoin() {
    for cut in [ProtocolCut::BeforeCommit, ProtocolCut::AfterEffect] {
        let dir = tempfile::tempdir().unwrap();
        let dbs = ["a", "b"].map(|name| Database::create(dir.path().join(name)).unwrap());
        let (work, response, identity, group, new_member) = {
            let (mut a, mut b) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
            let group = a
                .engine
                .create(&mut a.owner, request(1), "old Leave carrier", NOW)
                .unwrap();
            crate::enrollment::tests::management_tests::enroll(&mut a, &mut b, group, 2);
            b.engine
                .leave(&mut b.owner, request(10), group, NOW)
                .unwrap();
            let work = b.work("leave");
            b.engine
                .delete_local(&mut b.owner, request(11), group, NOW)
                .unwrap();
            let response = a.receive(b.identity, &work, NOW);
            // The old reply is delayed while a new, independently admitted
            // membership becomes current. Its exact outbox must still finish.
            crate::enrollment::tests::management_tests::enroll(&mut a, &mut b, group, 20);
            let new_member = b.descriptor(group).identity.binding();
            (work, response, b.identity, group, new_member)
        };
        let mut attached = attachment(&dbs[1], identity);
        let mut gate = Gate::default();
        let command = || Command::EnrollmentComplete {
            id: work.id,
            response: response.clone(),
            now: NOW,
        };
        {
            let mut manager = GroupManager::open_attached(&dbs[1], &mut attached).unwrap();
            protocol_fault::set(cut);
            assert!(matches!(
                execute_protocol(&mut manager, &mut gate, command()),
                Err(Error::Unavailable)
            ));
        }
        recover(&dbs[1], &mut attached, &mut gate, cut);
        {
            let mut manager = GroupManager::open_attached(&dbs[1], &mut attached).unwrap();
            if cut == ProtocolCut::BeforeCommit {
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
        }
        let mut b = Device::new(&dbs[1], 2);
        assert_eq!(b.engine.read().unwrap().header.counts.departures, 0);
        let current = b.descriptor(group);
        assert_eq!(current.lifecycle, GroupLifecycle::Active);
        assert_eq!(current.identity.binding(), new_member);
    }
}
