#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use prns_groups_mls::{
    Adapter, Admission, Error, Event, Limit, LocalIdentity, MemberBinding, Policy, ProviderError,
};
use prns_groups_store::{
    AtomicBackend, CommitOutcome, Coordinator, Mutation, OperationId, RecoveryOutcome, Revision,
    StoreError,
};
use support::durable::{Cut, Directory, FileBackend};
use support::{limits, store_limits, Endpoint, Enrollment, Trio, GROUP, NOW};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CUTS: [Cut; 4] = [
    Cut::BeforeSnapshot,
    Cut::DuringSnapshot,
    Cut::AfterSnapshot,
    Cut::AfterPublish,
];

fn operation(value: u8) -> OperationId {
    OperationId([value; 32])
}

#[test]
fn file_fixture_reopens_complete_images_after_torn_and_uncertain_publication() -> TestResult {
    for cut in CUTS {
        let directory = Directory::new()?;
        let mut backend = FileBackend::open(directory.path())?;
        assert!(matches!(
            backend.commit(
                Revision(0),
                operation(1),
                &[Mutation::Put {
                    key: b"old".to_vec(),
                    value: b"old state".to_vec(),
                }],
            )?,
            CommitOutcome::Committed(_)
        ));
        backend.faults().arm(cut);
        assert!(matches!(
            backend.commit(
                Revision(1),
                operation(2),
                &[
                    Mutation::Delete {
                        key: b"old".to_vec(),
                    },
                    Mutation::Put {
                        key: b"new".to_vec(),
                        value: b"new state".to_vec(),
                    },
                ],
            )?,
            CommitOutcome::Uncertain
        ));
        drop(backend);
        let mut reopened = FileBackend::open(directory.path())?;
        if matches!(cut, Cut::AfterPublish) {
            assert!(matches!(
                reopened.recover(operation(2))?,
                RecoveryOutcome::Committed(_)
            ));
            assert_eq!(reopened.read(b"old", 100)?, None);
            assert_eq!(reopened.read(b"new", 100)?, Some(b"new state".to_vec()));
        } else {
            assert!(matches!(
                reopened.recover(operation(2))?,
                RecoveryOutcome::NotCommitted {
                    revision: Revision(1)
                }
            ));
            assert_eq!(reopened.read(b"old", 100)?, Some(b"old state".to_vec()));
            assert_eq!(reopened.read(b"new", 100)?, None);
        }
    }
    Ok(())
}

#[test]
fn file_fixture_enforces_the_callers_value_read_bound() -> TestResult {
    let directory = Directory::new()?;
    let mut backend = FileBackend::open(directory.path())?;
    backend.commit(
        Revision(0),
        operation(1),
        &[Mutation::Put {
            key: b"large".to_vec(),
            value: vec![42; 1024],
        }],
    )?;
    drop(backend);
    let mut reopened = FileBackend::open(directory.path())?;
    assert!(reopened.read(b"large", 1023).is_err());
    assert_eq!(reopened.read(b"large", 1024)?, Some(vec![42; 1024]));
    Ok(())
}

#[test]
fn key_package_advertisement_and_private_material_recover_together() -> TestResult {
    for cut in CUTS {
        let owner_identity = LocalIdentity::generate([1; 32], 1)?;
        let member_identity = LocalIdentity::generate([2; 32], 1)?;
        let policy = Policy::new(
            owner_identity.binding(),
            vec![owner_identity.binding(), member_identity.binding()],
        )?;
        let owner = Endpoint::new(owner_identity, policy.clone(), limits())?;
        let mut member = Endpoint::new(member_identity, policy, limits())?;
        member.faults.arm(cut);
        assert!(member.adapter().key_package(operation(1), NOW).is_err());
        member.reopen()?;
        let recovered = member.adapter().coordinator().recover(operation(1))?;
        let package = if matches!(cut, Cut::AfterPublish) {
            assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
            member.adapter().retry(operation(1))?.unwrap()
        } else {
            assert!(matches!(recovered, RecoveryOutcome::NotCommitted { .. }));
            assert!(member.adapter().retry(operation(1))?.is_none());
            member.adapter().key_package(operation(1), NOW)?.output
        };
        owner.adapter().create(operation(1), GROUP)?;
        let added = owner
            .adapter()
            .add(operation(2), GROUP, &[&package.messages[0]], NOW)?;
        let joined = member
            .adapter()
            .join(operation(2), GROUP, &added.output.messages[1], NOW)?;
        assert!(matches!(joined.output.event, Event::Joined));
    }
    Ok(())
}

