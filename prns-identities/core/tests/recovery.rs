#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use prns_identities_core::*;
use support::{Durable, Failure};

const INCARNATION: [u8; 16] = [0x41; 16];
type State = Catalog<4, 16, 2>;
type Disk = Durable<4, 16, 2>;
type Core<'a> = Coordinator<'a, 4, 16, 2>;

const TELEMETRY: ServiceId = ServiceId(17);
const DIGEST: CommandDigest = CommandDigest([0x61; 32]);

fn create(label: &str) -> Command<2> {
    Command::Create(CreateRequest {
        source: KeySource::Generate,
        metadata: Metadata::new(label, None).unwrap(),
        services: ServiceSet::new(&[TELEMETRY]).unwrap(),
    })
}

fn dependencies(revision: u64) -> DependencyReport<2> {
    DependencyReport::new(&[ServiceDependency::clear(TELEMETRY, revision)], 1).unwrap()
}

/// This vault survives coordinator restarts independently of the catalog. Its
/// idempotent reference lookup models reopening durable keys after a lost ack.
#[derive(Default)]
struct Vault {
    keys: Vec<VerifiedKey>,
    generated: usize,
    accounted_extra_keys: Vec<KeyReference>,
}

impl Vault {
    fn perform(&mut self, action: Action<2>) -> VerifiedKey {
        let Action::PublishKey {
            key_reference,
            source,
            ..
        } = action
        else {
            panic!("expected key publication")
        };
        if let Some(key) = self
            .keys
            .iter()
            .find(|key| key.key_reference == key_reference)
        {
            return *key;
        }
        let public_hash = match source {
            KeySource::Generate => {
                self.generated += 1;
                PublicIdentityHash([u8::try_from(self.generated).unwrap(); 16])
            }
            KeySource::Import { expected_hash } => expected_hash,
        };
        let key = VerifiedKey {
            key_reference,
            public_hash,
        };
        self.keys.push(key);
        key
    }
}

#[derive(Debug)]
enum AttemptError {
    Storage,
    LostExternalReply,
}

/// The caller stores its operation token before beginning side effects. A lost
/// reserve reply is recovered from the catalog's still-reserved operation.
fn create_attempt(
    disk: &mut Disk,
    vault: &mut Vault,
    caller_token: &mut Option<OperationToken>,
    lose_key_reply: bool,
) -> Result<LocalIdentityId, AttemptError> {
    let command = create("Weather station");
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core =
        Coordinator::open(disk, &mut working, &mut staging).map_err(|_| AttemptError::Storage)?;
    let token = match *caller_token {
        Some(token) => token,
        None => {
            let reserved = core
                .snapshot()
                .operations()
                .find(|op| op.command == command && op.token.digest == DIGEST)
                .map(|op| op.token);
            match reserved {
                Some(token) => token,
                None => core
                    .reserve(disk, command, DIGEST)
                    .map_err(|_| AttemptError::Storage)?,
            }
        }
    };
    *caller_token = Some(token);
    core.begin(disk, token, &command)
        .map_err(|_| AttemptError::Storage)?;
    if let Some(action) = core.action(token).unwrap() {
        let key = vault.perform(action);
        if lose_key_reply {
            return Err(AttemptError::LostExternalReply);
        }
        core.ack_key(disk, action.ticket(), key)
            .map_err(|_| AttemptError::Storage)?;
    }
    let OperationState::Completed(OperationResult::Created { identity }) =
        core.resume(token, &command).unwrap()
    else {
        panic!("creation did not settle")
    };
    let record = core
        .snapshot()
        .identities()
        .find(|record| record.id == identity)
        .unwrap();
    assert_eq!(record.desired, DesiredState::Paused);
    assert!(vault
        .keys
        .iter()
        .any(|key| key.key_reference == record.key_reference
            && key.public_hash == record.public_hash));
    Ok(identity)
}

#[test]
fn creation_survives_every_catalog_write_before_or_after_commit() {
    let mut baseline = Disk::new(State::new(INCARNATION));
    create_attempt(&mut baseline, &mut Vault::default(), &mut None, false).unwrap();
    let creation_writes = baseline.writes;
    assert!(creation_writes >= 4);

    for write in 1..=creation_writes {
        for failure in [Failure::BeforeWrite, Failure::AfterWrite] {
            let mut disk = Disk::new(State::new(INCARNATION));
            let mut vault = Vault::default();
            let mut caller_token = None;
            disk.fail_write = Some((write, failure));
            assert!(create_attempt(&mut disk, &mut vault, &mut caller_token, false).is_err());

            // At any publication boundary a visible identity must already have
            // the exact durable key, independently of whether the reply arrived.
            for record in disk.catalog.identities() {
                assert!(vault
                    .keys
                    .iter()
                    .any(|key| key.key_reference == record.key_reference
                        && key.public_hash == record.public_hash));
            }
            disk.reboot();
            let identity = create_attempt(&mut disk, &mut vault, &mut caller_token, false).unwrap();
            let replay = create_attempt(&mut disk, &mut vault, &mut caller_token, false).unwrap();
            assert_eq!(identity, replay, "lost reply at write {write}: {failure:?}");
            assert_eq!(disk.catalog.identities().count(), 1);
            assert_eq!(vault.generated, 1);
            assert_eq!(vault.keys.len(), 1);
        }
    }
}

#[test]
fn durable_vault_key_is_reopened_after_external_reply_is_lost() {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut vault = Vault::default();
    let mut token = None;
    assert!(matches!(
        create_attempt(&mut disk, &mut vault, &mut token, true),
        Err(AttemptError::LostExternalReply)
    ));
    assert_eq!(vault.generated, 1);
    assert_eq!(disk.catalog.identities().count(), 0);
    let key = vault.keys[0];
    disk.reboot();
    create_attempt(&mut disk, &mut vault, &mut token, false).unwrap();
    assert_eq!(vault.keys, vec![key]);
    assert_eq!(disk.catalog.identities().count(), 1);
}

fn seeded() -> (Disk, Vault, LocalIdentityId) {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut vault = Vault::default();
    let identity = create_attempt(&mut disk, &mut vault, &mut None, false).unwrap();
    (disk, vault, identity)
}

fn transition(disk: &Disk, identity: LocalIdentityId, target: TransitionTarget) -> Command<2> {
    let record = disk
        .catalog
        .identities()
        .find(|record| record.id == identity)
        .unwrap();
    Command::Transition(PreparedTransition {
        identity: record.guard(),
        target,
        review: dependencies(7),
    })
}

/// Application services and infrastructure have separate owners. Quiescing a
/// persona cannot erase its telemetry outbox or its board-management key use.
#[derive(Clone)]
struct Runtime {
    active: Vec<LocalIdentityId>,
    management_key_uses: usize,
    pending: Vec<(LocalIdentityId, [u8; 32])>,
    receipts: Vec<(LocalIdentityId, [u8; 32])>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            active: Vec::new(),
            management_key_uses: 1,
            pending: Vec::new(),
            receipts: Vec::new(),
        }
    }
}

impl Runtime {
    fn perform(&mut self, action: Action<2>) {
        match action {
            Action::Activate { identity, .. } => {
                if !self.active.contains(&identity.id) {
                    self.active.push(identity.id);
                }
            }
            Action::Quiesce { identity, .. } => self.active.retain(|id| *id != identity.id),
            Action::PublishKey { .. } | Action::InitializeService { .. } => {
                panic!("runtime cannot publish vault keys")
            }
        }
    }
}

