#![allow(clippy::unwrap_used, clippy::panic)]
mod support;
use prns_identities_core::*;
use support::{Durable, Failure};

type State = Catalog<2, 16, 2>;
type Disk = Durable<2, 16, 2>;
const INCARNATION: [u8; 16] = [28; 16];
const SENSOR: ServiceId = ServiceId(17);
const ARCHIVE: ServiceId = ServiceId(901);

fn report(services: &[ServiceId]) -> DependencyReport<2> {
    let entries: Vec<_> = services
        .iter()
        .map(|service| ServiceDependency::clear(*service, 1))
        .collect();
    DependencyReport::new(&entries, 0).unwrap()
}
fn seeded(services: &[ServiceId]) -> (Disk, OperationToken, LocalIdentityId, Command<2>) {
    let mut disk = Disk::new(State::new(INCARNATION));
    let mut work = State::new(INCARNATION);
    let mut stage = work.clone();
    let mut core = Coordinator::open(&mut disk, &mut work, &mut stage).unwrap();
    let command = Command::Create(CreateRequest {
        source: KeySource::Generate,
        metadata: Metadata::new("Weather station", None).unwrap(),
        services: ServiceSet::new(services).unwrap(),
    });
    let token = core
        .reserve(&mut disk, command, CommandDigest([1; 32]))
        .unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    let Action::PublishKey {
        ticket,
        key_reference,
        ..
    } = core.action(token).unwrap().unwrap()
    else {
        panic!()
    };
    core.ack_key(
        &mut disk,
        ticket,
        VerifiedKey {
            key_reference,
            public_hash: PublicIdentityHash([2; 16]),
        },
    )
    .unwrap();
    (disk, token, ticket.identity, command)
}
fn begin(
    core: &mut Coordinator<'_, 2, 16, 2>,
    disk: &mut Disk,
    id: LocalIdentityId,
    target: TransitionTarget,
    dependencies: DependencyReport<2>,
) -> OperationToken {
    let prepared = core.prepare(id, target, dependencies).unwrap();
    let command = Command::Transition(prepared);
    let token = core.reserve(disk, command, CommandDigest([3; 32])).unwrap();
    core.begin(disk, token, &command).unwrap();
    token
}
fn initialize(
    core: &mut Coordinator<'_, 2, 16, 2>,
    disk: &mut Disk,
    token: OperationToken,
) -> OperationResult {
    let Action::InitializeService {
        ticket, service, ..
    } = core.action(token).unwrap().unwrap()
    else {
        panic!()
    };
    core.ack_service_initialized(disk, InitializedService { ticket, service })
        .unwrap()
}

#[test]
fn add_then_remove_preserves_creation_provenance_and_never_enables() {
    let (mut disk, created, identity, creation) = seeded(&[]);
    let mut work = State::new(INCARNATION);
    let mut stage = work.clone();
    let mut core = Coordinator::open(&mut disk, &mut work, &mut stage).unwrap();
    let add = begin(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::AddService(SENSOR),
        report(&[]),
    );
    let quiesce = core.action(add).unwrap().unwrap().ticket();
    assert_eq!(
        core.ack_configuration_quiesced(&mut disk, quiesce, report(&[]))
            .unwrap(),
        OperationState::Transitioning
    );
    assert_eq!(
        core.identity(identity).unwrap().services,
        ServiceSet::empty()
    );
    assert_eq!(
        core.prepare(identity, TransitionTarget::Enable, report(&[])),
        Err(Fault::Busy)
    );
    assert_eq!(
        initialize(&mut core, &mut disk, add),
        OperationResult::ServiceAdded {
            identity,
            service: SENSOR
        }
    );
    assert_eq!(
        core.identity(identity).unwrap().services,
        ServiceSet::new(&[SENSOR]).unwrap()
    );
    assert_eq!(
        core.identity(identity).unwrap().initial_services,
        ServiceSet::empty()
    );
    assert_eq!(
        core.identity(identity).unwrap().desired,
        DesiredState::Paused
    );
    assert_eq!(
        core.resume(created, &creation).unwrap(),
        OperationState::Completed(OperationResult::Created { identity })
    );
    let remove = begin(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::RemoveService(SENSOR),
        report(&[SENSOR]),
    );
    let action = core.action(remove).unwrap().unwrap();
    assert_eq!(
        core.ack_configuration_quiesced(&mut disk, action.ticket(), report(&[SENSOR]))
            .unwrap(),
        OperationState::Completed(OperationResult::ServiceRemoved {
            identity,
            service: SENSOR
        })
    );
    assert_eq!(
        core.identity(identity).unwrap().services,
        ServiceSet::empty()
    );
    assert_eq!(
        core.identity(identity).unwrap().desired,
        DesiredState::Paused
    );
    assert_eq!(
        core.resume(created, &creation).unwrap(),
        OperationState::Completed(OperationResult::Created { identity })
    );
    validate_catalog(core.snapshot()).unwrap();
}