#[test]
fn join_consumes_key_package_in_the_same_recoverable_commit_as_group_state() -> TestResult {
    for cut in CUTS {
        let mut peers = Enrollment::new(limits())?;
        let before = peers.member.bytes()?;
        let package_ids = FileBackend::open(peers.member.directory.path())?
            .read(b"key-packages", 256)?
            .unwrap();
        assert_eq!(package_ids.len(), 32);
        let mut package_key = vec![b'K'];
        package_key.extend_from_slice(&package_ids);
        peers.member.faults.arm(cut);
        assert!(peers
            .member
            .adapter()
            .join(operation(2), GROUP, &peers.welcome, NOW)
            .is_err());
        peers.member.reopen()?;
        let recovered = peers.member.adapter().coordinator().recover(operation(2))?;
        if matches!(cut, Cut::AfterPublish) {
            assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
            let retry = peers.member.adapter().retry(operation(2))?.unwrap();
            assert!(matches!(retry.event, Event::Joined));
            assert!(peers
                .member
                .adapter()
                .join(operation(3), GROUP, &peers.welcome, NOW)
                .is_err());
        } else {
            assert!(matches!(recovered, RecoveryOutcome::NotCommitted { .. }));
            assert_eq!(peers.member.bytes()?, before);
            assert!(peers.member.adapter().retry(operation(2))?.is_none());
            peers
                .member
                .adapter()
                .join(operation(2), GROUP, &peers.welcome, NOW)?;
        }
        let sent = peers
            .owner
            .adapter()
            .send(operation(3), GROUP, b"after join")?;
        let received = peers.member.adapter().receive(
            operation(4),
            GROUP,
            &sent.output.messages[0],
            |_| Admission::Accept,
        )?;
        assert!(
            matches!(received.output.event, Event::Application(ref body) if body == b"after join")
        );
        let mut storage = FileBackend::open(peers.member.directory.path())?;
        assert_eq!(storage.read(b"key-packages", 256)?, Some(Vec::new()));
        assert_eq!(storage.read(&package_key, 65_536)?, None);
    }
    Ok(())
}

#[test]
fn uncertain_send_recovery_returns_exact_ciphertext_and_rejects_operation_reuse() -> TestResult {
    for cut in CUTS {
        let mut peers = Enrollment::joined(limits())?;
        let before = peers.owner.bytes()?;
        peers.owner.faults.arm(cut);
        assert!(peers
            .owner
            .adapter()
            .send(operation(3), GROUP, b"durable outbox")
            .is_err());
        assert!(peers.owner.adapter().coordinator().head().is_err());
        peers.owner.reopen()?;
        let recovered = peers.owner.adapter().coordinator().recover(operation(3))?;
        let output = if matches!(cut, Cut::AfterPublish) {
            assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
            peers.owner.adapter().retry(operation(3))?.unwrap()
        } else {
            assert!(matches!(recovered, RecoveryOutcome::NotCommitted { .. }));
            assert_eq!(peers.owner.bytes()?, before);
            assert!(peers.owner.adapter().retry(operation(3))?.is_none());
            peers
                .owner
                .adapter()
                .send(operation(3), GROUP, b"durable outbox")?
                .output
        };
        peers.owner.reopen()?;
        let retry = peers.owner.adapter().retry(operation(3))?.unwrap();
        assert_eq!(retry.messages, output.messages);
        let committed = peers.owner.bytes()?;
        assert!(peers
            .owner
            .adapter()
            .send(operation(3), GROUP, b"different content")
            .is_err());
        assert_eq!(peers.owner.bytes()?, committed);
        let received =
            peers
                .member
                .adapter()
                .receive(operation(3), GROUP, &retry.messages[0], |_| {
                    Admission::Accept
                })?;
        assert!(
            matches!(received.output.event, Event::Application(ref body) if body == b"durable outbox")
        );
        let next = peers
            .owner
            .adapter()
            .send(operation(4), GROUP, b"next generation")?;
        assert_ne!(next.output.messages, retry.messages);
        peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &next.output.messages[0], |_| {
                Admission::Accept
            })?;
    }
    Ok(())
}