fn transition_attempt(
    disk: &mut Disk,
    runtime: &mut Runtime,
    caller_token: &mut Option<OperationToken>,
    command: Command<2>,
    after_drain: DependencyReport<2>,
    lose_runtime_reply: bool,
) -> Result<OperationResult, AttemptError> {
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core =
        Coordinator::open(disk, &mut working, &mut staging).map_err(|_| AttemptError::Storage)?;
    let digest = CommandDigest([0x71; 32]);
    let token = match *caller_token {
        Some(token) => token,
        None => {
            let prior = core
                .snapshot()
                .operations()
                .find(|op| op.command == command && op.token.digest == digest)
                .map(|op| op.token);
            match prior {
                Some(token) => token,
                None => core
                    .reserve(disk, command, digest)
                    .map_err(|_| AttemptError::Storage)?,
            }
        }
    };
    *caller_token = Some(token);
    core.begin(disk, token, &command)
        .map_err(|_| AttemptError::Storage)?;
    if let Some(action) = core.action(token).unwrap() {
        runtime.perform(action);
        if lose_runtime_reply {
            return Err(AttemptError::LostExternalReply);
        }
        match action {
            Action::Activate { .. } => core
                .ack_activated(disk, action.ticket())
                .map_err(|_| AttemptError::Storage)?,
            Action::Quiesce { .. } => {
                core.ack_quiesced(disk, action.ticket(), after_drain)
                    .map_err(|_| AttemptError::Storage)?;
            }
            Action::PublishKey { .. } | Action::InitializeService { .. } => {
                panic!("unexpected key action")
            }
        };
    }
    let OperationState::Completed(result) = core.resume(token, &command).unwrap() else {
        panic!("transition did not settle")
    };
    Ok(result)
}

#[test]
fn each_transition_recovers_before_and_after_every_durable_write() {
    for target in [
        TransitionTarget::Enable,
        TransitionTarget::Pause,
        TransitionTarget::Retire,
    ] {
        let (mut initial, _, identity) = seeded();
        let mut initial_runtime = Runtime::default();
        if target != TransitionTarget::Enable {
            let enable = transition(&initial, identity, TransitionTarget::Enable);
            transition_attempt(
                &mut initial,
                &mut initial_runtime,
                &mut None,
                enable,
                dependencies(7),
                false,
            )
            .unwrap();
        }
        let command = transition(&initial, identity, target);
        let mut baseline = initial.clone();
        let expected = transition_attempt(
            &mut baseline,
            &mut initial_runtime.clone(),
            &mut None,
            command,
            dependencies(7),
            false,
        )
        .unwrap();
        let writes = baseline.writes - initial.writes;
        assert!(writes >= 4);
        for offset in 1..=writes {
            for failure in [Failure::BeforeWrite, Failure::AfterWrite] {
                let mut disk = initial.clone();
                let mut runtime = initial_runtime.clone();
                let mut token = None;
                disk.fail_write = Some((disk.writes + offset, failure));
                assert!(transition_attempt(
                    &mut disk,
                    &mut runtime,
                    &mut token,
                    command,
                    dependencies(7),
                    false
                )
                .is_err());
                disk.reboot();
                let recovered = transition_attempt(
                    &mut disk,
                    &mut runtime,
                    &mut token,
                    command,
                    dependencies(7),
                    false,
                )
                .unwrap();
                assert_eq!(
                    recovered, expected,
                    "{target:?} write {offset}: {failure:?}"
                );
                assert_eq!(
                    transition_attempt(
                        &mut disk,
                        &mut runtime,
                        &mut token,
                        command,
                        dependencies(7),
                        false
                    )
                    .unwrap(),
                    expected
                );
                assert_eq!(runtime.management_key_uses, 1);
                assert_eq!(disk.catalog.identities().count(), 1);
                let record = disk.catalog.identities().next().unwrap();
                assert!(record.pending.is_none());
                assert_eq!(
                    record.desired,
                    match target {
                        TransitionTarget::Enable => DesiredState::Enabled,
                        TransitionTarget::Pause => DesiredState::Paused,
                        TransitionTarget::Retire => DesiredState::Retired,
                        TransitionTarget::AddService(_) | TransitionTarget::RemoveService(_) =>
                            panic!("configuration has separate fixtures"),
                    }
                );
            }
        }
    }
}

#[test]
fn quiescence_reply_loss_preserves_exact_pending_owner_and_other_key_uses() {
    let (mut disk, _, identity) = seeded();
    let mut runtime = Runtime::default();
    let enable = transition(&disk, identity, TransitionTarget::Enable);
    transition_attempt(
        &mut disk,
        &mut runtime,
        &mut None,
        enable,
        dependencies(7),
        false,
    )
    .unwrap();
    let other = LocalIdentityId {
        incarnation: INCARNATION,
        sequence: 999,
    };
    runtime.active.push(other);
    runtime.pending.push((identity, [0x93; 32]));
    let pending = runtime.pending.clone();
    let mut report = dependencies(7);
    report.entries[0].as_mut().unwrap().pending_messages = 1;
    let Command::Transition(mut prepared) = transition(&disk, identity, TransitionTarget::Pause)
    else {
        unreachable!()
    };
    prepared.review = report;
    let pause = Command::Transition(prepared);
    let mut token = None;
    assert!(matches!(
        transition_attempt(&mut disk, &mut runtime, &mut token, pause, report, true),
        Err(AttemptError::LostExternalReply)
    ));
    assert_eq!(runtime.active, vec![other]);
    disk.reboot();
    assert_eq!(
        transition_attempt(&mut disk, &mut runtime, &mut token, pause, report, false).unwrap(),
        OperationResult::Paused { identity }
    );
    assert_eq!(runtime.pending, pending);
    assert!(runtime.receipts.is_empty());
    assert_eq!(runtime.management_key_uses, 1);
    assert_eq!(runtime.active, vec![other]);
}

#[test]
fn retirement_rechecks_obligations_created_by_the_admitted_work_it_drains() {
    for mut entry in [ServiceDependency::clear(TELEMETRY, 8); 4]
        .into_iter()
        .enumerate()
        .map(|(index, mut entry)| {
            match index {
                0 => entry.authority_count = 1,
                1 => entry.unsettled_control = 1,
                2 => entry.pending_messages = 1,
                _ => entry.recovery_pending = true,
            }
            entry
        })
    {
        let (mut disk, _, identity) = seeded();
        let retire = transition(&disk, identity, TransitionTarget::Retire);
        let result = transition_attempt(
            &mut disk,
            &mut Runtime::default(),
            &mut None,
            retire,
            DependencyReport::new(&[entry], 1).unwrap(),
            false,
        )
        .unwrap();
        assert!(
            matches!(result, OperationResult::RetirementBlocked { identity: actual, .. }
            if actual == identity)
        );
        assert_eq!(
            disk.catalog.identities().next().unwrap().desired,
            DesiredState::Paused
        );

        // Even a changed revision with no reported blocker needs a fresh review.
        entry = ServiceDependency::clear(TELEMETRY, 9);
        let retry = transition(&disk, identity, TransitionTarget::Retire);
        assert!(matches!(
            transition_attempt(
                &mut disk,
                &mut Runtime::default(),
                &mut None,
                retry,
                DependencyReport::new(&[entry], 1).unwrap(),
                false
            )
            .unwrap(),
            OperationResult::RetirementBlocked { .. }
        ));
    }
}