#[test]
fn duplicate_absent_capacity_and_unavailable_reports_fail_before_effects() {
    let (mut disk, _, identity, _) = seeded(&[SENSOR, ARCHIVE]);
    let mut work = State::new(INCARNATION);
    let mut stage = work.clone();
    let mut core = Coordinator::open(&mut disk, &mut work, &mut stage).unwrap();
    let before = core.snapshot().clone();
    assert_eq!(
        core.prepare(
            identity,
            TransitionTarget::AddService(SENSOR),
            report(&[SENSOR, ARCHIVE])
        ),
        Err(Fault::InvalidServices)
    );
    assert_eq!(
        core.prepare(
            identity,
            TransitionTarget::AddService(ServiceId(77)),
            report(&[SENSOR, ARCHIVE])
        ),
        Err(Fault::CapacityExceeded)
    );
    assert_eq!(
        core.prepare(
            identity,
            TransitionTarget::RemoveService(ServiceId(77)),
            report(&[SENSOR, ARCHIVE])
        ),
        Err(Fault::InvalidServices)
    );
    assert_eq!(
        core.prepare(
            identity,
            TransitionTarget::RemoveService(ARCHIVE),
            report(&[SENSOR])
        ),
        Err(Fault::InvalidReport)
    );
    let forged = Command::Transition(PreparedTransition {
        identity: core.identity(identity).unwrap().guard(),
        target: TransitionTarget::AddService(SENSOR),
        review: report(&[SENSOR, ARCHIVE]),
    });
    assert_eq!(
        core.reserve(&mut disk, forged, CommandDigest([8; 32])),
        Err(Error::Fault(Fault::InvalidServices))
    );
    assert_eq!(core.snapshot(), &before);
}

#[test]
fn removal_requires_actual_removed_obligations_and_cannot_ignore_unknown_service() {
    let (mut disk, _, identity, _) = seeded(&[SENSOR, ARCHIVE]);
    let mut work = State::new(INCARNATION);
    let mut stage = work.clone();
    let mut core = Coordinator::open(&mut disk, &mut work, &mut stage).unwrap();
    let mut blocked = report(&[SENSOR, ARCHIVE]);
    blocked.entries[1].as_mut().unwrap().pending_messages = 1;
    let token = begin(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::RemoveService(ARCHIVE),
        blocked,
    );
    let ticket = core.action(token).unwrap().unwrap().ticket();
    assert_eq!(
        core.ack_configuration_quiesced(&mut disk, ticket, report(&[SENSOR])),
        Err(Error::Fault(Fault::InvalidReport))
    );
    assert!(core.identity(identity).unwrap().pending.is_some());
    assert_eq!(
        core.ack_configuration_quiesced(&mut disk, ticket, blocked)
            .unwrap(),
        OperationState::Completed(OperationResult::ServiceRemovalBlocked {
            identity,
            service: ARCHIVE,
            reason: RetirementBlock::PendingMessages
        })
    );
    assert_eq!(
        core.identity(identity).unwrap().services,
        ServiceSet::new(&[SENSOR, ARCHIVE]).unwrap()
    );

    let mut dependencies = report(&[SENSOR, ARCHIVE]);
    dependencies.entries[0].as_mut().unwrap().pending_messages = 9;
    let token = begin(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::RemoveService(ARCHIVE),
        dependencies,
    );
    let ticket = core.action(token).unwrap().unwrap().ticket();
    assert_eq!(
        core.ack_configuration_quiesced(&mut disk, ticket, dependencies)
            .unwrap(),
        OperationState::Completed(OperationResult::ServiceRemoved {
            identity,
            service: ARCHIVE
        })
    );
    assert_eq!(
        core.identity(identity).unwrap().services,
        ServiceSet::new(&[SENSOR]).unwrap()
    );
}