#[test]
fn receive_and_consumed_ratchet_recover_as_one_result() -> TestResult {
    for cut in CUTS {
        let mut peers = Enrollment::joined(limits())?;
        let sent = peers
            .owner
            .adapter()
            .send(operation(3), GROUP, b"admitted")?;
        let wire = &sent.output.messages[0];
        let before = peers.member.bytes()?;
        peers.member.faults.arm(cut);
        assert!(peers
            .member
            .adapter()
            .receive(operation(3), GROUP, wire, |_| Admission::Accept)
            .is_err());
        peers.member.reopen()?;
        let recovered = peers.member.adapter().coordinator().recover(operation(3))?;
        if matches!(cut, Cut::AfterPublish) {
            assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
            let retry = peers.member.adapter().retry(operation(3))?.unwrap();
            assert!(matches!(retry.event, Event::Application(ref body) if body == b"admitted"));
        } else {
            assert!(matches!(recovered, RecoveryOutcome::NotCommitted { .. }));
            assert_eq!(peers.member.bytes()?, before);
            assert!(peers.member.adapter().retry(operation(3))?.is_none());
            peers
                .member
                .adapter()
                .receive(operation(3), GROUP, wire, |_| Admission::Accept)?;
        }
        peers.member.reopen()?;
        assert!(peers
            .member
            .adapter()
            .receive(operation(4), GROUP, wire, |_| Admission::Accept)
            .is_err());
    }
    Ok(())
}

#[test]
fn invalid_aead_discards_mutated_attempt_and_original_ciphertext_still_receives() -> TestResult {
    let mut peers = Enrollment::joined(limits())?;
    let sent = peers
        .owner
        .adapter()
        .send(operation(3), GROUP, b"authentic")?;
    let mut damaged = sent.output.messages[0].clone();
    *damaged.last_mut().unwrap() ^= 1;
    let before = peers.member.bytes()?;
    assert!(peers
        .member
        .adapter()
        .receive(operation(3), GROUP, &damaged, |_| {
            panic!("unauthenticated data reached application admission")
        })
        .is_err());
    assert_eq!(peers.member.bytes()?, before);
    peers.member.reopen()?;
    assert!(peers.member.adapter().retry(operation(3))?.is_none());
    let received =
        peers
            .member
            .adapter()
            .receive(operation(3), GROUP, &sent.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(received.output.event, Event::Application(ref body) if body == b"authentic"));
    assert_eq!(received.output.sender, Some(peers.owner.identity.binding()));
    Ok(())
}

#[test]
fn authenticated_rejection_is_durable_and_does_not_resurrect_on_replay() -> TestResult {
    for cut in CUTS {
        let mut peers = Enrollment::joined(limits())?;
        let sent = peers
            .owner
            .adapter()
            .send(operation(3), GROUP, b"forbidden command")?;
        let before = peers.member.bytes()?;
        peers.member.faults.arm(cut);
        assert!(peers
            .member
            .adapter()
            .receive_with(
                operation(3),
                GROUP,
                &sent.output.messages[0],
                |sender, body| {
                    assert_eq!(sender, &peers.owner.identity.binding());
                    assert_eq!(body, b"forbidden command");
                    Admission::Reject(41)
                },
            )
            .is_err());
        peers.member.reopen()?;
        let recovered = peers.member.adapter().coordinator().recover(operation(3))?;
        if matches!(cut, Cut::AfterPublish) {
            assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
        } else {
            assert!(matches!(recovered, RecoveryOutcome::NotCommitted { .. }));
            assert_eq!(peers.member.bytes()?, before);
            assert!(peers.member.adapter().retry(operation(3))?.is_none());
            let rejected = peers.member.adapter().receive_with(
                operation(3),
                GROUP,
                &sent.output.messages[0],
                |sender, body| {
                    assert_eq!(sender, &peers.owner.identity.binding());
                    assert_eq!(body, b"forbidden command");
                    Admission::Reject(41)
                },
            )?;
            assert!(matches!(rejected.output.event, Event::Rejected(41)));
        }
        peers.member.reopen()?;
        let retry = peers.member.adapter().retry(operation(3))?.unwrap();
        assert!(matches!(retry.event, Event::Rejected(41)));
        assert_eq!(retry.sender, Some(peers.owner.identity.binding()));
        let committed = peers.member.bytes()?;
        assert!(peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &sent.output.messages[0], |_| {
                panic!("rejected generation was accepted a second time")
            },)
            .is_err());
        assert_eq!(peers.member.bytes()?, committed);
    }
    Ok(())
}