fn create_in_session(
    core: &mut Core<'_>,
    disk: &mut Disk,
    vault: &mut Vault,
    label: &str,
) -> (OperationToken, LocalIdentityId) {
    let command = create(label);
    let token = core.reserve(disk, command, DIGEST).unwrap();
    core.begin(disk, token, &command).unwrap();
    let action = core.action(token).unwrap().unwrap();
    let key = vault.perform(action);
    core.ack_key(disk, action.ticket(), key).unwrap();
    let OperationState::Completed(OperationResult::Created { identity }) =
        core.resume(token, &command).unwrap()
    else {
        panic!("expected created identity")
    };
    (token, identity)
}

#[test]
fn reservation_is_inert_and_a_changed_command_cannot_use_its_token() {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let command = create("Personal");
    let token = core.reserve(&mut disk, command, DIGEST).unwrap();
    assert_eq!(core.resume(token, &command), Ok(OperationState::Reserved));
    assert!(core.action(token).unwrap().is_none());
    assert_eq!(core.snapshot().identities().count(), 0);
    let changed = create("Work");
    assert!(matches!(
        core.begin(&mut disk, token, &changed),
        Err(Error::Fault(Fault::CommandMismatch))
    ));
    assert_eq!(core.resume(token, &command), Ok(OperationState::Reserved));
}

#[test]
fn every_ambiguous_write_fences_further_work_until_authoritative_reload() {
    for failure in [Failure::BeforeWrite, Failure::AfterWrite] {
        let mut disk = Disk::new(State::new(INCARNATION));
        let mut working = State::new(INCARNATION);
        let mut staging = State::new(INCARNATION);
        let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
        let command = create("Personal");
        let token = core.reserve(&mut disk, command, DIGEST).unwrap();
        disk.fail_next(failure);
        assert!(matches!(
            core.begin(&mut disk, token, &command),
            Err(Error::Storage(_))
        ));
        assert!(core.is_poisoned());
        assert_eq!(core.action(token), Err(Fault::Poisoned));
        assert!(core
            .reserve(&mut disk, create("Must not appear"), DIGEST)
            .is_err());
        disk.reboot();
        core.reload(&mut disk).unwrap();
        assert!(!core.is_poisoned());
        core.begin(&mut disk, token, &command).unwrap();
        assert!(matches!(
            core.action(token).unwrap(),
            Some(Action::PublishKey { .. })
        ));
        assert_eq!(core.snapshot().operations().count(), 1);
    }
}

#[test]
fn stale_vault_completion_cannot_cross_a_runtime_restart() {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut vault = Vault::default();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let command = create("Weather station");
    let token = core.reserve(&mut disk, command, DIGEST).unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    let old = core.action(token).unwrap().unwrap();
    let key = vault.perform(old);
    disk.reboot();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    assert!(matches!(
        core.ack_key(&mut disk, old.ticket(), key),
        Err(Error::Fault(Fault::StaleSession))
    ));
    assert_eq!(core.snapshot().identities().count(), 0);
    let fresh = core.action(token).unwrap().unwrap();
    assert_eq!(fresh.ticket().identity, old.ticket().identity);
    assert_ne!(fresh.ticket().session, old.ticket().session);
    core.ack_key(&mut disk, fresh.ticket(), vault.perform(fresh))
        .unwrap();
    assert_eq!(vault.generated, 1);
    assert_eq!(core.snapshot().identities().count(), 1);
}

#[test]
fn capacity_failure_does_not_evict_existing_identities_or_pending_operations() {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut vault = Vault::default();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    for label in ["Personal", "Work", "Community", "Weather"] {
        create_in_session(&mut core, &mut disk, &mut vault, label);
    }
    let identities: Vec<_> = core.snapshot().identities().copied().collect();
    let command = create("Too many");
    match core.reserve(&mut disk, command, DIGEST) {
        Ok(token) => assert!(matches!(
            core.begin(&mut disk, token, &command),
            Err(Error::Fault(Fault::CapacityExceeded))
        )),
        Err(error) => assert_eq!(error, Error::Fault(Fault::CapacityExceeded)),
    }
    assert_eq!(
        core.snapshot().identities().copied().collect::<Vec<_>>(),
        identities
    );
    assert_eq!(vault.generated, 4);

    // Exhaust operation slots with inert reservations: no vault/network effect
    // and no silent eviction of a retained result is permitted to make room.
    while core.snapshot().operations().count() < 16 {
        core.reserve(&mut disk, command, DIGEST).unwrap();
    }
    assert!(matches!(
        core.reserve(&mut disk, command, DIGEST),
        Err(Error::Fault(Fault::CapacityExceeded))
    ));
    assert_eq!(core.snapshot().operations().count(), 16);
    assert_eq!(
        core.snapshot().identities().copied().collect::<Vec<_>>(),
        identities
    );
}

#[test]
fn pruning_completed_results_keeps_a_rejection_fence_after_reopen() {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let (token, identity) =
        create_in_session(&mut core, &mut disk, &mut Vault::default(), "Personal");
    let command = create("Personal");
    let pending = core.reserve(&mut disk, create("Work"), DIGEST).unwrap();
    assert!(matches!(
        core.prune_completed(&mut disk, pending.sequence),
        Err(Error::Fault(Fault::CannotPrunePending))
    ));
    core.prune_completed(&mut disk, token.sequence).unwrap();
    disk.reboot();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    assert_eq!(core.resume(token, &command), Err(Fault::ReplayExpired));
    assert!(matches!(
        core.begin(&mut disk, token, &command),
        Err(Error::Fault(Fault::ReplayExpired))
    ));
    assert_eq!(core.snapshot().identities().next().unwrap().id, identity);
    assert_eq!(core.snapshot().identities().count(), 1);
}

#[test]
fn importing_a_known_retired_key_resolves_without_reactivation_or_relabeling() {
    let (mut disk, mut vault, identity) = seeded();
    let retire = transition(&disk, identity, TransitionTarget::Retire);
    transition_attempt(
        &mut disk,
        &mut Runtime::default(),
        &mut None,
        retire,
        dependencies(7),
        false,
    )
    .unwrap();
    let original = *disk.catalog.identities().next().unwrap();
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let command = Command::Create(CreateRequest {
        source: KeySource::Import {
            expected_hash: original.public_hash,
        },
        metadata: Metadata::new("New name must not replace existing", Some("Published")).unwrap(),
        services: ServiceSet::new(&[TELEMETRY]).unwrap(),
    });
    let token = core
        .reserve(&mut disk, command, CommandDigest([0x99; 32]))
        .unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    if let Some(action) = core.action(token).unwrap() {
        core.ack_key(&mut disk, action.ticket(), vault.perform(action))
            .unwrap();
    }
    assert_eq!(
        core.resume(token, &command).unwrap(),
        OperationState::Completed(OperationResult::Existing { identity })
    );
    assert_eq!(core.snapshot().identities().count(), 1);
    assert_eq!(*core.snapshot().identities().next().unwrap(), original);
    assert!(core.action(token).unwrap().is_none());
}

