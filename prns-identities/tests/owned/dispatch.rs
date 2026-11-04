use super::*;
use std::sync::mpsc;

struct Shared {
    leases: Mutex<Vec<DispatchLease>>,
    opened: AtomicUsize,
    ready: AtomicBool,
    drain: AtomicBool,
    wake: Mutex<Option<Waker>>,
}
impl Shared {
    fn new(ready: bool) -> Arc<Self> {
        Arc::new(Self {
            leases: Mutex::new(Vec::new()),
            opened: AtomicUsize::new(0),
            ready: AtomicBool::new(ready),
            drain: AtomicBool::new(true),
            wake: Mutex::new(None),
        })
    }
    fn latest(&self) -> DispatchLease {
        self.leases.lock().unwrap().last().unwrap().clone()
    }
}
struct Hooks(Arc<Shared>);
enum Job {
    Block(mpsc::Sender<()>, mpsc::Receiver<()>),
}
impl Driver<4> for Hooks {
    fn bind_dispatch(&mut self, owner: rt::OwnerToken, lease: DispatchLease) {
        assert_eq!(owner, lease.owner());
        assert!(!lease.is_open());
        self.0.leases.lock().unwrap().push(lease);
    }
    fn dispatch_opened(&mut self, permit: rt::NetworkPermit) {
        let lease = self.0.latest();
        assert_eq!(lease.owner(), permit.owner());
        assert!(lease.is_open());
        self.0.opened.fetch_add(1, Ordering::SeqCst);
    }
    fn activation_plan(
        &self,
        identity: &IdentityRecord<4>,
    ) -> Result<rt::ActivationPlan<4>, rt::Fault> {
        <IdentityOnly as Driver<4>>::activation_plan(&IdentityOnly, identity)
    }
    fn dependencies(
        &mut self,
        identity: IdentityReference,
        services: ServiceSet<4>,
    ) -> Result<DependencyReport<4>, rt::Fault> {
        <IdentityOnly as Driver<4>>::dependencies(&mut IdentityOnly, identity, services)
    }
    fn bind_key(&mut self, _: rt::OwnerToken, _: KeyLease) {}
    fn classify_failure(&self, _: &rt::Fault) -> ServiceFailure {
        ServiceFailure::StorageUnavailable
    }
}
impl rt::configuration::ServiceInitializer for Hooks {
    type Error = rt::Fault;
    fn initialize_service(
        &mut self,
        ticket: EffectTicket,
        identity: IdentityGuard,
        key: KeyReference,
        service: ServiceId,
    ) -> Result<InitializedService, rt::Fault> {
        rt::configuration::ServiceInitializer::initialize_service(
            &mut IdentityOnly,
            ticket,
            identity,
            key,
            service,
        )
    }
}
impl rt::ScopedDriver<4> for Hooks {
    type Error = rt::Fault;
    fn preflight(
        &mut self,
        scope: rt::Scope,
        plan: &rt::ActivationPlan<4>,
    ) -> Result<rt::Progress<rt::Evidence<rt::Resources>>, rt::Fault> {
        assert!(
            !self.0.latest().is_open(),
            "dispatch opened before native activation"
        );
        if !self.0.ready.load(Ordering::Acquire) {
            return Ok(rt::Progress::Pending);
        }
        <IdentityOnly as rt::ScopedDriver<4>>::preflight(&mut IdentityOnly, scope, plan)
    }
    fn prepare_service(
        &mut self,
        scope: rt::Scope,
        key: KeyReference,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::PreparedService>>, rt::Fault> {
        <IdentityOnly as rt::ScopedDriver<4>>::prepare_service(&mut IdentityOnly, scope, key, plan)
    }
    fn activate_service(
        &mut self,
        scope: rt::Scope,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<()>>, rt::Fault> {
        <IdentityOnly as rt::ScopedDriver<4>>::activate_service(&mut IdentityOnly, scope, plan)
    }
    fn fence_previous_workers(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<bool>>, rt::Fault> {
        if let Some(lease) = self.0.leases.lock().unwrap().last() {
            assert!(!lease.is_open());
        }
        if !self.0.drain.load(Ordering::Acquire) {
            return Ok(rt::Progress::Pending);
        }
        <IdentityOnly as rt::ScopedDriver<4>>::fence_previous_workers(&mut IdentityOnly, scope)
    }
    fn quiesce_service(
        &mut self,
        scope: rt::Scope,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::QuiescedService>>, rt::Fault> {
        <IdentityOnly as rt::ScopedDriver<4>>::quiesce_service(&mut IdentityOnly, scope, plan)
    }
    fn release_identity(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<rt::ReleasedIdentity<4>>>, rt::Fault> {
        if let Some(lease) = self.0.leases.lock().unwrap().last() {
            assert!(!lease.is_open());
        }
        if !self.0.drain.load(Ordering::Acquire) {
            return Ok(rt::Progress::Pending);
        }
        <IdentityOnly as rt::ScopedDriver<4>>::release_identity(&mut IdentityOnly, scope)
    }
}
impl CommandHandler<4, Job> for Hooks {
    fn command(&mut self, job: Job, _: &mut dyn Admission) {
        match job {
            Job::Block(entered, release) => {
                entered.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        }
    }
    fn reject(_: Job) {}
}
type HookOwner = Owner<4, 32, 4, 16, Hooks, Job>;
fn open(root: &Root, shared: Arc<Shared>) -> HookOwner {
    HookOwner::create_new(&root.0, options(), move |wake| {
        *shared.wake.lock().unwrap() = Some(wake);
        Ok(Hooks(shared))
    })
    .unwrap()
}
fn block(commands: &Commands<Job>) -> mpsc::Sender<()> {
    let (entered, entered_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    commands.submit(Job::Block(entered, release_rx)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    release
}

#[test]
fn dispatch_is_staged_until_confirmed_and_old_stop_replays_do_not_fence_replacement() {
    let root = Root::new();
    let shared = Shared::new(false);
    let owner = open(&root, shared.clone());
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let (enable, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    let deadline = Instant::now() + Duration::from_secs(5);
    while shared.leases.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let first = shared.latest();
    assert!(!first.is_open());
    assert_eq!(shared.opened.load(Ordering::SeqCst), 0);
    shared.ready.store(true, Ordering::Release);
    shared.wake.lock().unwrap().as_ref().unwrap().wake_by_ref();
    completed(&client, enable, prepared.clone());
    assert!(first.is_open());
    assert_eq!(
        wait(identity.snapshot()).unwrap().runtime,
        RuntimeStatus::Running
    );
    let pause = wait(identity.prepare(TransitionTarget::Pause)).unwrap();
    let digest = CommandDigest([19; 32]);
    let stop = wait(identity.apply_stop(pause.clone(), digest)).unwrap();
    completed(&client, stop, pause.clone());
    assert!(!first.is_open());
    let (enable, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    completed(&client, enable, prepared.clone());
    let second = shared.latest();
    assert_ne!(first.owner(), second.owner());
    assert!(second.is_open());
    assert!(!first.is_open());
    assert_eq!(
        wait(identity.apply_stop(pause.clone(), digest)).unwrap(),
        stop
    );
    assert!(matches!(
        wait(identity.begin(stop, pause.clone())).unwrap(),
        OperationState::Completed(_)
    ));
    assert!(
        second.is_open(),
        "historical retry fenced replacement owner"
    );
    assert_eq!(shared.opened.load(Ordering::SeqCst), 0);
    wait(owner.close()).unwrap();
    assert!(!second.is_open());
}

#[test]
fn failed_stop_publication_stays_fenced_without_claiming_catalog_change() {
    let root = Root::new();
    let shared = Shared::new(true);
    let owner = open(&root, shared.clone());
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let (enable, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    completed(&client, enable, prepared.clone());
    let lease = shared.latest();
    assert!(lease.is_open());
    let pause = wait(identity.prepare(TransitionTarget::Pause)).unwrap();
    let token = wait(client.reserve(pause.clone(), CommandDigest([20; 32]))).unwrap();
    let before = std::fs::read(root.0.join("catalog.bin")).unwrap();
    // Inject a publication-path failure after canonical validation without
    // corrupting the authoritative catalog or pretending the stop committed.
    std::fs::create_dir(root.0.join("catalog.next")).unwrap();
    assert!(wait(identity.begin(token, pause.clone())).is_err());
    assert!(!lease.is_open());
    assert_eq!(std::fs::read(root.0.join("catalog.bin")).unwrap(), before);
    wait(owner.close()).unwrap();
}

#[test]
fn close_and_drop_fence_external_dispatch_while_owner_handler_is_blocked() {
    let root = Root::new();
    let shared = Shared::new(true);
    let owner = open(&root, shared.clone());
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let (enable, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    completed(&client, enable, prepared.clone());
    let lease = shared.latest();
    let release = block(&owner.commands());
    let completion = owner.close();
    assert!(!lease.is_open());
    assert!(completion.status().is_none());
    release.send(()).unwrap();
    wait(completion).unwrap();
    drop(owner);
    let root = Root::new();
    let shared = Shared::new(true);
    let owner = open(&root, shared.clone());
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let (enable, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    completed(&client, enable, prepared.clone());
    let lease = shared.latest();
    let release = block(&owner.commands());
    drop(owner);
    assert!(!lease.is_open());
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)) {
            Ok(owner) => {
                wait(owner.close()).unwrap();
                break;
            }
            Err(_) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

#[test]
fn retrying_a_pending_stop_after_reopen_reschedules_its_retained_work() {
    let root = Root::new();
    let shared = Shared::new(true);
    let owner = open(&root, shared.clone());
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let (enable, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    completed(&client, enable, prepared.clone());
    shared.drain.store(false, Ordering::Release);
    let pause = wait(identity.prepare(TransitionTarget::Pause)).unwrap();
    let digest = CommandDigest([41; 32]);
    let token = wait(identity.apply_stop(pause.clone(), digest)).unwrap();
    assert_eq!(
        wait(client.operation(token, Command::Transition(pause.clone()))).unwrap(),
        OperationState::Transitioning
    );
    let release = block(&owner.commands());
    let completion = owner.close();
    shared.drain.store(true, Ordering::Release);
    release.send(()).unwrap();
    shared.wake.lock().unwrap().as_ref().unwrap().wake_by_ref();
    wait(completion).unwrap();
    drop(owner);
    let shared = Shared::new(true);
    let reopened = HookOwner::open_existing(&root.0, options(), move |wake| {
        *shared.wake.lock().unwrap() = Some(wake);
        Ok(Hooks(shared))
    })
    .unwrap();
    let client = reopened.client();
    assert_eq!(
        wait(client.operation(token, Command::Transition(pause.clone()))).unwrap(),
        OperationState::Transitioning
    );
    let identity = client.identity(identity.reference()).unwrap();
    // Exact retry is sufficient; no separate recover/restore call is needed.
    assert_eq!(
        wait(identity.apply_stop(pause.clone(), digest)).unwrap(),
        token
    );
    assert!(matches!(
        completed(&client, token, pause.clone()),
        OperationResult::Paused { .. }
    ));
    wait(reopened.close()).unwrap();
}
