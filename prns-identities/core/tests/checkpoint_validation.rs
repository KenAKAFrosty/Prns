#![allow(clippy::unwrap_used, clippy::panic)]

use prns_identities_core::*;

type State = Catalog<3, 8, 2>;
const INCARNATION: [u8; 16] = [0x92; 16];

struct Disk {
    saved: State,
    writes: usize,
}

impl Storage<3, 8, 2> for Disk {
    type Error = ();

    fn load(&mut self, into: &mut State) -> Result<(), ()> {
        into.copy_from(&self.saved);
        Ok(())
    }

    fn compare_and_store(&mut self, expected: u64, next: &State) -> Result<(), ()> {
        if self.saved.revision != expected {
            return Err(());
        }
        self.writes += 1;
        self.saved.copy_from(next);
        Ok(())
    }
}

fn populated() -> State {
    let mut disk = Disk {
        saved: State::new(INCARNATION),
        writes: 0,
    };
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    for (label, byte) in [("Weather", 1), ("Irrigation", 2)] {
        let command = Command::Create(CreateRequest {
            source: KeySource::Generate,
            metadata: Metadata::new(label, None).unwrap(),
            services: ServiceSet::new(&[ServiceId(7)]).unwrap(),
        });
        let token = core
            .reserve(&mut disk, command, CommandDigest([byte; 32]))
            .unwrap();
        core.begin(&mut disk, token, &command).unwrap();
        let action = core.action(token).unwrap().unwrap();
        let Action::PublishKey { key_reference, .. } = action else {
            panic!("creation must request a key");
        };
        core.ack_key(
            &mut disk,
            action.ticket(),
            VerifiedKey {
                key_reference,
                public_hash: PublicIdentityHash([byte; 16]),
            },
        )
        .unwrap();
    }
    disk.saved
}

#[test]
fn malformed_checkpoints_never_gain_authority_or_overwrite_evidence() {
    let valid = populated();
    let corruptions: &[fn(&mut State)] = &[
        |s| s.format += 1,
        |s| s.completed_floor = s.issued_sequence + 1,
        |s| s.identities[1].as_mut().unwrap().id = s.identities[0].unwrap().id,
        |s| s.identities[1].as_mut().unwrap().public_hash = s.identities[0].unwrap().public_hash,
        |s| {
            s.identities[1].as_mut().unwrap().key_reference = s.identities[0].unwrap().key_reference
        },
        |s| s.identities[0].as_mut().unwrap().id.incarnation = [0x11; 16],
        |s| s.identities[0].as_mut().unwrap().id.sequence = 0,
        |s| s.identities[0].as_mut().unwrap().metadata.local_label = BoundedText::new("").unwrap(),
        |s| s.identities[0].as_mut().unwrap().services.entries = [Some(ServiceId(7)); 2],
        |s| s.operations[1].as_mut().unwrap().token = s.operations[0].unwrap().token,
        |s| s.operations[0].as_mut().unwrap().token.incarnation = [0x22; 16],
        |s| s.operations[0].as_mut().unwrap().token.sequence = s.issued_sequence + 1,
        |s| s.operations[0] = None,
        |s| {
            let Command::Create(ref mut create) = s.operations[0].as_mut().unwrap().command else {
                panic!("fixture operation must create an identity");
            };
            create.source = KeySource::Import {
                expected_hash: PublicIdentityHash([0xef; 16]),
            };
        },
        |s| {
            let token = s.operations[0].unwrap().token;
            s.identities[0].as_mut().unwrap().pending = Some(PendingTransition {
                operation: token,
                target: TransitionTarget::Retire,
                phase: TransitionPhase::Quiescing,
            });
        },
    ];
    for (index, corrupt) in corruptions.iter().enumerate() {
        let mut saved = valid.clone();
        corrupt(&mut saved);
        let expected = saved.clone();
        let mut disk = Disk { saved, writes: 0 };
        let mut working = State::new(INCARNATION);
        let mut staging = State::new(INCARNATION);
        let result = Coordinator::open(&mut disk, &mut working, &mut staging);
        assert!(
            matches!(result, Err(Error::Fault(Fault::InvalidCatalog))),
            "corruption {index} was not rejected"
        );
        assert_eq!(disk.writes, 0, "must validate before a session write");
        assert_eq!(disk.saved, expected, "must retain malformed evidence");
    }
}

#[test]
fn opening_at_counter_exhaustion_does_not_wrap_or_write() {
    for exhaust_session in [true, false] {
        let mut saved = State::new(INCARNATION);
        saved.revision = u64::MAX;
        saved.session = if exhaust_session { u64::MAX } else { 1 };
        let expected = saved.clone();
        let mut disk = Disk { saved, writes: 0 };
        let mut working = State::new(INCARNATION);
        let mut staging = State::new(INCARNATION);
        let result = Coordinator::open(&mut disk, &mut working, &mut staging);
        assert!(matches!(result, Err(Error::Fault(Fault::Exhausted))));
        assert_eq!(disk.writes, 0);
        assert_eq!(disk.saved, expected);
    }
}

#[test]
fn retained_retirement_evidence_cannot_restore_an_enabled_or_older_identity() {
    let mut disk = Disk {
        saved: populated(),
        writes: 0,
    };
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let id = core.snapshot().identities().next().unwrap().id;
    let report = DependencyReport::new(&[ServiceDependency::clear(ServiceId(7), 1)], 0).unwrap();
    let prepared = core.prepare(id, TransitionTarget::Retire, report).unwrap();
    let command = Command::Transition(prepared);
    let token = core
        .reserve(&mut disk, command, CommandDigest([0x44; 32]))
        .unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    let ticket = core.action(token).unwrap().unwrap().ticket();
    core.ack_quiesced(&mut disk, ticket, report).unwrap();
    let valid = disk.saved.clone();
    assert_eq!(validate_catalog(&valid), Ok(()));

    for corruption in 0..3 {
        disk.saved.copy_from(&valid);
        let record = disk.saved.identities[0].as_mut().unwrap();
        match corruption {
            0 => record.desired = DesiredState::Enabled,
            1 => record.lifecycle_revision = prepared.identity.lifecycle_revision,
            _ => record.services = ServiceSet::new(&[ServiceId(8)]).unwrap(),
        }
        disk.writes = 0;
        assert!(matches!(
            Coordinator::open(&mut disk, &mut working, &mut staging),
            Err(Error::Fault(Fault::InvalidCatalog))
        ));
        assert_eq!(disk.writes, 0);
    }
}