#[test]
fn activation_failure_cannot_finish_until_partial_runtime_registration_is_cleaned() {
    let (mut disk, _, identity) = seeded();
    let command = transition(&disk, identity, TransitionTarget::Enable);
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let token = core.reserve(&mut disk, command, DIGEST).unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    let activation = core.action(token).unwrap().unwrap();
    assert!(matches!(activation, Action::Activate { .. }));
    core.activation_failed(
        &mut disk,
        activation.ticket(),
        ServiceFailure::CapacityExceeded,
    )
    .unwrap();
    assert_eq!(
        core.resume(token, &command),
        Ok(OperationState::Transitioning)
    );
    let cleanup = core.action(token).unwrap().unwrap();
    assert!(matches!(cleanup, Action::Quiesce { .. }));
    disk.reboot();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let resumed_cleanup = core.action(token).unwrap().unwrap();
    assert!(matches!(resumed_cleanup, Action::Quiesce { .. }));
    assert_eq!(resumed_cleanup.ticket().identity, identity);
    assert_ne!(resumed_cleanup.ticket().session, cleanup.ticket().session);
    core.ack_quiesced(&mut disk, resumed_cleanup.ticket(), dependencies(7))
        .unwrap();
    assert_eq!(
        core.resume(token, &command),
        Ok(OperationState::Completed(
            OperationResult::ActivationFailed {
                identity,
                reason: ServiceFailure::CapacityExceeded
            }
        ))
    );
    assert_eq!(
        core.snapshot().identities().next().unwrap().desired,
        DesiredState::Paused
    );
}

fn transition_in_session(
    core: &mut Core<'_>,
    disk: &mut Disk,
    identity: LocalIdentityId,
    target: TransitionTarget,
    report: DependencyReport<2>,
) -> OperationToken {
    let command = Command::Transition(core.prepare(identity, target, report).unwrap());
    let token = core.reserve(disk, command, DIGEST).unwrap();
    core.begin(disk, token, &command).unwrap();
    let action = core.action(token).unwrap().unwrap();
    match action {
        Action::Activate { .. } => core.ack_activated(disk, action.ticket()).unwrap(),
        Action::Quiesce { .. } => {
            core.ack_quiesced(disk, action.ticket(), report).unwrap();
        }
        Action::PublishKey { .. } | Action::InitializeService { .. } => {
            panic!("unexpected creation effect")
        }
    }
    token
}

#[test]
fn pause_and_retire_fence_fresh_work_but_keep_exact_recovery_bound_to_original_owner() {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut vault = Vault::default();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let (creation, personal) = create_in_session(&mut core, &mut disk, &mut vault, "Personal");
    let (_, work) = create_in_session(&mut core, &mut disk, &mut vault, "Work");
    let personal_hash = core.identity(personal).unwrap().public_hash;
    let work_hash = core.identity(work).unwrap().public_hash;
    transition_in_session(
        &mut core,
        &mut disk,
        personal,
        TransitionTarget::Enable,
        dependencies(7),
    );
    let network = core
        .admit(personal, personal_hash, AdmissionClass::Network)
        .unwrap();
    let recovery = core
        .admit(personal, personal_hash, AdmissionClass::Recovery)
        .unwrap();
    let record = (personal, [0x22; 32]);
    let mut service = Runtime::default();
    service.pending.push(record);

    let mut report = dependencies(7);
    report.entries[0].as_mut().unwrap().pending_messages = 1;
    let command = Command::Transition(
        core.prepare(personal, TransitionTarget::Pause, report)
            .unwrap(),
    );
    let token = core.reserve(&mut disk, command, DIGEST).unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    assert!(core.validate_admission(network).is_err());
    assert_eq!(core.runtime_status(personal), Ok(RuntimeStatus::Stopping));
    let action = core.action(token).unwrap().unwrap();
    service.perform(action);
    core.ack_quiesced(&mut disk, action.ticket(), report)
        .unwrap();
    assert_eq!(
        core.admit(personal, personal_hash, AdmissionClass::Network),
        Err(Fault::Paused)
    );
    assert_eq!(service.pending, vec![record]);
    assert!(service.receipts.is_empty());

    // The service, not the lifecycle core, checks that receipt evidence belongs
    // to a previously admitted record. Selecting Work cannot relabel that fact.
    assert_eq!(core.validate_admission(recovery), Ok(()));
    assert_eq!(
        core.admit(personal, work_hash, AdmissionClass::Recovery),
        Err(Fault::IdentityChanged)
    );
    assert_eq!(recovery.identity.id, record.0);
    service.receipts.push(record);
    service.pending.retain(|pending| *pending != record);

    transition_in_session(
        &mut core,
        &mut disk,
        personal,
        TransitionTarget::Retire,
        dependencies(8),
    );
    assert_eq!(core.validate_admission(recovery), Ok(()));
    assert!(core
        .admit(personal, personal_hash, AdmissionClass::History)
        .is_ok());
    assert_eq!(
        core.admit(personal, personal_hash, AdmissionClass::Network),
        Err(Fault::Retired)
    );
    assert_eq!(
        core.admit(personal, personal_hash, AdmissionClass::OfflineSigning),
        Err(Fault::Retired)
    );
    assert_eq!(
        core.prepare(personal, TransitionTarget::Enable, dependencies(8)),
        Err(Fault::Retired)
    );
    assert_eq!(
        core.resume(creation, &create("Personal")),
        Ok(OperationState::Completed(OperationResult::Created {
            identity: personal
        }))
    );
    assert_eq!(service.receipts, vec![record]);
    assert_eq!(core.identity(work).unwrap().desired, DesiredState::Paused);
    assert_eq!(service.management_key_uses, 1);
}

#[test]
fn old_network_admission_cannot_revive_after_pause_resume_or_process_restart() {
    let (mut disk, _, identity) = seeded();
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let hash = core.identity(identity).unwrap().public_hash;
    transition_in_session(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::Enable,
        dependencies(7),
    );
    let first = core.admit(identity, hash, AdmissionClass::Network).unwrap();
    transition_in_session(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::Pause,
        dependencies(7),
    );
    transition_in_session(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::Enable,
        dependencies(7),
    );
    assert_eq!(core.validate_admission(first), Err(Fault::StaleLifecycle));
    let second = core.admit(identity, hash, AdmissionClass::Network).unwrap();
    let key_reference = core.identity(identity).unwrap().key_reference;
    disk.reboot();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    assert_eq!(
        core.admit(identity, hash, AdmissionClass::Network),
        Err(Fault::RuntimeUnavailable)
    );
    assert_eq!(
        core.identity(identity).unwrap().desired,
        DesiredState::Enabled
    );
    let restore = core.recovery_action(identity).unwrap().unwrap();
    assert!(matches!(restore, Action::Activate { key_reference: key, .. } if key == key_reference));
    assert_eq!(restore.ticket().kind, EffectKind::Restore);
    core.ack_activated(&mut disk, restore.ticket()).unwrap();
    assert_eq!(core.validate_admission(second), Err(Fault::StaleSession));
    assert!(core.admit(identity, hash, AdmissionClass::Network).is_ok());
    assert!(core.recovery_action(identity).unwrap().is_none());
}

