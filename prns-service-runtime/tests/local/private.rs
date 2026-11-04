//! A real selected private settings service, intentionally fixture-only. Its
//! schema, writes, key-owned running resource and delayed drain prove the
//! extension seam without claiming a second production networking capability.
use super::*;
use owned::CommandHandler;
use redb::{Database, ReadableDatabase, ReadableTableMetadata, TableDefinition, WriteTransaction};
use std::sync::{atomic::AtomicBool, mpsc, Mutex};
const SETTINGS: core::ServiceId = core::ServiceId(0x7374_6e67);
const TABLE: TableDefinition<u8, u64> = TableDefinition::new("fixture_private_settings_v1");
fn db_error(error: impl Into<redb::Error>) -> groups::SchemaError {
    groups::SchemaError::Database(error.into())
}
fn initialize(write: &WriteTransaction) -> Result<(), groups::SchemaError> {
    let mut table = write.open_table(TABLE).map_err(db_error)?;
    if !table.is_empty().map_err(db_error)? {
        return Err(group::Error::Corrupt.into());
    }
    table.insert(0, 0).map_err(db_error)?;
    table.insert(1, 0).map_err(db_error)?;
    Ok(())
}
fn read(database: &Database) -> Result<(u64, u64), groups::SchemaError> {
    let tx = database.begin_read().map_err(db_error)?;
    let table = tx.open_table(TABLE).map_err(db_error)?;
    if table.len().map_err(db_error)? != 2 {
        return Err(group::Error::Corrupt.into());
    }
    Ok((
        table
            .get(0)
            .map_err(db_error)?
            .ok_or(group::Error::Corrupt)?
            .value(),
        table
            .get(1)
            .map_err(db_error)?
            .ok_or(group::Error::Corrupt)?
            .value(),
    ))
}
fn reconcile(database: &Database) -> Result<(), groups::SchemaError> {
    let tx = database.begin_read().map_err(db_error)?;
    match tx.open_table(TABLE) {
        Ok(_) => {
            drop(tx);
            read(database).map(|_| ())
        }
        Err(redb::TableError::TableDoesNotExist(_)) => {
            drop(tx);
            let mut write = database.begin_write().map_err(db_error)?;
            write
                .set_durability(redb::Durability::Immediate)
                .map_err(db_error)?;
            initialize(&write)?;
            write.commit().map_err(db_error)
        }
        Err(error) => Err(db_error(error)),
    }
}
// The queue is explicitly bounded; keep its existing 384-byte domain job
// inline instead of adding a per-submission allocation solely for this sum.
#[allow(clippy::large_enum_variant)]
pub enum Job {
    Groups(groups::Command),
    Settings {
        identity: core::IdentityReference,
        value: Option<u64>,
        reply: mpsc::Sender<Result<u64, String>>,
    },
}
struct Controls {
    drain: AtomicBool,
    fail_after_group_prepare: AtomicBool,
    wake: Mutex<Option<Waker>>,
    observation_ready: AtomicBool,
    observed: Mutex<Option<core::IdentityReference>>,
    threads: Mutex<Vec<std::thread::ThreadId>>,
}
impl Controls {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            drain: AtomicBool::new(true),
            fail_after_group_prepare: AtomicBool::new(true),
            wake: Mutex::new(None),
            observation_ready: AtomicBool::new(false),
            observed: Mutex::new(None),
            threads: Mutex::new(Vec::new()),
        })
    }
}
struct Driver {
    groups: groups::Local<4>,
    controls: Arc<Controls>,
    keys: Vec<(rt::OwnerToken, owned::KeyLease)>,
    settings_quiesced: Vec<(rt::OwnerToken, core::IdentityReference)>,
}
impl Driver {
    fn dependency(
        &mut self,
        identity: core::IdentityReference,
    ) -> Result<core::ServiceDependency, groups::Error> {
        let (revision, _) = self
            .groups
            .stores()
            .with_existing(identity, read)
            .map_err(groups::Error::Store)?;
        Ok(core::ServiceDependency::clear(SETTINGS, revision))
    }
    fn command_settings(
        &mut self,
        identity: core::IdentityReference,
        value: Option<u64>,
        admission: &mut dyn owned::Admission,
    ) -> Result<u64, String> {
        let permit = admission
            .acquire(
                identity,
                if value.is_some() {
                    owned::Access::LocalMutation
                } else {
                    owned::Access::History
                },
            )
            .map_err(|error| format!("{error:?}"))?;
        let result: Result<u64, stores::Error<groups::SchemaError>> =
            self.groups.stores().with_existing(identity, |database| {
                let (revision, previous) = read(database)?;
                let Some(value) = value else {
                    return Ok(previous);
                };
                let mut write = database.begin_write().map_err(db_error)?;
                write
                    .set_durability(redb::Durability::Immediate)
                    .map_err(db_error)?;
                {
                    let mut table = write.open_table(TABLE).map_err(db_error)?;
                    table
                        .insert(0, revision.checked_add(1).ok_or(group::Error::StorageFull)?)
                        .map_err(db_error)?;
                    table.insert(1, value).map_err(db_error)?;
                }
                write.commit().map_err(db_error)?;
                Ok(value)
            });
        // The fixture only injects successful writes. A real uncertain commit
        // would need its own retained recovery record before physical settlement.
        if let Ok(value) = result {
            admission
                .settle(permit, rt::Settlement::Committed)
                .map_err(|error| format!("{error:?}"))?;
            Ok(value)
        } else {
            Err(format!("{result:?}"))
        }
    }
}
impl owned::Driver<4> for Driver {
    fn poll_observations(&mut self, cx: &mut Context<'_>) {
        self.groups.poll_observations(cx);
        let identity = *self.controls.observed.lock().unwrap();
        if let Some(identity) = identity {
            if self.groups.dependency(identity).is_ok() {
                self.controls
                    .observation_ready
                    .store(true, Ordering::Release);
            }
        }
    }
    fn activation_plan(
        &self,
        identity: &core::IdentityRecord<4>,
    ) -> Result<rt::ActivationPlan<4>, groups::Error> {
        let mut services = [None; 4];
        for (slot, service) in services.iter_mut().zip(identity.services.iter()) {
            if service != SETTINGS && service != groups::SERVICE {
                return Err(groups::Error::UnsupportedService(service));
            }
            *slot = Some(rt::ServicePlan {
                service,
                resources: rt::Resources::ZERO,
                requires_receive_state: false,
                capabilities: rt::Capabilities {
                    scoped_teardown: true,
                    admission_gate: true,
                    durable_drain: true,
                    durable_receive_state: false,
                },
            });
        }
        Ok(rt::ActivationPlan {
            identity_resources: rt::Resources {
                held_keys: 1,
                ..rt::Resources::ZERO
            },
            services,
        })
    }
    fn dependencies(
        &mut self,
        identity: core::IdentityReference,
        services: core::ServiceSet<4>,
    ) -> Result<core::DependencyReport<4>, groups::Error> {
        let mut report = core::DependencyReport::empty();
        for (slot, service) in report.entries.iter_mut().zip(services.iter()) {
            *slot = Some(if service == SETTINGS {
                self.dependency(identity)?
            } else if service == groups::SERVICE {
                self.groups.dependency(identity)?
            } else {
                return Err(groups::Error::UnsupportedService(service));
            });
        }
        Ok(report)
    }
    fn bind_key(&mut self, owner: rt::OwnerToken, key: owned::KeyLease) {
        self.keys.push((owner, key));
    }
    fn classify_failure(&self, _: &groups::Error) -> core::ServiceFailure {
        core::ServiceFailure::StorageUnavailable
    }
}
impl rt::configuration::ServiceInitializer for Driver {
    type Error = groups::Error;
    fn initialize_service(
        &mut self,
        ticket: core::EffectTicket,
        identity: core::IdentityGuard,
        _: core::KeyReference,
        service: core::ServiceId,
    ) -> Result<core::InitializedService, groups::Error> {
        let reference = core::IdentityReference {
            id: identity.id,
            public_hash: identity.public_hash,
        };
        if service == groups::SERVICE {
            let witness = self.groups.initialize(ticket, identity)?;
            *self.controls.observed.lock().unwrap() = Some(reference);
            // Actual selected schemas committed, but catalog acknowledgement is
            // intentionally lost. Recovery must validate that exact same file.
            if self
                .controls
                .fail_after_group_prepare
                .swap(false, Ordering::AcqRel)
            {
                return Err(groups::Error::NetworkUnavailable);
            }
            Ok(witness)
        } else if service == SETTINGS {
            self.groups
                .stores()
                .prepare(reference, initialize, reconcile)
                .map_err(groups::Error::Store)?;
            Ok(core::InitializedService { ticket, service })
        } else {
            Err(groups::Error::UnsupportedService(service))
        }
    }
}
impl rt::ScopedDriver<4> for Driver {
    type Error = groups::Error;
    fn preflight(
        &mut self,
        scope: rt::Scope,
        plan: &rt::ActivationPlan<4>,
    ) -> Result<rt::Progress<rt::Evidence<rt::Resources>>, groups::Error> {
        if plan
            .services
            .iter()
            .flatten()
            .any(|plan| plan.service == groups::SERVICE)
        {
            return Err(groups::Error::NetworkUnavailable);
        }
        Ok(rt::Progress::Complete(rt::Evidence {
            scope,
            value: plan.total()?,
        }))
    }
    fn prepare_service(
        &mut self,
        scope: rt::Scope,
        _: core::KeyReference,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::PreparedService>>, groups::Error> {
        if plan.service != SETTINGS {
            return Err(groups::Error::NetworkUnavailable);
        }
        self.dependency(core::IdentityReference {
            id: scope.identity.id,
            public_hash: scope.identity.public_hash,
        })?;
        Ok(rt::Progress::Complete(rt::Evidence {
            scope,
            value: rt::PreparedService {
                storage_ready: true,
                receive_state_restored: true,
            },
        }))
    }
    fn activate_service(
        &mut self,
        scope: rt::Scope,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<()>>, groups::Error> {
        if plan.service != SETTINGS {
            return Err(groups::Error::NetworkUnavailable);
        }
        assert!(self.keys.iter().any(|(owner, _)| *owner == scope.owner));
        Ok(rt::Progress::Complete(rt::Evidence { scope, value: () }))
    }
    fn fence_previous_workers(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<bool>>, groups::Error> {
        Ok(rt::Progress::Complete(rt::Evidence { scope, value: true }))
    }
    fn quiesce_service(
        &mut self,
        scope: rt::Scope,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::QuiescedService>>, groups::Error> {
        if plan.service == groups::SERVICE {
            return self.groups.quiesce(scope);
        }
        if plan.service != SETTINGS {
            return Err(groups::Error::UnsupportedService(plan.service));
        }
        if !self.controls.drain.load(Ordering::Acquire) {
            return Ok(rt::Progress::Pending);
        }
        let identity = core::IdentityReference {
            id: scope.identity.id,
            public_hash: scope.identity.public_hash,
        };
        if !self
            .settings_quiesced
            .iter()
            .any(|(owner, _)| *owner == scope.owner)
        {
            self.settings_quiesced.push((scope.owner, identity));
        }
        Ok(rt::Progress::Complete(rt::Evidence {
            scope,
            value: rt::QuiescedService {
                evidence: rt::QuiescenceEvidence::COMPLETE,
                dependencies: self.dependency(identity)?,
            },
        }))
    }
    fn release_identity(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<rt::ReleasedIdentity<4>>>, groups::Error> {
        let mut report = core::DependencyReport::empty();
        let mut next = 0;
        if let Some(value) = self.groups.released_dependency(scope)? {
            report.entries[next] = Some(value);
            next += 1;
        }
        if let Some((_, identity)) = self
            .settings_quiesced
            .iter()
            .find(|(owner, _)| *owner == scope.owner)
        {
            let identity = *identity;
            report.entries[next] = Some(self.dependency(identity)?);
        }
        self.keys.retain(|(owner, _)| *owner != scope.owner);
        Ok(rt::Progress::Complete(rt::Evidence {
            scope,
            value: rt::ReleasedIdentity {
                owned_reservations_released: true,
                shared_owners_preserved: true,
                report,
            },
        }))
    }
}
impl CommandHandler<4, Job> for Driver {
    fn command(&mut self, job: Job, authority: &mut dyn owned::Admission) {
        self.controls
            .threads
            .lock()
            .unwrap()
            .push(std::thread::current().id());
        match job {
            Job::Groups(job) => self.groups.command(job, authority),
            Job::Settings {
                identity,
                value,
                reply,
            } => {
                let _ = reply.send(self.command_settings(identity, value, authority));
            }
        }
    }
    fn reject(job: Job) {
        match job {
            Job::Groups(job) => groups::Local::<4>::reject(job),
            Job::Settings { reply, .. } => {
                let _ = reply.send(Err("closed".into()));
            }
        }
    }
}
fn factory(
    stores: stores::Stores<4>,
    catalog: &core::Catalog<4, 32, 4>,
    wake: Waker,
    controls: Arc<Controls>,
    input: groups::SubmissionState,
) -> Result<Driver, groups::Error> {
    *controls.wake.lock().unwrap() = Some(wake.clone());
    let mut groups = groups::Local::new(stores, options().retention, wake, input)?;
    for identity in catalog.identities() {
        for service in identity.services.iter() {
            if service == groups::SERVICE {
                groups.attach(identity.reference())?;
            } else if service == SETTINGS {
                groups
                    .stores()
                    .with_existing(identity.reference(), read)
                    .map_err(groups::Error::Store)?;
            } else {
                return Err(groups::Error::UnsupportedService(service));
            }
        }
    }
    Ok(Driver {
        groups,
        controls,
        keys: Vec::with_capacity(4),
        settings_quiesced: Vec::with_capacity(4),
    })
}
type PrivateOwner = groups::Owner<4, 32, 4, 16, Driver, Job>;
fn open_private(
    root: &Root,
    create: bool,
    controls: Arc<Controls>,
) -> Result<PrivateOwner, local::Error<groups::Error>> {
    let input = groups::SubmissionState::new::<4>(group::InputBudget::default()).unwrap();
    let worker_input = input.clone();
    let factory = move |stores, catalog: &core::Catalog<4, 32, 4>, wake, _commands| {
        factory(stores, catalog, wake, controls, worker_input)
    };
    let owner = if create {
        local::Owner::create_new(&root.0, options().local, factory)
    } else {
        local::Owner::open_existing(&root.0, options().local, factory)
    }?;
    Ok(PrivateOwner::from_local(owner, input))
}
fn settings(
    commands: &owned::Commands<Job>,
    identity: core::IdentityReference,
    value: Option<u64>,
) -> Result<u64, String> {
    let (tx, rx) = mpsc::channel();
    commands
        .submit(Job::Settings {
            identity,
            value,
            reply: tx,
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}

#[test]
fn two_selected_domains_share_physical_owner_and_retained_initialization_recovery() {
    let root = Root::new();
    let controls = Controls::new();
    let owner = open_private(&root, true, controls.clone()).unwrap();
    let client = owner.identities();
    let identity = create(&client, 1, core::ServiceSet::empty());
    let reference = identity.reference();
    transition(
        &client,
        &identity,
        core::TransitionTarget::AddService(SETTINGS),
    );
    assert_eq!(
        settings(&owner.commands(), reference, Some(42)).unwrap(),
        42
    );
    let (token, prepared) = begin(
        &client,
        &identity,
        core::TransitionTarget::AddService(groups::SERVICE),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while wait(identity.snapshot()).unwrap().last_error.is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        std::fs::read_dir(root.0.join("domains")).unwrap().count(),
        1
    );
    wait(owner.close()).unwrap();
    drop(owner);
    controls.threads.lock().unwrap().clear();
    controls.observation_ready.store(false, Ordering::Release);
    let owner = open_private(&root, false, controls.clone()).unwrap();
    let client = owner.identities();
    let identity = client.identity(reference).unwrap();
    assert_eq!(settings(&owner.commands(), reference, None).unwrap(), 42);
    wait(identity.recover()).unwrap();
    assert!(matches!(
        completed(&client, token, prepared),
        core::OperationResult::ServiceAdded { .. }
    ));
    // No new client jobs drive the scan after this point. The owner waker alone
    // completes it, and the same database still contains private settings.
    while !controls.observation_ready.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(settings(&owner.commands(), reference, None).unwrap(), 42);
    let groups_client = groups::client(owner.submission(), reference, Job::Groups);
    let page = wait(groups_client.conversations(None, QUERY_PAGE)).unwrap();
    assert!(page.items.is_empty() && page.next.is_none());
    let report1 = wait(identity.prepare(core::TransitionTarget::RemoveService(groups::SERVICE)))
        .unwrap()
        .review;
    let report2 = wait(identity.prepare(core::TransitionTarget::RemoveService(groups::SERVICE)))
        .unwrap()
        .review;
    assert_eq!(
        report1, report2,
        "unchanged observation must not rewrite revisions"
    );
    assert!(matches!(
        transition(
            &client,
            &identity,
            core::TransitionTarget::RemoveService(groups::SERVICE)
        ),
        core::OperationResult::ServiceRemoved { .. }
    ));
    let page = wait(groups_client.conversations(None, QUERY_PAGE)).unwrap();
    assert!(
        page.items.is_empty() && page.next.is_none(),
        "historical group attachment remains readable after removal"
    );
    assert!(matches!(
        transition(&client, &identity, core::TransitionTarget::Enable),
        core::OperationResult::Enabled { .. }
    ));
    assert_eq!(
        wait(identity.snapshot()).unwrap().runtime,
        core::RuntimeStatus::Running
    );
    let threads = controls.threads.lock().unwrap();
    assert!(!threads.is_empty());
    assert!(threads.iter().all(|thread| *thread == threads[0]));
    drop(threads);
    controls.drain.store(false, Ordering::Release);
    let completion = owner.close();
    drop(owner);
    let mut completion = std::pin::pin!(completion);
    assert!(completion
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    assert!(open_private(&root, false, controls.clone()).is_err());
    controls.drain.store(true, Ordering::Release);
    controls
        .wake
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .wake_by_ref();
    wait(completion).unwrap();
    let reopened = open_private(&root, false, controls.clone()).unwrap();
    let snapshot = wait(
        reopened
            .identities()
            .identity(reference)
            .unwrap()
            .snapshot(),
    )
    .unwrap();
    assert_eq!(snapshot.identity.desired, core::DesiredState::Enabled);
    assert_ne!(snapshot.runtime, core::RuntimeStatus::Running);
    assert_eq!(settings(&reopened.commands(), reference, None).unwrap(), 42);
    wait(reopened.close()).unwrap();
}