#[test]
fn private_proposals_survive_reopen_and_only_owner_can_commit() -> TestResult {
    let mut peers = Enrollment::joined(limits())?;
    let before = peers.member.bytes()?;
    assert!(peers.member.adapter().rekey(operation(3), GROUP).is_err());
    assert_eq!(peers.member.bytes()?, before);
    let proposal = peers.member.adapter().propose_update(operation(3), GROUP)?;
    peers
        .owner
        .adapter()
        .receive(operation(3), GROUP, &proposal.output.messages[0], |_| {
            panic!("a control proposal reached application admission")
        })?;
    peers.owner.reopen()?;
    let rekey = peers.owner.adapter().rekey(operation(4), GROUP)?;
    peers.member.reopen()?;
    peers
        .member
        .adapter()
        .receive(operation(4), GROUP, &rekey.output.messages[0], |_| {
            panic!("a commit reached application admission")
        })?;
    let sent = peers
        .owner
        .adapter()
        .send(operation(5), GROUP, b"after leaf refresh")?;
    peers
        .member
        .adapter()
        .receive(operation(5), GROUP, &sent.output.messages[0], |_| {
            Admission::Accept
        })?;
    Ok(())
}

#[test]
fn untrusted_signed_key_package_cannot_change_persisted_membership() -> TestResult {
    let mut peers = Enrollment::joined(limits())?;
    let stranger_identity = LocalIdentity::generate([3; 32], 1)?;
    let stranger_policy = Policy::new(
        stranger_identity.binding(),
        vec![stranger_identity.binding()],
    )?;
    let stranger = Endpoint::new(stranger_identity, stranger_policy, limits())?;
    let package = stranger.adapter().key_package(operation(1), NOW)?;
    let before = peers.owner.bytes()?;
    assert!(peers
        .owner
        .adapter()
        .add(operation(3), GROUP, &[&package.output.messages[0]], NOW)
        .is_err());
    assert_eq!(peers.owner.bytes()?, before);
    peers.owner.reopen()?;
    let rekey = peers.owner.adapter().rekey(operation(3), GROUP)?;
    peers
        .member
        .adapter()
        .receive(operation(3), GROUP, &rekey.output.messages[0], |_| {
            Admission::Accept
        })?;
    Ok(())
}

#[test]
fn invalid_prior_epoch_aead_does_not_consume_delayed_key() -> TestResult {
    let mut peers = Enrollment::joined(limits())?;
    let delayed = peers
        .owner
        .adapter()
        .send(operation(3), GROUP, b"delayed")?;
    let rekey = peers.owner.adapter().rekey(operation(4), GROUP)?;
    peers
        .member
        .adapter()
        .receive(operation(3), GROUP, &rekey.output.messages[0], |_| {
            Admission::Accept
        })?;
    peers.member.reopen()?;
    let before = peers.member.bytes()?;
    let mut damaged = delayed.output.messages[0].clone();
    *damaged.last_mut().unwrap() ^= 1;
    assert!(peers
        .member
        .adapter()
        .receive(operation(4), GROUP, &damaged, |_| {
            panic!("invalid retained-epoch data reached admission")
        })
        .is_err());
    assert_eq!(peers.member.bytes()?, before);
    peers.member.reopen()?;
    let received =
        peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &delayed.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(received.output.event, Event::Application(ref body) if body == b"delayed"));
    peers.member.reopen()?;
    assert!(peers
        .member
        .adapter()
        .receive(operation(5), GROUP, &delayed.output.messages[0], |_| {
            Admission::Accept
        })
        .is_err());
    Ok(())
}