#[test]
fn failed_restoration_cleanup_survives_restart_when_ordinary_operation_capacity_is_full() {
    let (mut disk, _, identity) = seeded();
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let hash = core.identity(identity).unwrap().public_hash;
    transition_in_session(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::Enable,
        dependencies(7),
    );
    loop {
        match core.reserve(&mut disk, create("Reserved, not started"), DIGEST) {
            Ok(_) => {}
            Err(Error::Fault(Fault::CapacityExceeded)) => break,
            Err(other) => panic!("unexpected reservation failure: {other:?}"),
        }
    }
    let operations = core.snapshot().operations().copied().collect::<Vec<_>>();
    disk.reboot();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let failed = core.recovery_action(identity).unwrap().unwrap();
    core.activation_failed(
        &mut disk,
        failed.ticket(),
        ServiceFailure::StorageUnavailable,
    )
    .unwrap();
    assert_eq!(core.runtime_status(identity), Ok(RuntimeStatus::Stopping));
    assert!(core.admit(identity, hash, AdmissionClass::Network).is_err());
    assert_eq!(
        core.snapshot().operations().copied().collect::<Vec<_>>(),
        operations
    );
    assert!(core.ack_activated(&mut disk, failed.ticket()).is_err());

    // Recovery cleanup is durable independently of exhausted ordinary operation
    // capacity. Reopening cannot accidentally start activation first.
    disk.reboot();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let cleanup = core.recovery_action(identity).unwrap().unwrap();
    assert!(matches!(cleanup, Action::Quiesce { .. }));
    assert_eq!(cleanup.ticket().kind, EffectKind::RestoreCleanup);
    core.ack_recovery_quiesced(&mut disk, cleanup.ticket(), dependencies(7))
        .unwrap();
    assert_eq!(
        core.identity(identity).unwrap().desired,
        DesiredState::Enabled
    );
    assert_eq!(
        core.runtime_status(identity),
        Ok(RuntimeStatus::Unavailable)
    );
    let replacement = core.recovery_action(identity).unwrap().unwrap();
    core.ack_activated(&mut disk, replacement.ticket()).unwrap();
    assert!(core
        .ack_recovery_quiesced(&mut disk, cleanup.ticket(), dependencies(7))
        .is_err());
    assert!(core.ack_activated(&mut disk, failed.ticket()).is_err());
    assert_eq!(core.runtime_status(identity), Ok(RuntimeStatus::Running));
    assert_eq!(
        core.snapshot().operations().copied().collect::<Vec<_>>(),
        operations
    );
}

#[test]
fn failed_restore_ticket_stays_stale_after_cleanup_in_the_same_runtime_session() {
    let (mut disk, _, identity) = seeded();
    let enable = transition(&disk, identity, TransitionTarget::Enable);
    transition_attempt(
        &mut disk,
        &mut Runtime::default(),
        &mut None,
        enable,
        dependencies(7),
        false,
    )
    .unwrap();
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let failed = core.recovery_action(identity).unwrap().unwrap();
    core.activation_failed(&mut disk, failed.ticket(), ServiceFailure::CapacityExceeded)
        .unwrap();
    let cleanup = core.recovery_action(identity).unwrap().unwrap();
    core.ack_recovery_quiesced(&mut disk, cleanup.ticket(), dependencies(7))
        .unwrap();
    let retry = core.recovery_action(identity).unwrap().unwrap();
    assert_eq!(retry.ticket().session, failed.ticket().session);
    assert_ne!(
        retry.ticket().lifecycle_revision,
        failed.ticket().lifecycle_revision
    );
    assert!(core.ack_activated(&mut disk, failed.ticket()).is_err());
    assert_eq!(
        core.runtime_status(identity),
        Ok(RuntimeStatus::Unavailable)
    );
    core.ack_activated(&mut disk, retry.ticket()).unwrap();
    assert!(core.ack_activated(&mut disk, failed.ticket()).is_err());
    assert!(core
        .ack_recovery_quiesced(&mut disk, cleanup.ticket(), dependencies(7))
        .is_err());
    assert_eq!(core.runtime_status(identity), Ok(RuntimeStatus::Running));
}

#[test]
fn completed_effect_acknowledgements_reject_changed_identity_revision_and_kind() {
    let (mut disk, _, identity) = seeded();
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    for target in [TransitionTarget::Enable, TransitionTarget::Pause] {
        let command = Command::Transition(core.prepare(identity, target, dependencies(7)).unwrap());
        let token = core.reserve(&mut disk, command, DIGEST).unwrap();
        core.begin(&mut disk, token, &command).unwrap();
        let action = core.action(token).unwrap().unwrap();
        match target {
            TransitionTarget::Enable => core.ack_activated(&mut disk, action.ticket()).unwrap(),
            TransitionTarget::Pause => {
                core.ack_quiesced(&mut disk, action.ticket(), dependencies(7))
                    .unwrap();
            }
            TransitionTarget::Retire
            | TransitionTarget::AddService(_)
            | TransitionTarget::RemoveService(_) => unreachable!(),
        }
        let original = action.ticket();
        for mut modified in [original; 3]
            .into_iter()
            .enumerate()
            .map(|(index, mut ticket)| {
                match index {
                    0 => ticket.identity.sequence += 100,
                    1 => ticket.lifecycle_revision += 1,
                    _ => ticket.kind = EffectKind::PublishKey,
                }
                ticket
            })
        {
            match target {
                TransitionTarget::Enable => {
                    assert!(core.ack_activated(&mut disk, modified).is_err())
                }
                TransitionTarget::Pause => assert!(core
                    .ack_quiesced(&mut disk, modified, dependencies(7))
                    .is_err()),
                TransitionTarget::Retire
                | TransitionTarget::AddService(_)
                | TransitionTarget::RemoveService(_) => unreachable!(),
            }
            // Also reject a wrong incarnation even when a sequence happens to match.
            modified = original;
            modified.identity.incarnation = [0xEE; 16];
            match target {
                TransitionTarget::Enable => {
                    assert!(core.ack_activated(&mut disk, modified).is_err())
                }
                TransitionTarget::Pause => assert!(core
                    .ack_quiesced(&mut disk, modified, dependencies(7))
                    .is_err()),
                TransitionTarget::Retire
                | TransitionTarget::AddService(_)
                | TransitionTarget::RemoveService(_) => unreachable!(),
            }
        }
    }
}

#[test]
fn enabled_identity_can_accept_offline_durable_work_before_runtime_restoration() {
    let (mut disk, _, identity) = seeded();
    let enable = transition(&disk, identity, TransitionTarget::Enable);
    transition_attempt(
        &mut disk,
        &mut Runtime::default(),
        &mut None,
        enable,
        dependencies(7),
        false,
    )
    .unwrap();
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let hash = core.identity(identity).unwrap().public_hash;
    let offline = core
        .admit(identity, hash, AdmissionClass::ServiceMutation)
        .unwrap();
    assert_eq!(core.validate_admission(offline), Ok(()));
    assert_eq!(
        core.admit(identity, hash, AdmissionClass::Network),
        Err(Fault::RuntimeUnavailable)
    );
    assert_eq!(
        core.runtime_status(identity),
        Ok(RuntimeStatus::Unavailable)
    );

    transition_in_session(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::Pause,
        dependencies(7),
    );
    assert_eq!(
        core.admit(identity, hash, AdmissionClass::ServiceMutation),
        Err(Fault::Paused)
    );
    assert!(core.validate_admission(offline).is_err());
    assert!(core
        .admit(identity, hash, AdmissionClass::LocalMutation)
        .is_ok());
    assert!(core
        .admit(identity, hash, AdmissionClass::OfflineSigning)
        .is_ok());

    transition_in_session(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::Retire,
        dependencies(7),
    );
    for class in [
        AdmissionClass::ServiceMutation,
        AdmissionClass::Network,
        AdmissionClass::LocalMutation,
        AdmissionClass::OfflineSigning,
    ] {
        assert_eq!(core.admit(identity, hash, class), Err(Fault::Retired));
    }
    assert!(core.admit(identity, hash, AdmissionClass::History).is_ok());
    assert!(core.admit(identity, hash, AdmissionClass::Recovery).is_ok());
}

