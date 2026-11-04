//! Embedding-owned composition for the fixture. The production network adapter
//! owns all group protocol methods/resources; this only adds a private service.
use super::*;
use owned::CommandHandler;
use redb::{Database, ReadableDatabase, ReadableTableMetadata, TableDefinition, WriteTransaction};
use std::sync::Mutex;
use tokio::sync::{mpsc, oneshot};

pub const SETTINGS: core::ServiceId = core::ServiceId(0x7374_6e67);
const TABLE: TableDefinition<u8, u64> = TableDefinition::new("fixture_shared_settings_v1");
fn db(error: impl Into<redb::Error>) -> groups::SchemaError {
    groups::SchemaError::Database(error.into())
}
fn initialize(tx: &WriteTransaction) -> Result<(), groups::SchemaError> {
    let mut table = tx.open_table(TABLE).map_err(db)?;
    if !table.is_empty().map_err(db)? {
        return Err(domain::Error::Corrupt.into());
    }
    table.insert(0, 0).map_err(db)?;
    table.insert(1, 0).map_err(db)?;
    Ok(())
}
fn read(database: &Database) -> Result<(u64, u64), groups::SchemaError> {
    let tx = database.begin_read().map_err(db)?;
    let table = tx.open_table(TABLE).map_err(db)?;
    if table.len().map_err(db)? != 2 {
        return Err(domain::Error::Corrupt.into());
    }
    Ok((
        table
            .get(0)
            .map_err(db)?
            .ok_or(domain::Error::Corrupt)?
            .value(),
        table
            .get(1)
            .map_err(db)?
            .ok_or(domain::Error::Corrupt)?
            .value(),
    ))
}
fn reconcile(database: &Database) -> Result<(), groups::SchemaError> {
    let tx = database.begin_read().map_err(db)?;
    match tx.open_table(TABLE) {
        Ok(_) => {
            drop(tx);
            read(database).map(|_| ())
        }
        Err(redb::TableError::TableDoesNotExist(_)) => {
            drop(tx);
            let mut tx = database.begin_write().map_err(db)?;
            tx.set_durability(redb::Durability::Immediate).map_err(db)?;
            initialize(&tx)?;
            tx.commit().map_err(db)
        }
        Err(error) => Err(db(error)),
    }
}
#[allow(clippy::large_enum_variant)]
pub enum Job {
    Group(groups::Command),
    Settings {
        identity: core::IdentityReference,
        value: Option<u64>,
        reply: oneshot::Sender<Result<u64, String>>,
    },
}
pub struct Controls {
    pub workers: Mutex<Vec<std::thread::ThreadId>>,
    pub databases: Mutex<Vec<usize>>,
    pub scopes: Mutex<Vec<rt::Scope>>,
    pub opened: Mutex<Vec<rt::NetworkPermit>>,
    pub executor_threads: Mutex<Vec<std::thread::ThreadId>>,
    pub group_commands: AtomicUsize,
    pub settings_commands: AtomicUsize,
    pub group_prepared: AtomicBool,
    pub group_quiesced: AtomicBool,
    pub settings_waiting: AtomicBool,
    pub notification: AtomicU64,
    pub drain: tokio::sync::Semaphore,
}
impl Controls {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            workers: Mutex::new(Vec::new()),
            databases: Mutex::new(Vec::new()),
            scopes: Mutex::new(Vec::new()),
            opened: Mutex::new(Vec::new()),
            executor_threads: Mutex::new(Vec::new()),
            group_commands: AtomicUsize::new(0),
            settings_commands: AtomicUsize::new(0),
            group_prepared: AtomicBool::new(false),
            group_quiesced: AtomicBool::new(false),
            settings_waiting: AtomicBool::new(false),
            notification: AtomicU64::new(0),
            drain: tokio::sync::Semaphore::new(0),
        })
    }
}
struct SettingsRun {
    owner: rt::OwnerToken,
    key: owned::KeyLease,
    dispatch: owned::DispatchLease,
    sender: Option<mpsc::Sender<u64>>,
    task: Option<tokio::task::JoinHandle<()>>,
    quiesced: bool,
}
pub struct Driver {
    network: network::Driver<1, 2, Job>,
    executor: tokio::runtime::Handle,
    controls: Arc<Controls>,
    wake: Waker,
    binding: Option<(rt::OwnerToken, owned::DispatchLease)>,
    settings: Option<SettingsRun>,
    settings_quiesced: Option<(rt::OwnerToken, core::IdentityReference)>,
}
impl Driver {
    fn settings_dependency(
        &mut self,
        identity: core::IdentityReference,
    ) -> Result<core::ServiceDependency, network::DriverError> {
        let (revision, _) = self
            .network
            .local()
            .stores()
            .with_existing(identity, read)
            .map_err(groups::Error::Store)?;
        Ok(core::ServiceDependency::clear(SETTINGS, revision))
    }
    fn inspect_store(&mut self, identity: core::IdentityReference) {
        let pointer = self
            .network
            .local()
            .stores()
            .with_existing(identity, |database| {
                Ok::<_, groups::SchemaError>(database as *const Database as usize)
            })
            .unwrap();
        self.controls.databases.lock().unwrap().push(pointer);
    }
    fn settings_command(
        &mut self,
        identity: core::IdentityReference,
        value: Option<u64>,
        admission: &mut dyn owned::Admission,
    ) -> Result<u64, String> {
        // This is private state with no eventual network effect. Its proper
        // canonical class is LocalMutation, not a synthetic network permit.
        let permit = admission
            .acquire(
                identity,
                if value.is_some() {
                    owned::Access::LocalMutation
                } else {
                    owned::Access::History
                },
            )
            .map_err(|e| format!("{e:?}"))?;
        self.inspect_store(identity);
        let result = self
            .network
            .local()
            .stores()
            .with_existing(identity, |database| {
                let (revision, previous) = read(database)?;
                let Some(value) = value else {
                    return Ok(previous);
                };
                let mut tx = database.begin_write().map_err(db)?;
                tx.set_durability(redb::Durability::Immediate).map_err(db)?;
                {
                    let mut table = tx.open_table(TABLE).map_err(db)?;
                    table
                        .insert(
                            0,
                            revision.checked_add(1).ok_or(domain::Error::StorageFull)?,
                        )
                        .map_err(db)?;
                    table.insert(1, value).map_err(db)?;
                }
                tx.commit().map_err(db)?;
                Ok::<_, groups::SchemaError>(value)
            })
            .map_err(|e| format!("{e:?}"))?;
        admission
            .settle(permit, rt::Settlement::Committed)
            .map_err(|e| format!("{e:?}"))?;
        if value.is_some() {
            // Disposable post-commit notification: its bounded queue may
            // coalesce/drop hints; the durable table remains authoritative.
            if let Some(sender) = self.settings.as_ref().and_then(|s| s.sender.as_ref()) {
                let _ = sender.try_send(result);
            }
        }
        self.controls
            .settings_commands
            .fetch_add(1, Ordering::SeqCst);
        Ok(result)
    }
}
impl owned::Driver<2> for Driver {
    fn poll_observations(&mut self, cx: &mut Context<'_>) {
        self.network.poll_observations(cx);
    }
    fn poll_recovery(
        &mut self,
        cx: &mut Context<'_>,
        admission: &mut dyn owned::RecoveryAdmission,
    ) {
        self.network.poll_recovery(cx, admission);
    }
    fn bind_dispatch(&mut self, owner: rt::OwnerToken, dispatch: owned::DispatchLease) {
        self.binding = Some((owner, dispatch.clone()));
        self.network.bind_dispatch(owner, dispatch);
    }
    fn validate_metadata(&self, metadata: core::Metadata) -> Result<(), core::Fault> {
        self.network.validate_metadata(metadata)
    }
    fn bind_metadata(
        &mut self,
        owner: rt::OwnerToken,
        identity: core::IdentityReference,
        metadata: core::Metadata,
    ) {
        self.network.bind_metadata(owner, identity, metadata);
    }
    fn bind_key(&mut self, owner: rt::OwnerToken, key: owned::KeyLease) {
        let (bound, dispatch) = self.binding.as_ref().unwrap();
        assert_eq!(*bound, owner);
        self.settings = Some(SettingsRun {
            owner,
            key: key.clone(),
            dispatch: dispatch.clone(),
            sender: None,
            task: None,
            quiesced: false,
        });
        self.network.bind_key(owner, key);
    }
    fn dispatch_opened(&mut self, permit: rt::NetworkPermit) {
        self.controls.opened.lock().unwrap().push(permit);
        self.network.dispatch_opened(permit);
    }
    fn activation_plan(
        &self,
        identity: &core::IdentityRecord<2>,
    ) -> Result<rt::ActivationPlan<2>, network::DriverError> {
        let mut group_identity = *identity;
        group_identity.services = if identity.services.iter().any(|s| s == groups::SERVICE) {
            core::ServiceSet::new(&[groups::SERVICE]).unwrap()
        } else {
            core::ServiceSet::empty()
        };
        let group_plan = self.network.activation_plan(&group_identity)?;
        let mut plans = [None; 2];
        for (slot, service) in plans.iter_mut().zip(identity.services.iter()) {
            *slot = Some(if service == groups::SERVICE {
                *group_plan
                    .services
                    .iter()
                    .flatten()
                    .find(|p| p.service == service)
                    .unwrap()
            } else if service == SETTINGS {
                rt::ServicePlan {
                    service,
                    resources: rt::Resources {
                        queue_slots: 2,
                        ..rt::Resources::ZERO
                    },
                    requires_receive_state: false,
                    capabilities: rt::Capabilities {
                        scoped_teardown: true,
                        admission_gate: true,
                        durable_drain: true,
                        durable_receive_state: false,
                    },
                }
            } else {
                return Err(groups::Error::UnsupportedService(service).into());
            });
        }
        Ok(rt::ActivationPlan {
            identity_resources: group_plan.identity_resources,
            services: plans,
        })
    }
    fn dependencies(
        &mut self,
        identity: core::IdentityReference,
        services: core::ServiceSet<2>,
    ) -> Result<core::DependencyReport<2>, network::DriverError> {
        let mut report = core::DependencyReport::empty();
        for (slot, service) in report.entries.iter_mut().zip(services.iter()) {
            *slot = Some(if service == groups::SERVICE {
                self.network
                    .dependencies(identity, core::ServiceSet::new(&[service]).unwrap())?
                    .entries
                    .into_iter()
                    .flatten()
                    .next()
                    .unwrap()
            } else if service == SETTINGS {
                self.settings_dependency(identity)?
            } else {
                return Err(groups::Error::UnsupportedService(service).into());
            });
        }
        Ok(report)
    }
    fn classify_failure(&self, error: &network::DriverError) -> core::ServiceFailure {
        self.network.classify_failure(error)
    }
}
impl rt::configuration::ServiceInitializer for Driver {
    type Error = network::DriverError;
    fn initialize_service(
        &mut self,
        ticket: core::EffectTicket,
        identity: core::IdentityGuard,
        key: core::KeyReference,
        service: core::ServiceId,
    ) -> Result<core::InitializedService, Self::Error> {
        if service == groups::SERVICE {
            return self
                .network
                .initialize_service(ticket, identity, key, service);
        }
        if service != SETTINGS {
            return Err(groups::Error::UnsupportedService(service).into());
        }
        let reference = core::IdentityReference {
            id: identity.id,
            public_hash: identity.public_hash,
        };
        self.network
            .local()
            .stores()
            .prepare(reference, initialize, reconcile)
            .map_err(groups::Error::Store)?;
        Ok(core::InitializedService { ticket, service })
    }
}
impl rt::ScopedDriver<2> for Driver {
    type Error = network::DriverError;
    fn preflight(
        &mut self,
        scope: rt::Scope,
        plan: &rt::ActivationPlan<2>,
    ) -> Result<rt::Progress<rt::Evidence<rt::Resources>>, Self::Error> {
        self.network.preflight(scope, plan)
    }
    fn prepare_service(
        &mut self,
        scope: rt::Scope,
        key: core::KeyReference,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::PreparedService>>, Self::Error> {
        self.controls.scopes.lock().unwrap().push(scope);
        if plan.service == groups::SERVICE {
            let progress = self.network.prepare_service(scope, key, plan)?;
            if matches!(progress, rt::Progress::Complete(_)) {
                self.controls.group_prepared.store(true, Ordering::Release);
            }
            return Ok(progress);
        }
        assert_eq!(plan.service, SETTINGS);
        self.settings_dependency(core::IdentityReference {
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
    ) -> Result<rt::Progress<rt::Evidence<()>>, Self::Error> {
        if plan.service == groups::SERVICE {
            return self.network.activate_service(scope, plan);
        }
        assert_eq!(plan.service, SETTINGS);
        let run = self.settings.as_mut().unwrap();
        assert_eq!(run.owner, scope.owner);
        if run.task.is_none() {
            let (sender, mut receiver) = mpsc::channel(2);
            run.sender = Some(sender);
            let controls = self.controls.clone();
            let wake = self.wake.clone();
            let key = run.key.clone();
            run.task = Some(self.executor.spawn(async move {
                struct WakeOnDrop(Waker);
                impl Drop for WakeOnDrop {
                    fn drop(&mut self) {
                        self.0.wake_by_ref();
                    }
                }
                let _wake = WakeOnDrop(wake);
                let _key = key;
                controls
                    .executor_threads
                    .lock()
                    .unwrap()
                    .push(std::thread::current().id());
                while let Some(value) = receiver.recv().await {
                    controls.notification.store(value, Ordering::Release);
                }
                controls.settings_waiting.store(true, Ordering::Release);
                let _drain = controls.drain.acquire().await.unwrap();
            }));
        }
        Ok(rt::Progress::Complete(rt::Evidence { scope, value: () }))
    }
    fn fence_previous_workers(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<bool>>, Self::Error> {
        self.network.fence_previous_workers(scope)
    }
    fn quiesce_service(
        &mut self,
        scope: rt::Scope,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::QuiescedService>>, Self::Error> {
        if plan.service == groups::SERVICE {
            let progress = self.network.quiesce_service(scope, plan)?;
            if let rt::Progress::Complete(ref evidence) = progress {
                assert_eq!(evidence.value.evidence.registered_destinations, 0);
                assert_eq!(evidence.value.evidence.request_handlers, 0);
                self.controls.group_quiesced.store(true, Ordering::Release);
            }
            return Ok(progress);
        }
        assert_eq!(plan.service, SETTINGS);
        if let Some(run) = &mut self.settings {
            assert_eq!(run.owner, scope.owner);
            assert!(!run.dispatch.is_open());
            run.sender.take();
            if let Some(task) = &mut run.task {
                match std::pin::Pin::new(task).poll(&mut Context::from_waker(&self.wake)) {
                    Poll::Pending => return Ok(rt::Progress::Pending),
                    Poll::Ready(value) => {
                        value.unwrap();
                        run.task = None;
                    }
                }
            }
            run.quiesced = true;
        }
        let identity = core::IdentityReference {
            id: scope.identity.id,
            public_hash: scope.identity.public_hash,
        };
        let dependencies = self.settings_dependency(identity)?;
        self.settings_quiesced = Some((scope.owner, identity));
        Ok(rt::Progress::Complete(rt::Evidence {
            scope,
            value: rt::QuiescedService {
                evidence: rt::QuiescenceEvidence::COMPLETE,
                dependencies,
            },
        }))
    }
    fn release_identity(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<rt::ReleasedIdentity<2>>>, Self::Error> {
        // Configuration of the first service also releases an empty owner.
        // Report only the private service actually included in this cleanup.
        let private = match self.settings_quiesced {
            Some((owner, identity)) if owner == scope.owner => {
                Some(self.settings_dependency(identity)?)
            }
            _ => None,
        };
        let mut progress = self.network.release_identity(scope)?;
        if let rt::Progress::Complete(ref mut evidence) = progress {
            if let Some(run) = &self.settings {
                assert!(run.quiesced);
            }
            if let Some(private) = private {
                *evidence
                    .value
                    .report
                    .entries
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .unwrap() = Some(private);
            }
            self.settings.take();
            self.settings_quiesced = None;
        }
        Ok(progress)
    }
}
impl CommandHandler<2, Job> for Driver {
    fn command(&mut self, job: Job, admission: &mut dyn owned::Admission) {
        self.controls
            .workers
            .lock()
            .unwrap()
            .push(std::thread::current().id());
        match job {
            Job::Group(command) => {
                self.inspect_store(command.identity);
                self.controls.group_commands.fetch_add(1, Ordering::SeqCst);
                self.network.command(command, admission);
            }
            Job::Settings {
                identity,
                value,
                reply,
            } => {
                let result = self.settings_command(identity, value, admission);
                let _ = reply.send(result);
            }
        }
    }
    fn reject(job: Job) {
        match job {
            Job::Group(command) => groups::Local::<1>::reject(command),
            Job::Settings { reply, .. } => {
                let _ = reply.send(Err("closed before dispatch".into()));
            }
        }
    }
}
pub fn factory(
    stores: stores::Stores<1>,
    catalog: &core::Catalog<1, 16, 2>,
    wake: Waker,
    commands: owned::Commands<Job>,
    context: network::Context,
    controls: Arc<Controls>,
    input: groups::SubmissionState,
) -> Result<Driver, network::DriverError> {
    let mut local = groups::Local::new(stores, options().storage.retention, wake.clone(), input)?;
    for identity in catalog.identities() {
        for service in identity.services.iter() {
            if service == groups::SERVICE {
                local.attach(identity.reference())?;
            } else if service == SETTINGS {
                local
                    .stores()
                    .with_existing(identity.reference(), read)
                    .map_err(groups::Error::Store)?;
            } else {
                return Err(groups::Error::UnsupportedService(service).into());
            }
        }
    }
    let executor = context.executor().clone();
    let network = network::Driver::new(
        local,
        context,
        commands,
        Job::Group,
        options().service,
        wake.clone(),
    )?;
    Ok(Driver {
        network,
        executor,
        controls,
        wake,
        binding: None,
        settings: None,
        settings_quiesced: None,
    })
}