#[test]
fn delayed_messages_from_independent_senders_survive_reopen() -> TestResult {
    let Trio {
        owner,
        mut member,
        other,
    } = Trio::joined(limits())?;
    let mut owner_messages = Vec::new();
    for generation in 0..4 {
        owner_messages.push(
            owner
                .adapter()
                .send(operation(10 + generation), GROUP, &[generation])?
                .output
                .messages[0]
                .clone(),
        );
    }
    let other_first = other.adapter().send(operation(10), GROUP, b"other zero")?;
    let other_later = other.adapter().send(operation(11), GROUP, b"other one")?;
    let deliveries: [(u8, &[u8], &[u8]); 6] = [
        (10, &owner_messages[1], &[1]),
        (11, &other_later.output.messages[0], b"other one"),
        (12, &owner_messages[3], &[3]),
        (13, &other_first.output.messages[0], b"other zero"),
        (14, &owner_messages[0], &[0]),
        (15, &owner_messages[2], &[2]),
    ];
    for (id, wire, expected) in deliveries {
        let received = member
            .adapter()
            .receive(operation(id), GROUP, wire, |_| Admission::Accept)?;
        assert!(matches!(received.output.event, Event::Application(ref body) if body == expected));
        member.reopen()?;
    }
    let before = member.bytes()?;
    assert!(member
        .adapter()
        .receive(operation(16), GROUP, &owner_messages[2], |_| {
            panic!("consumed delayed message reached admission")
        })
        .is_err());
    assert_eq!(member.bytes()?, before);
    Ok(())
}

#[test]
fn removal_barrier_atomically_purges_retained_epochs_and_disables_removed_member() -> TestResult {
    for cut in CUTS {
        let mut peers = Trio::joined(limits())?;
        let delayed_before_rekey = peers
            .owner
            .adapter()
            .send(operation(3), GROUP, b"old epoch")?;
        let rekey = peers.owner.adapter().rekey(operation(4), GROUP)?;
        for endpoint in [&peers.member, &peers.other] {
            endpoint
                .adapter()
                .receive(operation(3), GROUP, &rekey.output.messages[0], |_| {
                    Admission::Accept
                })?;
        }
        let delayed_before_removal =
            peers
                .owner
                .adapter()
                .send(operation(5), GROUP, b"old authority")?;
        let removal = peers.owner.adapter().remove(operation(6), GROUP, 2)?;
        let before = peers.member.bytes()?;
        peers.member.faults.arm(cut);
        assert!(peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &removal.output.messages[0], |_| {
                Admission::Accept
            },)
            .is_err());
        peers.member.reopen()?;
        let recovered = peers.member.adapter().coordinator().recover(operation(4))?;
        if matches!(cut, Cut::AfterPublish) {
            assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
            assert!(matches!(
                peers.member.adapter().retry(operation(4))?.unwrap().event,
                Event::Control
            ));
        } else {
            assert!(matches!(recovered, RecoveryOutcome::NotCommitted { .. }));
            assert_eq!(peers.member.bytes()?, before);
            peers.member.adapter().receive(
                operation(4),
                GROUP,
                &removal.output.messages[0],
                |_| Admission::Accept,
            )?;
        }
        peers.member.reopen()?;
        for (id, delayed) in [(5, &delayed_before_rekey), (6, &delayed_before_removal)] {
            assert!(peers
                .member
                .adapter()
                .receive(operation(id), GROUP, &delayed.output.messages[0], |_| {
                    panic!("pre-barrier content reached application admission")
                },)
                .is_err());
        }
        let after = peers
            .owner
            .adapter()
            .send(operation(7), GROUP, b"current authority")?;
        peers
            .member
            .adapter()
            .receive(operation(7), GROUP, &after.output.messages[0], |_| {
                Admission::Accept
            })?;
        let removed = peers.other.adapter().receive(
            operation(4),
            GROUP,
            &removal.output.messages[0],
            |_| Admission::Accept,
        )?;
        assert!(matches!(removed.output.event, Event::Removed));
        peers.other.reopen()?;
        assert!(peers
            .other
            .adapter()
            .send(operation(5), GROUP, b"removed")
            .is_err());
        assert!(peers
            .other
            .adapter()
            .receive(
                operation(6),
                GROUP,
                &delayed_before_rekey.output.messages[0],
                |_| Admission::Accept,
            )
            .is_err());
    }
    Ok(())
}