#[test]
fn concurrent_import_of_a_just_published_key_resolves_existing_identity_exactly() {
    for failure in [None, Some(Failure::BeforeWrite), Some(Failure::AfterWrite)] {
        let mut disk = Disk::new(State::new(INCARNATION));
        let mut working = State::new(INCARNATION);
        let mut staging = State::new(INCARNATION);
        let mut vault = Vault::default();
        let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
        let original_command = create("Original");
        let original_token = core.reserve(&mut disk, original_command, DIGEST).unwrap();
        core.begin(&mut disk, original_token, &original_command)
            .unwrap();
        let original_action = core.action(original_token).unwrap().unwrap();
        let original_key = vault.perform(original_action);

        // Both keys are durable before either publication has resolved. These
        // distinct vault references represent the same imported key material.
        let import_command = Command::Create(CreateRequest {
            source: KeySource::Import {
                expected_hash: original_key.public_hash,
            },
            metadata: Metadata::new("Import must not rename it", Some("Published hint")).unwrap(),
            services: ServiceSet::new(&[TELEMETRY]).unwrap(),
        });
        let import_token = core
            .reserve(&mut disk, import_command, CommandDigest([0xA1; 32]))
            .unwrap();
        core.begin(&mut disk, import_token, &import_command)
            .unwrap();
        let import_action = core.action(import_token).unwrap().unwrap();
        let imported_key = vault.perform(import_action);
        assert_ne!(original_key.key_reference, imported_key.key_reference);
        assert_eq!(original_key.public_hash, imported_key.public_hash);
        let OperationResult::Created { identity } = core
            .ack_key(&mut disk, original_action.ticket(), original_key)
            .unwrap()
        else {
            panic!("first publication must create the identity")
        };
        let original = *core.identity(identity).unwrap();
        let expected = OperationResult::Existing { identity };
        if let Some(failure) = failure {
            disk.fail_next(failure);
            assert!(matches!(
                core.ack_key(&mut disk, import_action.ticket(), imported_key),
                Err(Error::Storage(_))
            ));
            assert!(core.is_poisoned());
            disk.reboot();
            core.reload(&mut disk).unwrap();
            if let Some(action) = core.action(import_token).unwrap() {
                assert_eq!(
                    core.ack_key(&mut disk, action.ticket(), imported_key)
                        .unwrap(),
                    expected
                );
                assert_eq!(
                    core.ack_key(&mut disk, action.ticket(), imported_key)
                        .unwrap(),
                    expected
                );
            }
        } else {
            assert_eq!(
                core.ack_key(&mut disk, import_action.ticket(), imported_key)
                    .unwrap(),
                expected
            );
            assert_eq!(
                core.ack_key(&mut disk, import_action.ticket(), imported_key)
                    .unwrap(),
                expected
            );
        }
        assert_eq!(
            core.resume(import_token, &import_command),
            Ok(OperationState::Completed(expected))
        );
        assert_eq!(*core.identity(identity).unwrap(), original);
        assert_eq!(core.snapshot().identities().count(), 1);
        assert!(core.action(import_token).unwrap().is_none());
        assert_eq!(vault.keys, vec![original_key, imported_key]);
        assert_eq!(vault.generated, 1);
        assert_eq!(
            core.operation(import_token).unwrap().unpublished_key,
            Some(imported_key.key_reference)
        );
        assert!(matches!(
            core.prune_completed(&mut disk, import_token.sequence),
            Err(Error::Fault(Fault::UnaccountedKey))
        ));
        assert!(matches!(
            core.acknowledge_retained_key(&mut disk, import_token, original_key.key_reference),
            Err(Error::Fault(Fault::KeyMismatch))
        ));

        // The adapter independently persists ownership of the extra imported
        // reference before releasing the catalog's pruning fence. No deletion.
        vault.accounted_extra_keys.push(imported_key.key_reference);
        if let Some(failure) = failure {
            disk.fail_next(failure);
            assert!(matches!(
                core.acknowledge_retained_key(&mut disk, import_token, imported_key.key_reference),
                Err(Error::Storage(_))
            ));
            assert!(core.is_poisoned());
            disk.reboot();
            core.reload(&mut disk).unwrap();
        }
        core.acknowledge_retained_key(&mut disk, import_token, imported_key.key_reference)
            .unwrap();
        core.acknowledge_retained_key(&mut disk, import_token, imported_key.key_reference)
            .unwrap();
        assert_eq!(core.operation(import_token).unwrap().unpublished_key, None);
        core.prune_completed(&mut disk, import_token.sequence)
            .unwrap();
        assert_eq!(
            core.resume(import_token, &import_command),
            Err(Fault::ReplayExpired)
        );
        assert_eq!(vault.keys, vec![original_key, imported_key]);
        assert_eq!(vault.accounted_extra_keys, vec![imported_key.key_reference]);
    }
}

#[test]
fn generated_key_collision_is_a_provider_error_not_an_implicit_import() {
    let (mut disk, _, identity) = seeded();
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let original = *core.identity(identity).unwrap();
    let command = create("Different identity");
    let token = core.reserve(&mut disk, command, DIGEST).unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    let action = core.action(token).unwrap().unwrap();
    let Action::PublishKey { key_reference, .. } = action else {
        unreachable!()
    };
    assert!(matches!(
        core.ack_key(
            &mut disk,
            action.ticket(),
            VerifiedKey {
                key_reference,
                public_hash: original.public_hash,
            }
        ),
        Err(Error::Fault(Fault::KeyMismatch))
    ));
    assert_eq!(*core.identity(identity).unwrap(), original);
    assert_eq!(core.snapshot().identities().count(), 1);
    assert_eq!(core.action(token).unwrap(), Some(action));
}

#[test]
fn failed_session_publication_requires_a_successful_reopen() {
    for failure in [Failure::BeforeWrite, Failure::AfterWrite] {
        let mut disk = Disk::new(State::new(INCARNATION));
        let mut working = State::new(INCARNATION);
        let mut staging = State::new(INCARNATION);
        disk.fail_next(failure);
        assert!(Coordinator::open(&mut disk, &mut working, &mut staging).is_err());

        // The adapter may have committed despite its error. Recovery reads its
        // durable state rather than retrying an assumed old revision.
        disk.reboot();
        let recovered = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
        assert!(!recovered.is_poisoned());
    }
}

