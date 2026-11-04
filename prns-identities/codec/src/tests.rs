#![allow(clippy::unwrap_used, clippy::panic)]

extern crate std;
use super::*;
use std::vec;
use std::vec::Vec;

type State = Catalog<2, 16, 2>;
const INCARNATION: [u8; 16] = [0x53; 16];

fn encoded(catalog: &State) -> Vec<u8> {
    let mut buffer = vec![0; max_encoded_len::<2, 16, 2>().unwrap()];
    let used = encode(catalog, &mut buffer).unwrap();
    buffer.truncate(used);
    buffer
}

struct Disk {
    bytes: Vec<u8>,
    commits: Vec<Vec<u8>>,
}
impl Storage<2, 16, 2> for Disk {
    type Error = Error;
    fn load(&mut self, into: &mut State) -> Result<(), Error> {
        decode(&self.bytes, into)
    }
    fn compare_and_store(&mut self, expected: u64, next: &State) -> Result<(), Error> {
        let mut current = State::new(INCARNATION);
        decode(&self.bytes, &mut current)?;
        assert_eq!(current.revision, expected);
        self.bytes = encoded(next);
        decode(&self.bytes, &mut current)?;
        assert_eq!(current, *next);
        self.commits.push(self.bytes.clone());
        Ok(())
    }
}

fn scenario() -> Disk {
    let mut disk = Disk {
        bytes: encoded(&State::new(INCARNATION)),
        commits: Vec::new(),
    };
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut c = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let command = Command::Create(CreateRequest {
        source: KeySource::Generate,
        metadata: Metadata::new("Weather α", Some("Garden")).unwrap(),
        services: ServiceSet::new(&[ServiceId(2), ServiceId(9)]).unwrap(),
    });
    let token = c
        .reserve(&mut disk, command, CommandDigest([7; 32]))
        .unwrap();
    c.begin(&mut disk, token, &command).unwrap();
    let action = c.action(token).unwrap().unwrap();
    let Action::PublishKey {
        ticket,
        key_reference,
        ..
    } = action
    else {
        panic!("expected publication")
    };
    let id = ticket.identity;
    c.ack_key(
        &mut disk,
        ticket,
        VerifiedKey {
            key_reference,
            public_hash: PublicIdentityHash([8; 16]),
        },
    )
    .unwrap();
    let report = DependencyReport::new(
        &[
            ServiceDependency::clear(ServiceId(2), 1),
            ServiceDependency::clear(ServiceId(9), 1),
        ],
        1,
    )
    .unwrap();

    let enable = Command::Transition(c.prepare(id, TransitionTarget::Enable, report).unwrap());
    let token = c
        .reserve(&mut disk, enable, CommandDigest([2; 32]))
        .unwrap();
    c.begin(&mut disk, token, &enable).unwrap();
    let ticket = c.action(token).unwrap().unwrap().ticket();
    c.activation_failed(&mut disk, ticket, ServiceFailure::Unavailable(0xabcd))
        .unwrap();
    let ticket = c.action(token).unwrap().unwrap().ticket();
    c.ack_quiesced(&mut disk, ticket, report).unwrap();

    let enable = Command::Transition(c.prepare(id, TransitionTarget::Enable, report).unwrap());
    let token = c
        .reserve(&mut disk, enable, CommandDigest([3; 32]))
        .unwrap();
    c.begin(&mut disk, token, &enable).unwrap();
    c.ack_activated(&mut disk, c.action(token).unwrap().unwrap().ticket())
        .unwrap();
    c.reload(&mut disk).unwrap();
    let ticket = c.recovery_action(id).unwrap().unwrap().ticket();
    c.activation_failed(&mut disk, ticket, ServiceFailure::StorageUnavailable)
        .unwrap();
    let ticket = c.recovery_action(id).unwrap().unwrap().ticket();
    c.ack_recovery_quiesced(&mut disk, ticket, report).unwrap();
    let ticket = c.recovery_action(id).unwrap().unwrap().ticket();
    c.ack_activated(&mut disk, ticket).unwrap();

    let retire = c.prepare(id, TransitionTarget::Retire, report).unwrap();
    let token = c
        .apply_stop(&mut disk, retire, CommandDigest([4; 32]))
        .unwrap();
    let mut changed = report;
    changed.entries[0].as_mut().unwrap().revision += 1;
    changed.entries[0].as_mut().unwrap().authority_count = 1;
    c.ack_quiesced(
        &mut disk,
        c.action(token).unwrap().unwrap().ticket(),
        changed,
    )
    .unwrap();
    let retire = c.prepare(id, TransitionTarget::Retire, report).unwrap();
    let token = c
        .apply_stop(&mut disk, retire, CommandDigest([5; 32]))
        .unwrap();
    c.ack_quiesced(
        &mut disk,
        c.action(token).unwrap().unwrap().ticket(),
        report,
    )
    .unwrap();
    assert_eq!(c.identity(id).unwrap().desired, DesiredState::Retired);
    c.prune_completed(&mut disk, token.sequence).unwrap();
    disk
}