#[test]
fn reopened_configuration_rejects_changed_owner_identity_and_limits() -> TestResult {
    let peers = Enrollment::joined(limits())?;
    let before = peers.member.bytes()?;
    let changed_owner = Policy::new(
        peers.member.identity.binding(),
        vec![
            peers.owner.identity.binding(),
            peers.member.identity.binding(),
        ],
    )?;
    let coordinator = Coordinator::new(
        FileBackend::open(peers.member.directory.path())?,
        store_limits(),
    )?;
    assert!(Adapter::new(
        coordinator,
        peers.member.identity.clone(),
        changed_owner,
        limits()
    )
    .is_err());
    let replacement = LocalIdentity::generate([2; 32], 2)?;
    let changed_identity = Policy::new(
        peers.owner.identity.binding(),
        vec![peers.owner.identity.binding(), replacement.binding()],
    )?;
    let coordinator = Coordinator::new(
        FileBackend::open(peers.member.directory.path())?,
        store_limits(),
    )?;
    assert!(Adapter::new(coordinator, replacement, changed_identity, limits()).is_err());
    let mut reduced = limits();
    reduced.max_plaintext_bytes = 1;
    let coordinator = Coordinator::new(
        FileBackend::open(peers.member.directory.path())?,
        store_limits(),
    )?;
    assert!(Adapter::new(
        coordinator,
        peers.member.identity.clone(),
        peers.member.policy.clone(),
        reduced
    )
    .is_err());
    assert_eq!(peers.member.bytes()?, before);
    assert!(matches!(
        peers.member.adapter().retry(operation(2))?.unwrap().event,
        Event::Joined
    ));
    Ok(())
}