#[test]
fn activation_and_restoration_cleanup_recover_both_sides_of_each_durable_write() {
    for restoration in [false, true] {
        for failed_step in 0..2 {
            for failure in [Failure::BeforeWrite, Failure::AfterWrite] {
                let (mut disk, _, identity) = seeded();
                let mut working = State::new(INCARNATION);
                let mut staging = State::new(INCARNATION);
                let mut runtime = Runtime::default();
                let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
                let token = if restoration {
                    transition_in_session(
                        &mut core,
                        &mut disk,
                        identity,
                        TransitionTarget::Enable,
                        dependencies(7),
                    );
                    core.reload(&mut disk).unwrap();
                    None
                } else {
                    let command = Command::Transition(
                        core.prepare(identity, TransitionTarget::Enable, dependencies(7))
                            .unwrap(),
                    );
                    let token = core.reserve(&mut disk, command, DIGEST).unwrap();
                    core.begin(&mut disk, token, &command).unwrap();
                    Some(token)
                };
                let action = pending_runtime_action(&core, identity, token).unwrap();
                runtime.perform(action);
                if failed_step == 0 {
                    disk.fail_next(failure);
                    assert!(matches!(
                        core.activation_failed(
                            &mut disk,
                            action.ticket(),
                            ServiceFailure::StorageUnavailable
                        ),
                        Err(Error::Storage(_))
                    ));
                    assert!(core.is_poisoned());
                    disk.reboot();
                    core.reload(&mut disk).unwrap();
                    let recovered = pending_runtime_action(&core, identity, token).unwrap();
                    if matches!(recovered, Action::Activate { .. }) {
                        core.activation_failed(
                            &mut disk,
                            recovered.ticket(),
                            ServiceFailure::StorageUnavailable,
                        )
                        .unwrap();
                    }
                } else {
                    core.activation_failed(
                        &mut disk,
                        action.ticket(),
                        ServiceFailure::StorageUnavailable,
                    )
                    .unwrap();
                }
                let cleanup = pending_runtime_action(&core, identity, token).unwrap();
                assert!(matches!(cleanup, Action::Quiesce { .. }));
                runtime.perform(cleanup);
                if failed_step == 1 {
                    disk.fail_next(failure);
                    assert!(matches!(
                        ack_cleanup(&mut core, &mut disk, cleanup),
                        Err(Error::Storage(_))
                    ));
                    assert!(core.is_poisoned());
                    disk.reboot();
                    core.reload(&mut disk).unwrap();
                    if let Some(recovered @ Action::Quiesce { .. }) =
                        pending_runtime_action(&core, identity, token)
                    {
                        runtime.perform(recovered);
                        ack_cleanup(&mut core, &mut disk, recovered).unwrap();
                    }
                } else {
                    ack_cleanup(&mut core, &mut disk, cleanup).unwrap();
                }
                let record = core.identity(identity).unwrap();
                assert!(record.pending.is_none());
                assert!(record.recovery_cleanup.is_none());
                assert_eq!(
                    record.desired,
                    if restoration {
                        DesiredState::Enabled
                    } else {
                        DesiredState::Paused
                    }
                );
                assert_eq!(
                    core.runtime_status(identity),
                    Ok(RuntimeStatus::Unavailable)
                );
                if let Some(token) = token {
                    assert_eq!(
                        core.operation(token).unwrap().state,
                        OperationState::Completed(OperationResult::ActivationFailed {
                            identity,
                            reason: ServiceFailure::StorageUnavailable,
                        })
                    );
                }
                assert!(runtime.active.is_empty());
                assert_eq!(runtime.management_key_uses, 1);
            }
        }
    }
}

fn pending_runtime_action(
    core: &Core<'_>,
    identity: LocalIdentityId,
    token: Option<OperationToken>,
) -> Option<Action<2>> {
    match token {
        Some(token) => core.action(token).unwrap(),
        None => core.recovery_action(identity).unwrap(),
    }
}

fn ack_cleanup(
    core: &mut Core<'_>,
    disk: &mut Disk,
    action: Action<2>,
) -> Result<(), Error<Failure>> {
    if action.ticket().kind == EffectKind::RestoreCleanup {
        core.ack_recovery_quiesced(disk, action.ticket(), dependencies(7))
    } else {
        core.ack_quiesced(disk, action.ticket(), dependencies(7))
            .map(|_| ())
    }
}

#[test]
fn failed_catalog_read_cannot_open_a_runtime() {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    disk.fail_read = true;
    assert!(Coordinator::open(&mut disk, &mut working, &mut staging).is_err());
    assert_eq!(disk.writes, 0);
    disk.reboot();
    assert!(Coordinator::open(&mut disk, &mut working, &mut staging).is_ok());
}

#[test]
fn pinned_pending_publication_cannot_consume_two_enabled_identities_stop_capacity() {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut vault = Vault::default();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let unfinished_command = create("Unfinished publication");
    let unfinished = core.reserve(&mut disk, unfinished_command, DIGEST).unwrap();
    core.begin(&mut disk, unfinished, &unfinished_command)
        .unwrap();
    let original_key_action = core.action(unfinished).unwrap().unwrap();

    let (_, first) = create_in_session(&mut core, &mut disk, &mut vault, "First");
    let (_, second) = create_in_session(&mut core, &mut disk, &mut vault, "Second");
    let (_, paused) = create_in_session(&mut core, &mut disk, &mut vault, "Paused");
    for identity in [first, second] {
        transition_in_session(
            &mut core,
            &mut disk,
            identity,
            TransitionTarget::Enable,
            dependencies(7),
        );
    }
    loop {
        match core.reserve(&mut disk, create("Completed later operation"), DIGEST) {
            Ok(token) => core.cancel_reserved(&mut disk, token).unwrap(),
            Err(Error::Fault(Fault::CapacityExceeded)) => break,
            Err(other) => panic!("unexpected ordinary capacity failure: {other:?}"),
        }
    }
    assert_eq!(core.snapshot().operations().count(), 14);
    assert!(matches!(
        core.prune_completed(&mut disk, core.snapshot().issued_sequence),
        Err(Error::Fault(Fault::CannotPrunePending))
    ));

    let first_pause = core
        .prepare(first, TransitionTarget::Pause, dependencies(7))
        .unwrap();
    let second_pause = core
        .prepare(second, TransitionTarget::Pause, dependencies(7))
        .unwrap();
    let paused_stop = core
        .prepare(paused, TransitionTarget::Pause, dependencies(7))
        .unwrap();
    assert!(core.apply_stop(&mut disk, paused_stop, DIGEST).is_err());
    let mut stale = first_pause;
    stale.identity.lifecycle_revision += 1;
    assert!(core.apply_stop(&mut disk, stale, DIGEST).is_err());
    // Inert reservations cannot occupy protected slots: losing one would pin
    // prefix pruning and could strand a real stop after another lifecycle change.
    assert!(core
        .reserve(&mut disk, Command::Transition(first_pause), DIGEST)
        .is_err());
    assert_eq!(core.snapshot().operations().count(), 14);
    let first_stop = core.apply_stop(&mut disk, first_pause, DIGEST).unwrap();
    assert_eq!(
        core.apply_stop(&mut disk, first_pause, DIGEST).unwrap(),
        first_stop
    );
    assert_eq!(core.snapshot().operations().count(), 15);
    let second_stop = core.apply_stop(&mut disk, second_pause, DIGEST).unwrap();
    assert_eq!(core.snapshot().operations().count(), 16);
    for (token, command) in [(first_stop, first_pause), (second_stop, second_pause)] {
        assert_eq!(
            core.resume(token, &Command::Transition(command)),
            Ok(OperationState::Transitioning)
        );
        let action = core.action(token).unwrap().unwrap();
        assert_eq!(core.validate_effect(action.ticket()), Ok(()));
        core.ack_quiesced(&mut disk, action.ticket(), dependencies(7))
            .unwrap();
    }
    assert_eq!(core.identity(first).unwrap().desired, DesiredState::Paused);
    assert_eq!(core.identity(second).unwrap().desired, DesiredState::Paused);
    assert_eq!(core.action(unfinished).unwrap(), Some(original_key_action));
    assert!(matches!(
        core.prune_completed(&mut disk, core.snapshot().issued_sequence),
        Err(Error::Fault(Fault::CannotPrunePending))
    ));

    // Prefix cleanup remains conservative. Only settling the oldest original
    // publication releases later results; no out-of-order result pruning.
    let key = vault.perform(original_key_action);
    core.ack_key(&mut disk, original_key_action.ticket(), key)
        .unwrap();
    let through = core.snapshot().issued_sequence;
    core.prune_completed(&mut disk, through).unwrap();
    assert_eq!(core.snapshot().operations().count(), 0);
    assert_eq!(core.snapshot().completed_floor, through);
    assert_eq!(core.snapshot().identities().count(), 4);
}