#[derive(Default)]
struct Domain {
    prepared: Option<(LocalIdentityId, ServiceId, OperationToken)>,
    writes: usize,
    unavailable: bool,
}
impl Domain {
    fn initialize(
        &mut self,
        ticket: EffectTicket,
        service: ServiceId,
    ) -> Result<InitializedService, ()> {
        if self.unavailable {
            return Err(());
        }
        let binding = (ticket.identity, service, ticket.operation.unwrap());
        if let Some(existing) = self.prepared {
            if existing != binding {
                return Err(());
            }
        } else {
            self.prepared = Some(binding);
            self.writes += 1;
        }
        Ok(InitializedService { ticket, service })
    }
}
fn attempt(
    disk: &mut Disk,
    domain: &mut Domain,
    caller: &mut Option<OperationToken>,
    prepared: PreparedTransition<2>,
    lose_reply: bool,
) -> Result<OperationResult, ()> {
    let mut work = State::new(INCARNATION);
    let mut stage = work.clone();
    let mut core = Coordinator::open(disk, &mut work, &mut stage).map_err(|_| ())?;
    let command = Command::Transition(prepared);
    let token = match *caller {
        Some(token) => token,
        None => {
            let recovered = core
                .snapshot()
                .operations()
                .find(|op| op.command == command && op.token.digest == CommandDigest([22; 32]))
                .map(|op| op.token);
            let token = match recovered {
                Some(token) => token,
                None => core
                    .reserve(disk, command, CommandDigest([22; 32]))
                    .map_err(|_| ())?,
            };
            *caller = Some(token);
            token
        }
    };
    core.begin(disk, token, &command).map_err(|_| ())?;
    while let Some(action) = core.action(token).map_err(|_| ())? {
        match action {
            Action::Quiesce { ticket, .. } => {
                core.ack_configuration_quiesced(disk, ticket, prepared.review)
                    .map_err(|_| ())?;
            }
            Action::InitializeService {
                ticket, service, ..
            } => {
                let ready = domain.initialize(ticket, service)?;
                if lose_reply {
                    return Err(());
                }
                core.ack_service_initialized(disk, ready).map_err(|_| ())?;
            }
            _ => return Err(()),
        }
    }
    let OperationState::Completed(result) = core.resume(token, &command).map_err(|_| ())? else {
        return Err(());
    };
    Ok(result)
}

#[test]
fn domain_commit_reply_loss_and_missing_adapter_keep_the_exact_pending_addition() {
    let (mut disk, _, identity, _) = seeded(&[]);
    let prepared = PreparedTransition {
        identity: disk.catalog.identities().next().unwrap().guard(),
        target: TransitionTarget::AddService(SENSOR),
        review: report(&[]),
    };
    let mut domain = Domain::default();
    let mut caller = None;
    assert!(attempt(&mut disk, &mut domain, &mut caller, prepared, true).is_err());
    let first = domain.prepared;
    assert_eq!(domain.writes, 1);
    domain.unavailable = true;
    assert!(attempt(&mut disk, &mut domain, &mut caller, prepared, false).is_err());
    let identity_record = disk.catalog.identities().next().unwrap();
    assert_eq!(identity_record.services, ServiceSet::empty());
    assert_eq!(
        identity_record.pending.unwrap().phase,
        TransitionPhase::InitializingService
    );
    domain.unavailable = false;
    assert_eq!(
        attempt(&mut disk, &mut domain, &mut caller, prepared, false).unwrap(),
        OperationResult::ServiceAdded {
            identity,
            service: SENSOR
        }
    );
    assert_eq!(domain.prepared, first);
    assert_eq!(domain.writes, 1);
}