#[test]
fn non_owner_welcome_is_rejected_even_when_the_real_owner_is_in_the_roster() -> TestResult {
    use mls_rs::{
        group::mls_rules::{CommitOptions, DefaultMlsRules, EncryptionOptions},
        identity::{
            basic::{BasicCredential, BasicIdentityProvider},
            SigningIdentity,
        },
        CipherSuite, CipherSuiteProvider, Client, CryptoProvider, MlsMessage,
    };
    use mls_rs_crypto_rustcrypto::RustCryptoProvider;

    let owner_identity = LocalIdentity::generate([1; 32], 1)?;
    let member_identity = LocalIdentity::generate([2; 32], 1)?;
    let crypto =
        RustCryptoProvider::with_enabled_cipher_suites(vec![CipherSuite::CURVE25519_AES128]);
    let suite = crypto
        .cipher_suite_provider(CipherSuite::CURVE25519_AES128)
        .unwrap();
    let (secret, public) = suite.signature_key_generate().unwrap();
    let attacker_binding = MemberBinding {
        endpoint: [3; 32],
        incarnation: 1,
        signing_key: public.as_ref().try_into()?,
    };
    let policy = Policy::new(
        owner_identity.binding(),
        vec![
            owner_identity.binding(),
            member_identity.binding(),
            attacker_binding.clone(),
        ],
    )?;
    let owner = Endpoint::new(owner_identity, policy.clone(), limits())?;
    let mut member = Endpoint::new(member_identity, policy, limits())?;
    let owner_package = owner.adapter().key_package(operation(1), NOW)?;
    let member_package = member.adapter().key_package(operation(1), NOW)?;
    let mut credential = attacker_binding.endpoint.to_vec();
    credential.extend_from_slice(&attacker_binding.incarnation.to_be_bytes());
    let attacker = Client::builder()
        .identity_provider(BasicIdentityProvider)
        .crypto_provider(crypto)
        .mls_rules(
            DefaultMlsRules::new()
                .with_commit_options(CommitOptions::new().with_path_required(true))
                .with_encryption_options(EncryptionOptions::new(true, Default::default())),
        )
        .signing_identity(
            SigningIdentity::new(BasicCredential::new(credential).into_credential(), public),
            secret,
            CipherSuite::CURVE25519_AES128,
        )
        .build();
    let mut fake_group = attacker
        .group_builder()
        .unwrap()
        .with_group_id(GROUP.0.to_vec())
        .build()
        .unwrap();
    let fake_commit = fake_group
        .commit_builder()
        .commit_time(NOW.into())
        .add_member(MlsMessage::from_bytes(&owner_package.output.messages[0]).unwrap())
        .unwrap()
        .add_member(MlsMessage::from_bytes(&member_package.output.messages[0]).unwrap())
        .unwrap()
        .build()
        .unwrap();
    let before = member.bytes()?;
    assert!(matches!(
        member.adapter().join(
            operation(2),
            GROUP,
            &fake_commit.welcome_messages[0].to_bytes().unwrap(),
            NOW
        ),
        Err(Error::Policy(prns_groups_mls::PolicyError::Owner))
    ));
    assert_eq!(member.bytes()?, before);
    member.reopen()?;
    owner.adapter().create(operation(2), GROUP)?;
    let legitimate = owner.adapter().add(
        operation(3),
        GROUP,
        &[&member_package.output.messages[0]],
        NOW,
    )?;
    let joined = member
        .adapter()
        .join(operation(2), GROUP, &legitimate.output.messages[1], NOW)?;
    assert!(matches!(joined.output.event, Event::Joined));
    Ok(())
}

#[test]
fn exact_send_retry_survives_routine_rekey_but_stops_at_known_removal() -> TestResult {
    let mut peers = Enrollment::joined(limits())?;
    let owner_sent = peers
        .owner
        .adapter()
        .send(operation(3), GROUP, b"owner old era")?;
    let member_sent = peers
        .member
        .adapter()
        .send(operation(3), GROUP, b"member old era")?;
    let rekey = peers.owner.adapter().rekey(operation(4), GROUP)?;
    peers
        .member
        .adapter()
        .receive(operation(4), GROUP, &rekey.output.messages[0], |_| {
            Admission::Accept
        })?;
    peers.owner.reopen()?;
    peers.member.reopen()?;
    assert_eq!(
        peers.owner.adapter().retry(operation(3))?.unwrap().messages,
        owner_sent.output.messages
    );
    assert_eq!(
        peers
            .member
            .adapter()
            .retry(operation(3))?
            .unwrap()
            .messages,
        member_sent.output.messages
    );

    let removal = peers.owner.adapter().remove(operation(5), GROUP, 1)?;
    peers.owner.reopen()?;
    assert!(matches!(
        peers.owner.adapter().retry(operation(3)),
        Err(Error::PriorAuthorizationEra)
    ));
    let control = peers.owner.adapter().retry(operation(5))?.unwrap();
    assert_eq!(control.messages, removal.output.messages);
    assert!(matches!(control.event, Event::Control));
    let removed =
        peers
            .member
            .adapter()
            .receive(operation(5), GROUP, &control.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(removed.output.event, Event::Removed));
    peers.member.reopen()?;
    assert!(matches!(
        peers.member.adapter().retry(operation(3)),
        Err(Error::PriorAuthorizationEra)
    ));
    assert!(matches!(
        peers.member.adapter().retry(operation(5))?.unwrap().event,
        Event::Removed
    ));
    Ok(())
}