#[test]
fn enabling_requires_capacity_for_its_later_stop_before_registering_services() {
    let (mut disk, _, identity) = seeded();
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    while core.snapshot().operations().count() < 15 {
        let token = core
            .reserve(&mut disk, create("Finished reservation"), DIGEST)
            .unwrap();
        core.cancel_reserved(&mut disk, token).unwrap();
    }
    let enable = Command::Transition(
        core.prepare(identity, TransitionTarget::Enable, dependencies(7))
            .unwrap(),
    );
    assert!(matches!(
        core.reserve(&mut disk, enable, DIGEST),
        Err(Error::Fault(Fault::CapacityExceeded))
    ));
    assert_eq!(
        core.identity(identity).unwrap().desired,
        DesiredState::Paused
    );
    assert!(core.recovery_action(identity).unwrap().is_none());
    assert_eq!(core.snapshot().operations().count(), 15);
}

#[test]
fn atomic_stop_at_capacity_recovers_exactly_after_before_or_after_commit_error() {
    for target in [TransitionTarget::Pause, TransitionTarget::Retire] {
        for failure in [Failure::BeforeWrite, Failure::AfterWrite] {
            let (mut disk, _, identity) = seeded();
            let mut working = State::new(INCARNATION);
            let mut staging = State::new(INCARNATION);
            let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
            transition_in_session(
                &mut core,
                &mut disk,
                identity,
                TransitionTarget::Enable,
                dependencies(7),
            );
            loop {
                match core.reserve(&mut disk, create("Later completed result"), DIGEST) {
                    Ok(token) => core.cancel_reserved(&mut disk, token).unwrap(),
                    Err(Error::Fault(Fault::CapacityExceeded)) => break,
                    Err(error) => panic!("unexpected general admission error: {error:?}"),
                }
            }
            assert_eq!(core.snapshot().operations().count(), 15);
            let prepared = core.prepare(identity, target, dependencies(7)).unwrap();
            let digest = CommandDigest([0xC1; 32]);
            disk.fail_next(failure);
            assert!(matches!(
                core.apply_stop(&mut disk, prepared, digest),
                Err(Error::Storage(_))
            ));
            assert!(core.is_poisoned());
            disk.reboot();
            core.reload(&mut disk).unwrap();
            let token = core.apply_stop(&mut disk, prepared, digest).unwrap();
            assert_eq!(core.snapshot().operations().count(), 16);
            assert_eq!(core.apply_stop(&mut disk, prepared, digest).unwrap(), token);
            assert!(matches!(
                core.cancel_reserved(&mut disk, token),
                Err(Error::Fault(Fault::AlreadyStarted))
            ));
            let action = core.action(token).unwrap().unwrap();
            assert!(matches!(action, Action::Quiesce { .. }));
            assert_eq!(core.validate_effect(action.ticket()), Ok(()));
            let result = core
                .ack_quiesced(&mut disk, action.ticket(), dependencies(7))
                .unwrap();
            assert_eq!(
                result,
                match target {
                    TransitionTarget::Pause => OperationResult::Paused { identity },
                    TransitionTarget::Retire => OperationResult::Retired { identity },
                    TransitionTarget::Enable
                    | TransitionTarget::AddService(_)
                    | TransitionTarget::RemoveService(_) => unreachable!(),
                }
            );
            let settled = core.snapshot().clone();
            assert_eq!(core.apply_stop(&mut disk, prepared, digest).unwrap(), token);
            assert_eq!(core.snapshot(), &settled);
            assert_eq!(
                core.resume(token, &Command::Transition(prepared)),
                Ok(OperationState::Completed(result))
            );
        }
    }
}

#[test]
fn stale_inert_transition_cannot_strand_stop_capacity_after_restoration_changes_revision() {
    let (mut disk, _, identity) = seeded();
    let mut working = State::new(INCARNATION);
    let mut staging = State::new(INCARNATION);
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    transition_in_session(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::Enable,
        dependencies(7),
    );
    let old_command = Command::Transition(
        core.prepare(identity, TransitionTarget::Pause, dependencies(7))
            .unwrap(),
    );
    let old_token = core.reserve(&mut disk, old_command, DIGEST).unwrap();
    assert!(matches!(
        core.reserve(&mut disk, old_command, DIGEST),
        Err(Error::Fault(Fault::Busy))
    ));
    loop {
        match core.reserve(&mut disk, create("Later completed operation"), DIGEST) {
            Ok(token) => core.cancel_reserved(&mut disk, token).unwrap(),
            Err(Error::Fault(Fault::CapacityExceeded)) => break,
            Err(error) => panic!("unexpected capacity error: {error:?}"),
        }
    }
    assert_eq!(core.snapshot().operations().count(), 15);
    core.reload(&mut disk).unwrap();
    let failed_restore = core.recovery_action(identity).unwrap().unwrap();
    core.activation_failed(
        &mut disk,
        failed_restore.ticket(),
        ServiceFailure::Unavailable(1),
    )
    .unwrap();
    let cleanup = core.recovery_action(identity).unwrap().unwrap();
    core.ack_recovery_quiesced(&mut disk, cleanup.ticket(), dependencies(8))
        .unwrap();
    assert!(matches!(
        core.begin(&mut disk, old_token, &old_command),
        Err(Error::Fault(Fault::StaleLifecycle))
    ));
    let stop = core
        .prepare(identity, TransitionTarget::Pause, dependencies(8))
        .unwrap();
    let token = core
        .apply_stop(&mut disk, stop, CommandDigest([0xD1; 32]))
        .unwrap();
    assert_ne!(token, old_token);
    assert_eq!(
        core.resume(old_token, &old_command),
        Ok(OperationState::Completed(OperationResult::Cancelled))
    );
    assert_eq!(core.snapshot().operations().count(), 16);
    let action = core.action(token).unwrap().unwrap();
    core.ack_quiesced(&mut disk, action.ticket(), dependencies(8))
        .unwrap();
    assert_eq!(
        core.identity(identity).unwrap().desired,
        DesiredState::Paused
    );
}

#[test]
fn checkpoint_preserves_session_readiness_and_authority_only_after_success() {
    for failure in [None, Some(Failure::BeforeWrite), Some(Failure::AfterWrite)] {
        let mut disk = Disk::new(State::new(INCARNATION));
        let mut working = State::new(INCARNATION);
        let mut staging = working.clone();
        let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
        let (_, identity) =
            create_in_session(&mut core, &mut disk, &mut Vault::default(), "Weather");
        transition_in_session(
            &mut core,
            &mut disk,
            identity,
            TransitionTarget::Enable,
            dependencies(7),
        );
        let hash = core.identity(identity).unwrap().public_hash;
        let guard = core.admit(identity, hash, AdmissionClass::Network).unwrap();
        let before = core.snapshot().clone();
        if let Some(failure) = failure {
            disk.fail_next(failure);
        }
        let outcome = core.checkpoint(&mut disk);
        match failure {
            None => {
                outcome.unwrap();
                let mut expected = before.clone();
                expected.revision += 1;
                assert_eq!(core.snapshot(), &expected);
                assert_eq!(disk.catalog, expected);
                assert_eq!(core.validate_admission(guard), Ok(()));
                assert_eq!(
                    core.admit(identity, hash, AdmissionClass::Network),
                    Ok(guard)
                );
            }
            Some(failure) => {
                assert_eq!(outcome, Err(Error::Storage(failure)));
                assert!(core.is_poisoned());
                assert_eq!(core.snapshot(), &before);
                assert_eq!(core.validate_admission(guard), Err(Fault::Poisoned));
                assert_eq!(
                    disk.catalog.revision,
                    before.revision + u64::from(failure == Failure::AfterWrite)
                );
                disk.reboot();
                core.reload(&mut disk).unwrap();
                assert!(core.snapshot().session > before.session);
                assert!(core.admit(identity, hash, AdmissionClass::Network).is_err());
            }
        }
    }
}