#[test]
fn addition_survives_before_and_after_each_catalog_commit() {
    let (baseline, _, identity, _) = seeded(&[]);
    let prepared = PreparedTransition {
        identity: baseline.catalog.identities().next().unwrap().guard(),
        target: TransitionTarget::AddService(SENSOR),
        review: report(&[]),
    };
    let mut complete = baseline.clone();
    attempt(
        &mut complete,
        &mut Domain::default(),
        &mut None,
        prepared,
        false,
    )
    .unwrap();
    for write in (baseline.writes + 1)..=complete.writes {
        for failure in [Failure::BeforeWrite, Failure::AfterWrite] {
            let mut disk = baseline.clone();
            let mut domain = Domain::default();
            let mut caller = None;
            disk.fail_write = Some((write, failure));
            assert!(attempt(&mut disk, &mut domain, &mut caller, prepared, false).is_err());
            if disk
                .catalog
                .identities()
                .next()
                .unwrap()
                .services
                .iter()
                .any(|id| id == SENSOR)
            {
                assert!(domain.prepared.is_some());
            }
            disk.reboot();
            assert_eq!(
                attempt(&mut disk, &mut domain, &mut caller, prepared, false).unwrap(),
                OperationResult::ServiceAdded {
                    identity,
                    service: SENSOR
                }
            );
            assert_eq!(domain.writes, 1);
            assert_eq!(
                disk.catalog.identities().next().unwrap().desired,
                DesiredState::Paused
            );
            validate_catalog(&disk.catalog).unwrap();
        }
    }
}

#[test]
fn removal_survives_before_and_after_each_catalog_commit() {
    let (baseline, _, identity, _) = seeded(&[SENSOR]);
    let prepared = PreparedTransition {
        identity: baseline.catalog.identities().next().unwrap().guard(),
        target: TransitionTarget::RemoveService(SENSOR),
        review: report(&[SENSOR]),
    };
    let mut complete = baseline.clone();
    attempt(
        &mut complete,
        &mut Domain::default(),
        &mut None,
        prepared,
        false,
    )
    .unwrap();
    for write in (baseline.writes + 1)..=complete.writes {
        for failure in [Failure::BeforeWrite, Failure::AfterWrite] {
            let mut disk = baseline.clone();
            let mut caller = None;
            disk.fail_write = Some((write, failure));
            assert!(attempt(
                &mut disk,
                &mut Domain::default(),
                &mut caller,
                prepared,
                false
            )
            .is_err());
            disk.reboot();
            assert_eq!(
                attempt(
                    &mut disk,
                    &mut Domain::default(),
                    &mut caller,
                    prepared,
                    false
                )
                .unwrap(),
                OperationResult::ServiceRemoved {
                    identity,
                    service: SENSOR
                }
            );
            assert_eq!(
                disk.catalog.identities().next().unwrap().services,
                ServiceSet::empty()
            );
            validate_catalog(&disk.catalog).unwrap();
        }
    }
}

#[test]
fn initialization_witness_is_service_scoped_and_uncertain_ack_requires_reload() {
    let (mut disk, _, identity, _) = seeded(&[]);
    let mut work = State::new(INCARNATION);
    let mut stage = work.clone();
    let mut core = Coordinator::open(&mut disk, &mut work, &mut stage).unwrap();
    let token = begin(
        &mut core,
        &mut disk,
        identity,
        TransitionTarget::AddService(SENSOR),
        report(&[]),
    );
    let quiesce = core.action(token).unwrap().unwrap().ticket();
    core.ack_configuration_quiesced(&mut disk, quiesce, report(&[]))
        .unwrap();
    let ticket = core.action(token).unwrap().unwrap().ticket();
    assert_eq!(
        core.ack_service_initialized(
            &mut disk,
            InitializedService {
                ticket,
                service: ARCHIVE
            }
        ),
        Err(Error::Fault(Fault::StaleEffect))
    );
    disk.fail_next(Failure::AfterWrite);
    assert_eq!(
        core.ack_service_initialized(
            &mut disk,
            InitializedService {
                ticket,
                service: SENSOR
            }
        ),
        Err(Error::Storage(Failure::AfterWrite))
    );
    assert!(core.is_poisoned());
    disk.reboot();
    core.reload(&mut disk).unwrap();
    assert_eq!(
        core.ack_service_initialized(
            &mut disk,
            InitializedService {
                ticket,
                service: SENSOR
            }
        ),
        Err(Error::Fault(Fault::StaleSession))
    );
    assert_eq!(
        core.operation(token).unwrap().state,
        OperationState::Completed(OperationResult::ServiceAdded {
            identity,
            service: SENSOR
        })
    );
    assert_eq!(
        core.identity(identity).unwrap().desired,
        DesiredState::Paused
    );
}