#[test]
fn output_capacity_reclaims_acknowledged_slot_without_reusing_committed_operation() -> TestResult {
    let mut profile = limits();
    profile.max_pending_outputs = 3;
    let mut peers = Enrollment::joined(profile)?;
    peers
        .member
        .adapter()
        .send(operation(3), GROUP, b"fills last slot")?;
    let before = peers.member.bytes()?;
    assert!(matches!(
        peers
            .member
            .adapter()
            .send(operation(4), GROUP, b"waits for capacity"),
        Err(Error::Provider(ProviderError::Capacity(
            Limit::PendingOutputs
        )))
    ));
    assert_eq!(peers.member.bytes()?, before);
    peers.member.reopen()?;
    peers
        .member
        .adapter()
        .acknowledge(operation(5), operation(3))?;
    peers.member.reopen()?;
    assert!(peers.member.adapter().retry(operation(3))?.is_none());
    assert!(matches!(
        peers
            .member
            .adapter()
            .send(operation(3), GROUP, b"must not reuse generation"),
        Err(Error::Store(StoreError::AlreadyCommitted(_)))
    ));
    let sent = peers
        .member
        .adapter()
        .send(operation(4), GROUP, b"waits for capacity")?;
    peers
        .owner
        .adapter()
        .receive(operation(3), GROUP, &sent.output.messages[0], |_| {
            Admission::Accept
        })?;
    Ok(())
}

#[test]
fn oversized_and_trailing_wire_bytes_rollback_before_valid_retry() -> TestResult {
    let mut peers = Enrollment::joined(limits())?;
    let sent = peers
        .owner
        .adapter()
        .send(operation(3), GROUP, b"valid wire")?;
    let before = peers.member.bytes()?;
    let oversized = vec![0; limits().max_wire_bytes + 1];
    assert!(matches!(
        peers
            .member
            .adapter()
            .receive(operation(3), GROUP, &oversized, |_| panic!(
                "oversized input reached admission"
            ),),
        Err(Error::Capacity(Limit::WireBytes))
    ));
    assert_eq!(peers.member.bytes()?, before);
    peers.member.reopen()?;
    let mut trailing = sent.output.messages[0].clone();
    trailing.push(0);
    assert!(matches!(
        peers
            .member
            .adapter()
            .receive(operation(3), GROUP, &trailing, |_| panic!(
                "trailing input reached admission"
            ),),
        Err(Error::UnexpectedMessage)
    ));
    assert_eq!(peers.member.bytes()?, before);
    peers.member.reopen()?;
    assert!(peers.member.adapter().retry(operation(3))?.is_none());
    let received =
        peers
            .member
            .adapter()
            .receive(operation(3), GROUP, &sent.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(received.output.event, Event::Application(ref body) if body == b"valid wire"));
    Ok(())
}

#[test]
fn zero_clock_key_package_fails_without_panic_or_durable_change() -> TestResult {
    let identity = LocalIdentity::generate([1; 32], 1)?;
    let policy = Policy::new(identity.binding(), vec![identity.binding()])?;
    let mut endpoint = Endpoint::new(identity, policy, limits())?;
    let before = endpoint.bytes()?;
    assert!(matches!(
        endpoint.adapter().key_package(operation(1), 0),
        Err(Error::TimeUncertain)
    ));
    assert_eq!(endpoint.bytes()?, before);
    endpoint.reopen()?;
    assert!(endpoint.adapter().retry(operation(1))?.is_none());
    let package = endpoint.adapter().key_package(operation(1), NOW)?;
    assert!(matches!(package.output.event, Event::KeyPackage));
    Ok(())
}

#[test]
fn admission_unwind_revokes_transaction_and_discards_receive_state() -> TestResult {
    let peers = Enrollment::joined(limits())?;
    let sent = peers.owner.adapter().send(operation(3), GROUP, b"unwind")?;
    let before = peers.member.bytes()?;
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ =
            peers
                .member
                .adapter()
                .receive(operation(3), GROUP, &sent.output.messages[0], |_| {
                    panic!("simulated application admission failure");
                });
    }));
    assert!(failure.is_err());
    assert_eq!(peers.member.bytes()?, before);
    assert!(peers.member.adapter().retry(operation(3))?.is_none());
    let received =
        peers
            .member
            .adapter()
            .receive(operation(3), GROUP, &sent.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(&received.output.event, Event::Application(body) if body == b"unwind"));
    Ok(())
}