#[test]
fn lifecycle_checkpoints_roundtrip_without_struct_layout_or_allocator_requirements() {
    let disk = scenario();
    assert!(disk.commits.len() >= 20);
    for bytes in disk.commits {
        let mut catalog = State::new([0; 16]);
        decode(&bytes, &mut catalog).unwrap();
        assert_eq!(
            encoded(&catalog),
            bytes,
            "canonical encoding survives reopen"
        );
        assert!(bytes.len() <= max_encoded_len::<2, 16, 2>().unwrap());
    }
}

#[test]
fn every_truncated_checkpoint_fails_closed_and_trailing_bytes_are_rejected() {
    let disk = scenario();
    let mut output = State::new(INCARNATION);
    for bytes in disk.commits {
        for cut in 0..bytes.len() {
            assert!(decode(&bytes[..cut], &mut output).is_err(), "cut {cut}");
            assert!(
                validate_catalog(&output).is_err(),
                "partial decode cannot grant authority"
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(decode(&trailing, &mut output), Err(Error::TrailingBytes));
        assert!(validate_catalog(&output).is_err());
        decode(&bytes, &mut output).unwrap();
    }
}

#[test]
fn fixed_header_has_stable_endianness_version_and_capacity_checks() {
    let empty = Catalog::<0, 0, 0>::new([1; 16]);
    let mut bytes = [0u8; 128];
    let n = encode(&empty, &mut bytes).unwrap();
    assert_eq!(n, 72);
    assert_eq!(&bytes[..10], b"PRNSIDC1\x00\x02");
    assert_eq!(&bytes[10..22], &[0; 12]);
    assert_eq!(&bytes[22..24], &[0, 2]);
    assert_eq!(&bytes[24..40], &[1; 16]);
    assert_eq!(&bytes[40..72], &[0; 32]);
    let mut output = Catalog::<0, 0, 0>::new([0; 16]);
    bytes[9] = 1;
    assert_eq!(
        decode(&bytes[..n], &mut output),
        Err(Error::UnsupportedVersion)
    );
    bytes[9] = 2;
    let mut different_capacity = Catalog::<1, 0, 0>::new([0; 16]);
    assert_eq!(
        decode(&bytes[..n], &mut different_capacity),
        Err(Error::CapacityMismatch)
    );
    assert!(max_encoded_len::<{ usize::MAX }, 1, 1>().is_none());
}

#[test]
fn insufficient_output_never_returns_a_usable_encoding() {
    let disk = scenario();
    let mut catalog = State::new(INCARNATION);
    decode(&disk.commits[5], &mut catalog).unwrap();
    let expected = encoded(&catalog);
    let mut bytes = vec![0; expected.len()];
    for length in 0..expected.len() {
        assert_eq!(
            encode(&catalog, &mut bytes[..length]),
            Err(Error::BufferTooSmall)
        );
    }
    assert_eq!(encode(&catalog, &mut bytes).unwrap(), expected.len());
    assert_eq!(bytes, expected);
}

#[test]
fn untrusted_tags_text_and_invalid_catalogs_are_rejected() {
    let mut bool_reader = Reader { bytes: &[2], at: 0 };
    assert_eq!(bool::decode(&mut bool_reader), Err(Error::InvalidEncoding));
    let mut option_reader = Reader { bytes: &[2], at: 0 };
    assert_eq!(
        Option::<u64>::decode(&mut option_reader),
        Err(Error::InvalidEncoding)
    );
    for bytes in [
        &[0, 1, 0xff][..],
        &[0, 1, b'\n'][..],
        &[0, 3, b'a', b'b', b'c'][..],
    ] {
        assert_eq!(
            BoundedText::<2>::decode(&mut Reader { bytes, at: 0 }),
            Err(Error::InvalidEncoding)
        );
    }
    let mut bad = State::new(INCARNATION);
    bad.issued_sequence = 2;
    assert_eq!(
        encode(&bad, &mut [0; 100]),
        Err(Error::InvalidCatalog(Fault::InvalidCatalog))
    );
    let mut bytes = encoded(&State::new(INCARNATION));
    // Catalog version is independent of framing version.
    bytes[23] = 99;
    assert_eq!(
        decode(&bytes, &mut bad),
        Err(Error::InvalidCatalog(Fault::InvalidCatalog))
    );
    assert_eq!(bad.format, 0);
}

fn roundtrip<T: Codec + core::fmt::Debug + PartialEq>(value: T) {
    let mut buffer = [0; 512];
    let mut w = Writer {
        bytes: &mut buffer,
        at: 0,
    };
    value.encode(&mut w).unwrap();
    let end = w.at;
    let mut r = Reader {
        bytes: &buffer[..end],
        at: 0,
    };
    assert_eq!(T::decode(&mut r).unwrap(), value);
    assert_eq!(r.at, end);
    let mut bad = Reader {
        bytes: &[255],
        at: 0,
    };
    assert!(T::decode(&mut bad).is_err());
}

#[test]
fn all_retained_results_and_cleanup_reasons_have_stable_distinct_encodings() {
    let identity = LocalIdentityId {
        incarnation: INCARNATION,
        sequence: u64::MAX,
    };
    for value in [
        OperationResult::Created { identity },
        OperationResult::Existing { identity },
        OperationResult::Enabled { identity },
        OperationResult::Paused { identity },
        OperationResult::Retired { identity },
        OperationResult::ServiceAdded {
            identity,
            service: ServiceId(u32::MAX),
        },
        OperationResult::ServiceRemoved {
            identity,
            service: ServiceId(u32::MAX),
        },
        OperationResult::Cancelled,
    ] {
        roundtrip(value);
    }
    for reason in [
        RetirementBlock::ChangedDependencies,
        RetirementBlock::IncompleteReport,
        RetirementBlock::UnqualifiedService,
        RetirementBlock::RecoveryPending,
        RetirementBlock::AuthorityOwned,
        RetirementBlock::UnsettledControl,
        RetirementBlock::PendingMessages,
    ] {
        roundtrip(OperationResult::RetirementBlocked { identity, reason });
        roundtrip(OperationResult::ServiceRemovalBlocked {
            identity,
            service: ServiceId(u32::MAX),
            reason,
        });
    }
    roundtrip(TransitionTarget::AddService(ServiceId(u32::MAX)));
    roundtrip(TransitionTarget::RemoveService(ServiceId(u32::MAX)));
    roundtrip(TransitionPhase::InitializingService);
    for reason in [
        ServiceFailure::CapacityExceeded,
        ServiceFailure::StorageUnavailable,
        ServiceFailure::Unavailable(u16::MAX),
    ] {
        roundtrip(OperationResult::ActivationFailed { identity, reason });
        roundtrip(TransitionPhase::ActivationCleanup(reason));
    }
    roundtrip(KeySource::Import {
        expected_hash: PublicIdentityHash([3; 16]),
    });
}

#[test]
fn maximum_names_and_duplicate_import_key_obligation_survive_encoding() {
    let mut disk = Disk {
        bytes: encoded(&State::new(INCARNATION)),
        commits: Vec::new(),
    };
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut c = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let hash = PublicIdentityHash([0xab; 16]);
    let metadata = Metadata::new(&"é".repeat(32), Some(&"é".repeat(64))).unwrap();
    let mut actions = Vec::new();
    for (source, tag) in [
        (KeySource::Generate, 1),
        (
            KeySource::Import {
                expected_hash: hash,
            },
            2,
        ),
    ] {
        let command = Command::Create(CreateRequest {
            source,
            metadata,
            services: ServiceSet::new(&[ServiceId(2), ServiceId(9)]).unwrap(),
        });
        let token = c
            .reserve(&mut disk, command, CommandDigest([tag; 32]))
            .unwrap();
        c.begin(&mut disk, token, &command).unwrap();
        actions.push((token, c.action(token).unwrap().unwrap()));
    }
    for (_, action) in &actions {
        let Action::PublishKey {
            ticket,
            key_reference,
            ..
        } = *action
        else {
            panic!("key publication")
        };
        c.ack_key(
            &mut disk,
            ticket,
            VerifiedKey {
                key_reference,
                public_hash: hash,
            },
        )
        .unwrap();
    }
    let duplicate = actions[1].0;
    assert!(c.operation(duplicate).unwrap().unpublished_key.is_some());
    let bytes = encoded(c.snapshot());
    let mut restored = State::new([0; 16]);
    decode(&bytes, &mut restored).unwrap();
    assert_eq!(restored, *c.snapshot());
    assert_eq!(restored.identities().next().unwrap().metadata, metadata);
    assert_eq!(
        c.prune_completed(&mut disk, duplicate.sequence),
        Err(prns_identities_core::Error::Fault(Fault::UnaccountedKey))
    );
}

#[test]
fn standalone_creation_and_result_encoding_reuse_checked_catalog_values() {
    let request = CreateRequest {
        source: KeySource::Import {
            expected_hash: PublicIdentityHash([5; 16]),
        },
        metadata: Metadata::new("Weather", Some("Station")).unwrap(),
        services: ServiceSet::<2>::new(&[ServiceId(17)]).unwrap(),
    };
    let mut bytes = vec![0; max_create_request_encoded_len::<2>().unwrap()];
    let used = encode_create_request(&request, &mut bytes).unwrap();
    assert_eq!(decode_create_request::<2>(&bytes[..used]), Ok(request));
    for length in 0..used {
        assert!(decode_create_request::<2>(&bytes[..length]).is_err());
    }
    assert_eq!(
        decode_create_request::<2>(&bytes[..used + 1]),
        Err(Error::TrailingBytes)
    );
    let mut invalid = request;
    invalid.services.entries = [Some(ServiceId(17)); 2];
    assert!(encode_create_request(&invalid, &mut bytes).is_err());
    let result = OperationResult::Created {
        identity: LocalIdentityId {
            incarnation: INCARNATION,
            sequence: 7,
        },
    };
    let used = encode_operation_result(&result, &mut bytes).unwrap();
    assert_eq!(decode_operation_result(&bytes[..used]), Ok(result));
    assert_eq!(
        decode_operation_result(&bytes[..used + 1]),
        Err(Error::TrailingBytes)
    );
}
