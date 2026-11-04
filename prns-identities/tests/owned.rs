#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::panic)]
use prns_identities::api::{Command, CreateRequest, PreparedTransition};
use prns_identities::{creation::CreateRequestId, owned::*};
use prns_identities_core::*;
use prns_identities_runtime as rt;
use std::{
    future::Future,
    num::NonZeroUsize,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
fn wait<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            return result;
        }
        assert!(Instant::now() < deadline, "completion did not arrive");
        std::thread::park_timeout(Duration::from_millis(10));
    }
}
// Explicitly cross the async admission boundary before losing a result or
// fencing the owner. This handles either an immediate or a delayed reply.
fn admit<F: Future>(future: F) -> impl Future<Output = F::Output> {
    let mut future = Box::pin(future);
    let first = future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    async move {
        match first {
            Poll::Ready(result) => result,
            Poll::Pending => future.await,
        }
    }
}
fn pending<F: Future>(future: Pin<&mut F>) -> bool {
    let waker = Waker::noop();
    matches!(future.poll(&mut Context::from_waker(waker)), Poll::Pending)
}
static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "identity-owned-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::SeqCst)
        )))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn options() -> Options {
    Options {
        queue_capacity: NonZeroUsize::new(8).unwrap(),
        budget: rt::Budget {
            capacity: rt::Resources {
                held_keys: 4,
                destinations: 4,
                request_handlers: 4,
                links: 4,
                queue_slots: 16,
            },
            infrastructure_reserve: rt::Resources::ZERO,
        },
    }
}
type Only = Owner<4, 32, 4, 16, IdentityOnly>;
fn create(client: &Client, nonce: u8, services: ServiceSet<4>) -> IdentityClient {
    let scope = wait(client.creation_scope()).unwrap();
    let outcome = wait(client.create_or_recover(
        CreateRequestId {
            scope,
            nonce: [nonce; 32],
        },
        CreateRequest {
            source: KeySource::Generate,
            metadata: Metadata::new("test", None).unwrap(),
            services: services.iter().collect(),
        },
        None,
    ))
    .unwrap();
    let OperationResult::Created { identity } = outcome.result else {
        panic!("not created")
    };
    let record = wait(client.identities())
        .unwrap()
        .into_iter()
        .find(|i| i.id == identity)
        .unwrap();
    client.identity(record.reference()).unwrap()
}
fn begin(
    client: &Client,
    identity: &IdentityClient,
    target: TransitionTarget,
) -> (OperationToken, PreparedTransition) {
    let prepared = wait(identity.prepare(target)).unwrap();
    let token = wait(client.reserve(prepared.clone(), CommandDigest([3; 32]))).unwrap();
    wait(identity.begin(token, prepared.clone())).unwrap();
    (token, prepared)
}
fn completed(
    client: &Client,
    token: OperationToken,
    prepared: PreparedTransition,
) -> OperationResult {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let OperationState::Completed(result) =
            wait(client.operation(token, Command::Transition(prepared.clone()))).unwrap()
        {
            return result;
        }
        if Instant::now() >= deadline {
            let snapshot = wait(
                client
                    .identity(IdentityReference {
                        id: prepared.identity.id,
                        public_hash: prepared.identity.public_hash,
                    })
                    .unwrap()
                    .snapshot(),
            )
            .unwrap();
            panic!(
                "operation {:?} stalled: {:?}, {:?}",
                prepared.target, snapshot.identity, snapshot.last_error
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn identity_only_owner_reopens_enabled_without_implicit_activation_and_preserves_exact_reference() {
    let root = Root::new();
    let owner = Only::create_new(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let reference = identity.reference();
    let (token, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    assert!(matches!(
        completed(&client, token, prepared.clone()),
        OperationResult::Enabled { .. }
    ));
    let before = wait(identity.snapshot()).unwrap();
    assert_eq!(before.runtime, RuntimeStatus::Running);
    assert!(Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)).is_err());
    let done = owner.close();
    wait(done.clone()).unwrap();
    wait(done).unwrap();
    assert!(matches!(
        wait(identity.snapshot()),
        Err(ApiError::Submission {
            reason: SubmitFailure::Closed
        })
    ));
    drop(owner);
    let owner = Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    let identity = owner.client().identity(reference).unwrap();
    let after = wait(identity.snapshot()).unwrap();
    assert_eq!(after.identity, before.identity);
    assert_eq!(after.identity.desired, DesiredState::Enabled);
    assert_eq!(after.runtime, RuntimeStatus::Unavailable);
    let wrong = owner
        .client()
        .identity(IdentityReference {
            public_hash: PublicIdentityHash([9; 16]),
            ..reference
        })
        .unwrap();
    assert!(matches!(
        wait(wrong.recover()).unwrap_err(),
        ApiError::Authority {
            reason: Fault::IdentityChanged
        }
    ));
    wait(identity.recover()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while wait(identity.snapshot()).unwrap().runtime != RuntimeStatus::Running {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    wait(owner.close()).unwrap();
}

#[derive(Default)]
struct Gate {
    open: AtomicBool,
    wake: Mutex<Option<Waker>>,
}
impl Gate {
    fn release(&self) {
        self.open.store(true, Ordering::SeqCst);
        if let Some(wake) = self.wake.lock().unwrap().take() {
            wake.wake();
        }
    }
}
struct PendingEffect {
    scope: rt::Scope,
    gate: Arc<Gate>,
    _key: KeyLease,
}
impl Future for PendingEffect {
    type Output = rt::Evidence<()>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut wake = self.gate.wake.lock().unwrap();
        if self.gate.open.load(Ordering::SeqCst) {
            Poll::Ready(rt::Evidence {
                scope: self.scope,
                value: (),
            })
        } else {
            *wake = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}
const SENSOR: ServiceId = ServiceId(421);
struct Sensor {
    root: PathBuf,
    wake: Waker,
    activation: Arc<Gate>,
    drain: Arc<Gate>,
    started: Arc<AtomicBool>,
    fail_drain: Arc<AtomicBool>,
    panic_activation: bool,
    keys: Vec<(rt::OwnerToken, KeyLease)>,
    effect: Option<Pin<Box<PendingEffect>>>,
    held: Arc<Mutex<Option<KeyLease>>>,
}
impl Sensor {
    fn record(&self, id: LocalIdentityId) -> PathBuf {
        self.root.join(format!("sensor-{}", id.sequence))
    }
    fn dependency() -> ServiceDependency {
        ServiceDependency {
            service: SENSOR,
            revision: 1,
            pending_messages: 0,
            unsettled_control: 0,
            authority_count: 0,
            recovery_pending: false,
            quiescence_supported: true,
        }
    }
}
impl Driver<4> for Sensor {
    fn activation_plan(
        &self,
        record: &IdentityRecord<4>,
    ) -> Result<rt::ActivationPlan<4>, rt::Fault> {
        let mut plans = Vec::new();
        for service in record.services.iter() {
            if service != SENSOR {
                return Err(rt::Fault::UnsupportedService);
            }
            plans.push(rt::ServicePlan {
                service,
                resources: rt::Resources::ZERO,
                requires_receive_state: true,
                capabilities: rt::Capabilities::QUALIFIED,
            });
        }
        rt::ActivationPlan::new(
            rt::Resources {
                held_keys: 1,
                ..rt::Resources::ZERO
            },
            &plans,
        )
    }
    fn dependencies(
        &mut self,
        _: IdentityReference,
        services: ServiceSet<4>,
    ) -> Result<DependencyReport<4>, rt::Fault> {
        let mut entries = Vec::new();
        for service in services.iter() {
            if service != SENSOR {
                return Err(rt::Fault::UnsupportedService);
            }
            entries.push(Self::dependency());
        }
        DependencyReport::new(&entries, 0).map_err(rt::Fault::Authority)
    }
    fn bind_key(&mut self, owner: rt::OwnerToken, key: KeyLease) {
        self.keys.push((owner, key));
    }
    fn classify_failure(&self, _: &rt::Fault) -> ServiceFailure {
        ServiceFailure::StorageUnavailable
    }
}
impl rt::configuration::ServiceInitializer for Sensor {
    type Error = rt::Fault;
    fn initialize_service(
        &mut self,
        ticket: EffectTicket,
        identity: IdentityGuard,
        _: KeyReference,
        service: ServiceId,
    ) -> Result<InitializedService, Self::Error> {
        if service != SENSOR {
            return Err(rt::Fault::UnsupportedService);
        }
        let expected = identity.public_hash.0;
        let path = self.record(identity.id);
        match std::fs::read(&path) {
            Ok(bytes) if bytes == expected => {}
            Ok(_) => return Err(rt::Fault::WrongIdentity),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|_| rt::Fault::StorageNotReady)?;
                file.write_all(&expected)
                    .map_err(|_| rt::Fault::StorageNotReady)?;
                file.sync_all().map_err(|_| rt::Fault::StorageNotReady)?;
                std::fs::File::open(&self.root).unwrap().sync_all().unwrap();
            }
            Err(_) => return Err(rt::Fault::StorageNotReady),
        }
        Ok(InitializedService { ticket, service })
    }
}
impl rt::ScopedDriver<4> for Sensor {
    type Error = rt::Fault;
    fn preflight(
        &mut self,
        scope: rt::Scope,
        plan: &rt::ActivationPlan<4>,
    ) -> Result<rt::Progress<rt::Evidence<rt::Resources>>, Self::Error> {
        Ok(rt::Progress::Complete(rt::Evidence {
            scope,
            value: plan.total()?,
        }))
    }
    fn prepare_service(
        &mut self,
        scope: rt::Scope,
        _: KeyReference,
        _: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::PreparedService>>, Self::Error> {
        if std::fs::read(self.record(scope.identity.id)).map_err(|_| rt::Fault::StorageNotReady)?
            != scope.identity.public_hash.0
        {
            return Err(rt::Fault::WrongIdentity);
        }
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
        _: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<()>>, Self::Error> {
        if self.effect.is_none() {
            let key = self
                .keys
                .iter()
                .find(|(owner, _)| *owner == scope.owner)
                .unwrap()
                .1
                .clone();
            *self.held.lock().unwrap() = Some(key.clone());
            self.effect = Some(Box::pin(PendingEffect {
                scope,
                gate: self.activation.clone(),
                _key: key,
            }));
            self.started.store(true, Ordering::SeqCst);
            assert!(
                !self.panic_activation,
                "injected driver panic after retaining a key"
            );
        }
        match self
            .effect
            .as_mut()
            .unwrap()
            .as_mut()
            .poll(&mut Context::from_waker(&self.wake))
        {
            Poll::Pending => Ok(rt::Progress::Pending),
            Poll::Ready(evidence) => {
                self.effect = None;
                Ok(rt::Progress::Complete(evidence))
            }
        }
    }
    fn fence_previous_workers(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<bool>>, Self::Error> {
        if let Some(effect) = self
            .effect
            .as_mut()
            .filter(|effect| effect.scope.owner == scope.owner)
        {
            if effect
                .as_mut()
                .poll(&mut Context::from_waker(&self.wake))
                .is_pending()
            {
                return Ok(rt::Progress::Pending);
            }
            self.effect = None;
        }
        Ok(rt::Progress::Complete(rt::Evidence { scope, value: true }))
    }
    fn quiesce_service(
        &mut self,
        scope: rt::Scope,
        _: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::QuiescedService>>, Self::Error> {
        let mut wake = self.drain.wake.lock().unwrap();
        *wake = Some(self.wake.clone());
        if self.fail_drain.load(Ordering::SeqCst) {
            return Err(rt::Fault::StorageNotReady);
        }
        if !self.drain.open.load(Ordering::SeqCst) {
            *wake = Some(self.wake.clone());
            return Ok(rt::Progress::Pending);
        }
        Ok(rt::Progress::Complete(rt::Evidence {
            scope,
            value: rt::QuiescedService {
                evidence: rt::QuiescenceEvidence::COMPLETE,
                dependencies: Self::dependency(),
            },
        }))
    }
    fn release_identity(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<rt::ReleasedIdentity<4>>>, Self::Error> {
        self.keys.retain(|(owner, _)| *owner != scope.owner);
        let services = if self.record(scope.identity.id).exists() {
            ServiceSet::new(&[SENSOR]).unwrap()
        } else {
            ServiceSet::empty()
        };
        let report = self.dependencies(
            IdentityReference {
                id: scope.identity.id,
                public_hash: scope.identity.public_hash,
            },
            services,
        )?;
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

#[test]
fn pending_native_effects_keep_keys_and_catalog_lock_after_caller_and_close_observer_loss() {
    type SensorOwner = Owner<4, 32, 4, 16, Sensor>;
    let root = Root::new();
    let domain = Root::new();
    std::fs::create_dir(&domain.0).unwrap();
    let activation = Arc::new(Gate::default());
    let drain = Arc::new(Gate::default());
    let started = Arc::new(AtomicBool::new(false));
    let fail_drain = Arc::new(AtomicBool::new(true));
    let held = Arc::new(Mutex::new(None));
    let owner = SensorOwner::create_new(&root.0, options(), {
        let domain = domain.0.clone();
        let activation = activation.clone();
        let drain = drain.clone();
        let started = started.clone();
        let fail_drain = fail_drain.clone();
        let held = held.clone();
        move |wake| {
            Ok(Sensor {
                root: domain,
                wake,
                activation,
                drain,
                started,
                fail_drain,
                panic_activation: false,
                keys: Vec::new(),
                effect: None,
                held,
            })
        }
    })
    .unwrap();
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let (add, prepared) = begin(&client, &identity, TransitionTarget::AddService(SENSOR));
    assert!(matches!(
        completed(&client, add, prepared.clone()),
        OperationResult::ServiceAdded {
            service: SENSOR,
            ..
        }
    ));
    let prepared = wait(identity.prepare(TransitionTarget::Enable)).unwrap();
    let token = wait(client.reserve(prepared.clone(), CommandDigest([4; 32]))).unwrap();
    // The result observer is discarded before the admitted command executes.
    drop(admit(identity.begin(token, prepared.clone())));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        wait(identity.snapshot()).unwrap().identity.desired,
        DesiredState::Enabled
    );
    // Another identity progresses on this lane while the first future is pending.
    let second = create(&client, 2, ServiceSet::empty());
    let (other, other_command) = begin(&client, &second, TransitionTarget::Enable);
    assert!(matches!(
        completed(&client, other, other_command),
        OperationResult::Enabled { .. }
    ));
    let before = wait(identity.snapshot()).unwrap().identity;
    drop(owner.close());
    let diagnostics = owner.diagnostics();
    let mut completion = Box::pin(owner.close());
    drop(owner);
    assert!(pending(completion.as_mut()));
    assert!(Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)).is_err());
    // The factory-created wake reaches the same pending worker; no ambient
    // executor and no ordinary queue submission is involved in these completions.
    activation.release();
    let deadline = Instant::now() + Duration::from_secs(5);
    while diagnostics.latest().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let diagnostic = diagnostics.latest().unwrap();
    assert_eq!(diagnostic.identity, Some(identity.reference()));
    assert!(matches!(
        &*diagnostic.error,
        prns_identities::owned::Error::Driver(rt::Fault::StorageNotReady)
    ));
    let public_drain_failure = diagnostic
        .error
        .api_error(|_| ServiceFailure::StorageUnavailable);
    assert_eq!(
        public_drain_failure,
        ApiError::Driver {
            reason: ServiceFailure::StorageUnavailable
        }
    );
    assert!(
        pending(completion.as_mut()),
        "a diagnostic is not cleanup evidence"
    );
    fail_drain.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(10));
    assert!(pending(completion.as_mut()));
    drain.release();
    std::thread::sleep(Duration::from_millis(10));
    assert!(pending(completion.as_mut()));
    let retained = held.lock().unwrap().take().unwrap();
    assert_eq!(
        *retained.material().identity_hash().as_bytes(),
        before.public_hash.0
    );
    drop(retained);
    wait(completion).unwrap();
    let historical = diagnostics.latest().unwrap();
    assert_eq!(historical.identity, diagnostic.identity);
    assert_eq!(
        historical
            .error
            .api_error(|_| ServiceFailure::StorageUnavailable),
        public_drain_failure,
        "successful drainage must not erase the retained historical failure"
    );
    // Termination drains process resources without acknowledging unfinished
    // activation or rewriting the caller's saved enabled intent.
    let reopened = SensorOwner::open_existing(&root.0, options(), {
        let domain = domain.0.clone();
        let activation = activation.clone();
        let drain = drain.clone();
        let started = started.clone();
        let fail_drain = fail_drain.clone();
        let held = held.clone();
        move |wake| {
            Ok(Sensor {
                root: domain,
                wake,
                activation,
                drain,
                started,
                fail_drain,
                panic_activation: false,
                keys: Vec::new(),
                effect: None,
                held,
            })
        }
    })
    .unwrap();
    let identity = reopened.client().identity(before.reference()).unwrap();
    assert_eq!(wait(identity.snapshot()).unwrap().identity, before);
    wait(identity.recover()).unwrap();
    assert!(matches!(
        completed(&reopened.client(), token, prepared.clone()),
        OperationResult::Enabled { .. }
    ));
    held.lock().unwrap().take();
    wait(reopened.close()).unwrap();
}

#[test]
fn unknown_configured_service_is_unavailable_and_reopen_never_initializes_it() {
    let root = Root::new();
    let owner = Only::create_new(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::new(&[SENSOR]).unwrap());
    assert!(matches!(
        wait(identity.prepare(TransitionTarget::Enable)).unwrap_err(),
        ApiError::Driver {
            reason: ServiceFailure::Unavailable(0)
        }
    ));
    let diagnostic = owner.diagnostics().latest().unwrap();
    assert_eq!(diagnostic.identity, Some(identity.reference()));
    assert!(matches!(
        &*diagnostic.error,
        prns_identities::owned::Error::Driver(rt::Fault::UnsupportedService)
    ));
    let before = wait(identity.snapshot()).unwrap().identity;
    wait(owner.close()).unwrap();
    drop(owner);
    let owner = Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    let after = wait(
        owner
            .client()
            .identity(before.reference())
            .unwrap()
            .snapshot(),
    )
    .unwrap()
    .identity;
    assert_eq!(before, after);
    assert!(!root
        .0
        .join(format!("sensor-{}", before.id.sequence))
        .exists());
    wait(owner.close()).unwrap();
}

#[test]
fn failed_start_preserves_store_and_invalid_capacity_has_no_file_effect() {
    let invalid = Root::new();
    let mut configuration = options();
    configuration.queue_capacity = NonZeroUsize::new(1025).unwrap();
    assert!(matches!(
        Only::create_new(&invalid.0, configuration, |_| Ok(IdentityOnly)),
        Err(prns_identities::owned::Error::InvalidOptions)
    ));
    assert!(!invalid.0.exists());
    let root = Root::new();
    assert!(matches!(
        Only::create_new(&root.0, options(), |_| Err(rt::Fault::UnsupportedService)),
        Err(prns_identities::owned::Error::Driver(
            rt::Fault::UnsupportedService
        ))
    ));
    assert!(root.0.join("catalog.bin").exists());
    assert!(Only::create_new(&root.0, options(), |_| Ok(IdentityOnly)).is_err());
    let reopened = Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    assert!(wait(reopened.client().identities()).unwrap().is_empty());
    wait(reopened.close()).unwrap();
}

#[test]
fn driver_panic_reports_failure_and_external_key_lease_keeps_exclusive_catalog_lock() {
    type SensorOwner = Owner<4, 32, 4, 16, Sensor>;
    let root = Root::new();
    let domain = Root::new();
    std::fs::create_dir(&domain.0).unwrap();
    let held = Arc::new(Mutex::new(None));
    let started = Arc::new(AtomicBool::new(false));
    let owner = SensorOwner::create_new(&root.0, options(), {
        let domain = domain.0.clone();
        let held = held.clone();
        let started = started.clone();
        move |wake| {
            Ok(Sensor {
                root: domain,
                wake,
                activation: Arc::new(Gate::default()),
                drain: Arc::new(Gate::default()),
                started,
                fail_drain: Arc::new(AtomicBool::new(false)),
                panic_activation: true,
                keys: Vec::new(),
                effect: None,
                held,
            })
        }
    })
    .unwrap();
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let (token, prepared) = begin(&client, &identity, TransitionTarget::AddService(SENSOR));
    completed(&client, token, prepared.clone());
    let prepared = wait(identity.prepare(TransitionTarget::Enable)).unwrap();
    let token = wait(client.reserve(prepared.clone(), CommandDigest([8; 32]))).unwrap();
    drop(admit(identity.begin(token, prepared.clone())));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(
        wait(owner.close()),
        Err(prns_storage::CloseError::Panicked)
    ));
    drop(owner);
    assert!(matches!(
        Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)),
        Err(prns_identities::owned::Error::Store(
            prns_identities::Error::Busy
        ))
    ));
    let key = held.lock().unwrap().take().unwrap();
    assert_eq!(
        *key.material().identity_hash().as_bytes(),
        identity.reference().public_hash.0
    );
    drop(key);
    let reopened = Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    assert_eq!(
        wait(
            reopened
                .client()
                .operation(token, Command::Transition(prepared.clone()))
        )
        .unwrap(),
        OperationState::Transitioning
    );
    wait(reopened.close()).unwrap();
}

#[derive(Debug)]
enum SensorError {
    Admission(AdmissionError),
    Io,
    Conflict,
    Closed,
}
struct Measurement {
    id: [u8; 16],
    value: u8,
}
enum SensorJob {
    Block {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    },
    Measure {
        identity: IdentityReference,
        measurement: Measurement,
        reply: std::sync::mpsc::Sender<Result<u8, SensorError>>,
    },
    DeferMeasure {
        identity: IdentityReference,
        measurement: Measurement,
        ready: Arc<Gate>,
        entered: std::sync::mpsc::Sender<()>,
    },
    Read {
        identity: IdentityReference,
        reply: std::sync::mpsc::Sender<Result<Vec<u8>, SensorError>>,
    },
    Label {
        identity: IdentityReference,
        reply: std::sync::mpsc::Sender<Result<(), SensorError>>,
    },
}
struct PendingMeasurement {
    identity: IdentityReference,
    measurement: Measurement,
    ready: Arc<Gate>,
    permit: Option<Permit>,
}
struct BorrowedSensor<'a> {
    sensor: Sensor,
    log: &'a mut std::fs::File,
    pending: Option<PendingMeasurement>,
    network: Option<rt::NetworkPermit>,
}
impl BorrowedSensor<'_> {
    fn measurements(&mut self) -> Result<Vec<u8>, SensorError> {
        use std::io::{Read, Seek};
        self.log.rewind().map_err(|_| SensorError::Io)?;
        let mut bytes = Vec::new();
        self.log
            .read_to_end(&mut bytes)
            .map_err(|_| SensorError::Io)?;
        if bytes.len() % 17 != 0 {
            return Err(SensorError::Io);
        }
        Ok(bytes)
    }
    fn measure(
        &mut self,
        identity: IdentityReference,
        measurement: Measurement,
        admission: &mut dyn Admission,
    ) -> Result<u8, SensorError> {
        // Exact durable recovery is checked before fresh service admission.
        let recovery = admission
            .acquire(identity, Access::Recovery)
            .map_err(SensorError::Admission)?;
        let rows = self.measurements();
        admission
            .settle(recovery, rt::Settlement::Committed)
            .map_err(SensorError::Admission)?;
        let rows = rows?;
        if let Some(row) = rows
            .as_chunks::<17>()
            .0
            .iter()
            .find(|row| row[..16] == measurement.id)
        {
            return if row[16] == measurement.value {
                Ok(row[16])
            } else {
                Err(SensorError::Conflict)
            };
        }
        let permit = admission
            .acquire(
                identity,
                Access::ServiceMutation {
                    service: SENSOR,
                    operation: rt::MutationId(measurement.id),
                },
            )
            .map_err(SensorError::Admission)?;
        let result = (|| {
            use std::io::{Seek, Write};
            self.log
                .seek(std::io::SeekFrom::End(0))
                .map_err(|_| SensorError::Io)?;
            self.log
                .write_all(&measurement.id)
                .map_err(|_| SensorError::Io)?;
            self.log
                .write_all(&[measurement.value])
                .map_err(|_| SensorError::Io)?;
            self.log.sync_all().map_err(|_| SensorError::Io)?;
            Ok(measurement.value)
        })();
        // This fixture's successful synchronous commit is definitive. On I/O
        // failure it conservatively retains the lease rather than inventing a
        // durable recovery witness; the production group retainer owns that fact.
        if result.is_ok() {
            admission
                .settle(permit, rt::Settlement::Committed)
                .map_err(SensorError::Admission)?;
        }
        result
    }
}
impl CommandHandler<4, SensorJob> for BorrowedSensor<'_> {
    fn command(&mut self, job: SensorJob, admission: &mut dyn Admission) {
        match job {
            SensorJob::Block { entered, release } => {
                entered.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            SensorJob::Measure {
                identity,
                measurement,
                reply,
            } => {
                let _ = reply.send(self.measure(identity, measurement, admission));
            }
            SensorJob::DeferMeasure {
                identity,
                measurement,
                ready,
                entered,
            } => {
                use std::io::{Seek, Write};
                assert!(self.pending.is_none());
                let network = self.network.unwrap();
                assert_eq!(network.admission().identity.id, identity.id);
                admission.validate_network(network).unwrap();
                let permit = admission
                    .acquire_network(network, rt::MutationId(measurement.id))
                    .unwrap();
                self.pending = Some(PendingMeasurement {
                    identity,
                    measurement,
                    ready,
                    permit: Some(permit),
                });
                // The storage effect and its original permit remain owned even
                // when its caller has lost the result. Recovery reads this exact
                // file before asserting settlement; releasing the gate alone
                // is not completion evidence.
                let pending = self.pending.as_ref().unwrap();
                self.log.seek(std::io::SeekFrom::End(0)).unwrap();
                self.log.write_all(&pending.measurement.id).unwrap();
                self.log.write_all(&[pending.measurement.value]).unwrap();
                self.log.sync_all().unwrap();
                entered.send(()).unwrap();
            }
            SensorJob::Read { identity, reply } => {
                let result = (|| {
                    let permit = admission
                        .acquire(identity, Access::History)
                        .map_err(SensorError::Admission)?;
                    let result = self.measurements();
                    admission
                        .settle(permit, rt::Settlement::Committed)
                        .map_err(SensorError::Admission)?;
                    result
                })();
                let _ = reply.send(result);
            }
            SensorJob::Label { identity, reply } => {
                let result = (|| {
                    let permit = admission
                        .acquire(identity, Access::LocalMutation)
                        .map_err(SensorError::Admission)?;
                    std::fs::write(self.sensor.root.join("private-label"), b"sensor")
                        .map_err(|_| SensorError::Io)?;
                    admission
                        .settle(permit, rt::Settlement::Committed)
                        .map_err(SensorError::Admission)
                })();
                let _ = reply.send(result);
            }
        }
    }
    fn reject(job: SensorJob) {
        match job {
            SensorJob::Block { .. } | SensorJob::DeferMeasure { .. } => {}
            SensorJob::Measure { reply, .. } => {
                let _ = reply.send(Err(SensorError::Closed));
            }
            SensorJob::Read { reply, .. } => {
                let _ = reply.send(Err(SensorError::Closed));
            }
            SensorJob::Label { reply, .. } => {
                let _ = reply.send(Err(SensorError::Closed));
            }
        }
    }
}
// The fixture borrows a real file alongside its existing service driver. These
// delegations add no lifecycle decisions; all command admission uses the owner.
impl Driver<4> for BorrowedSensor<'_> {
    fn dispatch_opened(&mut self, permit: rt::NetworkPermit) {
        assert_eq!(permit.service(), SENSOR);
        assert!(self
            .sensor
            .keys
            .iter()
            .any(|(owner, _)| *owner == permit.owner()));
        self.network = Some(permit);
    }
    fn poll_recovery(&mut self, context: &mut Context<'_>, admission: &mut dyn RecoveryAdmission) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        {
            let mut wake = pending.ready.wake.lock().unwrap();
            if !pending.ready.open.load(Ordering::SeqCst) {
                *wake = Some(context.waker().clone());
                return;
            }
        }
        admission
            .validate_existing(
                pending.identity,
                SENSOR,
                rt::MutationId(pending.measurement.id),
                pending.permit.as_ref().unwrap(),
            )
            .unwrap();
        let id = pending.measurement.id;
        let value = pending.measurement.value;
        let rows = self.measurements().unwrap();
        assert!(rows
            .as_chunks::<17>()
            .0
            .iter()
            .any(|row| row[..16] == id && row[16] == value));
        admission
            .settle_existing(
                &mut self.pending.as_mut().unwrap().permit,
                rt::Settlement::Committed,
            )
            .unwrap();
        assert!(self.pending.as_ref().unwrap().permit.is_none());
        self.pending = None;
    }
    fn activation_plan(
        &self,
        record: &IdentityRecord<4>,
    ) -> Result<rt::ActivationPlan<4>, rt::Fault> {
        self.sensor.activation_plan(record)
    }
    fn dependencies(
        &mut self,
        identity: IdentityReference,
        services: ServiceSet<4>,
    ) -> Result<DependencyReport<4>, rt::Fault> {
        self.sensor.dependencies(identity, services)
    }
    fn bind_key(&mut self, owner: rt::OwnerToken, key: KeyLease) {
        self.sensor.bind_key(owner, key);
    }
    fn classify_failure(&self, error: &rt::Fault) -> ServiceFailure {
        self.sensor.classify_failure(error)
    }
}
impl rt::configuration::ServiceInitializer for BorrowedSensor<'_> {
    type Error = rt::Fault;
    fn initialize_service(
        &mut self,
        ticket: EffectTicket,
        identity: IdentityGuard,
        key: KeyReference,
        service: ServiceId,
    ) -> Result<InitializedService, Self::Error> {
        self.sensor
            .initialize_service(ticket, identity, key, service)
    }
}
impl rt::ScopedDriver<4> for BorrowedSensor<'_> {
    type Error = rt::Fault;
    fn preflight(
        &mut self,
        scope: rt::Scope,
        plan: &rt::ActivationPlan<4>,
    ) -> Result<rt::Progress<rt::Evidence<rt::Resources>>, Self::Error> {
        self.sensor.preflight(scope, plan)
    }
    fn prepare_service(
        &mut self,
        scope: rt::Scope,
        key: KeyReference,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::PreparedService>>, Self::Error> {
        self.sensor.prepare_service(scope, key, plan)
    }
    fn activate_service(
        &mut self,
        scope: rt::Scope,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<()>>, Self::Error> {
        self.sensor.activate_service(scope, plan)
    }
    fn fence_previous_workers(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<bool>>, Self::Error> {
        self.sensor.fence_previous_workers(scope)
    }
    fn quiesce_service(
        &mut self,
        scope: rt::Scope,
        plan: rt::ServicePlan,
    ) -> Result<rt::Progress<rt::Evidence<rt::QuiescedService>>, Self::Error> {
        self.sensor.quiesce_service(scope, plan)
    }
    fn release_identity(
        &mut self,
        scope: rt::Scope,
    ) -> Result<rt::Progress<rt::Evidence<rt::ReleasedIdentity<4>>>, Self::Error> {
        self.sensor.release_identity(scope)
    }
}
fn measurement(
    commands: &Commands<SensorJob>,
    identity: IdentityReference,
    id: u8,
    value: u8,
) -> Result<u8, SensorError> {
    let (reply, result) = std::sync::mpsc::channel();
    commands
        .submit(SensorJob::Measure {
            identity,
            measurement: Measurement {
                id: [id; 16],
                value,
            },
            reply,
        })
        .unwrap();
    result.recv_timeout(Duration::from_secs(5)).unwrap()
}
fn borrowed_sensor_worker(
    root: &Root,
    domain: &Root,
    held: Arc<Mutex<Option<KeyLease>>>,
) -> prns_storage::Worker<Job<SensorJob>, prns_identities::owned::Error<rt::Fault>> {
    prns_storage::Worker::start("borrowed-sensor", options().queue_capacity, {
        let root = root.0.clone();
        let domain = domain.0.clone();
        let held = held.clone();
        move |inbox: prns_storage::Inbox<Job<SensorJob>>| {
            let mut store = prns_identities::IdentityStore::<4, 32, 4>::create_new(root)?;
            let mut log = std::fs::OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(domain.join("measurements"))
                .map_err(prns_identities::Error::Io)?;
            prns_identities::owned::run::<4, 32, 4, 16, _, SensorJob>(
                inbox,
                &mut store,
                options(),
                DiagnosticObserver::default(),
                |wake| {
                    let activation = Arc::new(Gate::default());
                    activation.release();
                    let drain = Arc::new(Gate::default());
                    drain.release();
                    Ok(BorrowedSensor {
                        sensor: Sensor {
                            root: domain,
                            wake,
                            activation,
                            drain,
                            started: Arc::new(AtomicBool::new(false)),
                            fail_drain: Arc::new(AtomicBool::new(false)),
                            panic_activation: false,
                            keys: Vec::new(),
                            effect: None,
                            held,
                        },
                        log: &mut log,
                        pending: None,
                        network: None,
                    })
                },
            )
        }
    })
    .unwrap()
}
#[test]
fn borrowed_native_command_lane_recovers_before_fresh_admission_and_keeps_storage_after_pause() {
    let root = Root::new();
    let domain = Root::new();
    std::fs::create_dir(&domain.0).unwrap();
    let held = Arc::new(Mutex::new(None));
    let worker = borrowed_sensor_worker(&root, &domain, held.clone());
    let client = Client::from_lane(worker.client(), worker.control_client(), 32, 4);
    let commands = Commands::from_lane(worker.client());
    let identity = create(&client, 1, ServiceSet::empty());
    let (add, prepared) = begin(&client, &identity, TransitionTarget::AddService(SENSOR));
    completed(&client, add, prepared.clone());
    let (enable, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    completed(&client, enable, prepared.clone());
    held.lock().unwrap().take();
    assert_eq!(
        measurement(&commands, identity.reference(), 1, 19).unwrap(),
        19
    );
    let prepared = wait(identity.prepare(TransitionTarget::Pause)).unwrap();
    let pause = wait(client.reserve(prepared.clone(), CommandDigest([11; 32]))).unwrap();
    let (entered, entry) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    commands
        .submit(SensorJob::Block {
            entered,
            release: released,
        })
        .unwrap();
    entry.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut queued = Vec::new();
    for id in 10..18 {
        let (reply, result) = std::sync::mpsc::channel();
        commands
            .submit(SensorJob::Measure {
                identity: identity.reference(),
                measurement: Measurement {
                    id: [id; 16],
                    value: id,
                },
                reply,
            })
            .unwrap();
        queued.push(result);
    }
    assert!(matches!(
        wait(client.identities()),
        Err(ApiError::Submission {
            reason: SubmitFailure::Full
        })
    ));
    // Only identity stopping commands carry the reserved control capability.
    let stopping = admit(identity.begin(pause, prepared.clone()));
    assert!(matches!(
        wait(identity.prepare(TransitionTarget::Pause)),
        Err(ApiError::Submission {
            reason: SubmissionFailure::Full
        })
    ));
    release.send(()).unwrap();
    wait(stopping).unwrap();
    for result in queued {
        assert!(matches!(
            result.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(SensorError::Admission(AdmissionError::Authority(
                Fault::Busy | Fault::Paused
            )))
        ));
    }
    completed(&client, pause, prepared.clone());
    assert_eq!(
        measurement(&commands, identity.reference(), 1, 19).unwrap(),
        19
    );
    assert!(matches!(
        measurement(&commands, identity.reference(), 1, 20),
        Err(SensorError::Conflict)
    ));
    assert!(matches!(
        measurement(&commands, identity.reference(), 2, 25),
        Err(SensorError::Admission(AdmissionError::Authority(
            Fault::Paused
        )))
    ));
    let (reply, result) = std::sync::mpsc::channel();
    commands
        .submit(SensorJob::Label {
            identity: identity.reference(),
            reply,
        })
        .unwrap();
    result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read(domain.0.join("private-label")).unwrap(),
        b"sensor"
    );
    let (retire, prepared) = begin(&client, &identity, TransitionTarget::Retire);
    completed(&client, retire, prepared.clone());
    let (reply, result) = std::sync::mpsc::channel();
    commands
        .submit(SensorJob::Read {
            identity: identity.reference(),
            reply,
        })
        .unwrap();
    assert_eq!(
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .len(),
        17
    );
    let (reply, result) = std::sync::mpsc::channel();
    commands
        .submit(SensorJob::Label {
            identity: identity.reference(),
            reply,
        })
        .unwrap();
    assert!(matches!(
        result.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(SensorError::Admission(AdmissionError::Authority(
            Fault::Retired
        )))
    ));
    worker.request_close();
    wait(worker.closed()).unwrap();
}

#[test]
fn atomic_stop_uses_protected_journal_capacity_and_recovers_a_dropped_reply() {
    type Small = Owner<1, 8, 4, 16, IdentityOnly>;
    let root = Root::new();
    let owner = Small::create_new(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let mut ordinary_full = false;
    for value in 1..10 {
        let prepared = wait(identity.prepare(TransitionTarget::Enable)).unwrap();
        match wait(client.reserve(prepared.clone(), CommandDigest([value; 32]))) {
            Ok(token) => {
                wait(identity.begin(token, prepared.clone())).unwrap();
                completed(&client, token, prepared.clone());
            }
            Err(error) => {
                assert!(matches!(
                    &error,
                    ApiError::Authority {
                        reason: Fault::CapacityExceeded
                    }
                ));
                ordinary_full = true;
                break;
            }
        }
    }
    assert!(ordinary_full);
    let prepared = wait(identity.prepare(TransitionTarget::Pause)).unwrap();
    assert!(matches!(
        wait(client.reserve(prepared.clone(), CommandDigest([12; 32]))).unwrap_err(),
        ApiError::Authority {
            reason: Fault::CapacityExceeded
        }
    ));
    drop(admit(
        identity.apply_stop(prepared.clone(), CommandDigest([13; 32])),
    ));
    // Wait for the control slot to be dispatched, then retry exact retained
    // intent, not a fresh reservation. No control or result journal is replaced.
    wait(identity.snapshot()).unwrap();
    let token = wait(identity.apply_stop(prepared.clone(), CommandDigest([13; 32]))).unwrap();
    assert!(matches!(
        completed(&client, token, prepared.clone()),
        OperationResult::Paused { .. }
    ));
    assert_eq!(
        wait(identity.apply_stop(prepared.clone(), CommandDigest([13; 32]))).unwrap(),
        token
    );
    wait(client.prune_completed(token.sequence)).unwrap();
    assert!(matches!(
        wait(client.operation(token, Command::Transition(prepared.clone()))).unwrap_err(),
        ApiError::Authority {
            reason: Fault::ReplayExpired
        }
    ));
    let (enabled, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    assert!(matches!(
        completed(&client, enabled, prepared.clone()),
        OperationResult::Enabled { .. }
    ));
    wait(owner.close()).unwrap();
}

#[test]
fn lost_inert_reservation_reply_is_queryable_and_explicitly_cancelled_before_pruning() {
    let root = Root::new();
    let owner = Only::create_new(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    let client = owner.client();
    let identity = create(&client, 1, ServiceSet::empty());
    let prepared = wait(identity.prepare(TransitionTarget::Enable)).unwrap();
    let digest = CommandDigest([22; 32]);
    drop(admit(client.reserve(prepared.clone(), digest)));
    let token = wait(client.find_reserved(Command::Transition(prepared.clone()), digest))
        .unwrap()
        .unwrap();
    assert_eq!(
        wait(client.operation(token, Command::Transition(prepared.clone()))).unwrap(),
        OperationState::Reserved
    );
    assert!(wait(client.operations())
        .unwrap()
        .iter()
        .any(|operation| operation.token == token));
    assert_eq!(
        wait(identity.snapshot()).unwrap().identity.desired,
        DesiredState::Paused
    );
    assert!(matches!(
        wait(client.prune_completed(token.sequence)).unwrap_err(),
        ApiError::Authority {
            reason: Fault::CannotPrunePending
        }
    ));
    wait(client.cancel_reserved(token)).unwrap();
    wait(client.prune_completed(token.sequence)).unwrap();
    assert!(matches!(
        wait(client.operation(token, Command::Transition(prepared.clone()))).unwrap_err(),
        ApiError::Authority {
            reason: Fault::ReplayExpired
        }
    ));
    wait(owner.close()).unwrap();
}

#[path = "owned/dispatch.rs"]
mod dispatch_tests;

#[test]
fn retained_original_mutation_recovers_without_client_jobs_during_close() {
    let root = Root::new();
    let domain = Root::new();
    std::fs::create_dir(&domain.0).unwrap();
    let held = Arc::new(Mutex::new(None));
    let worker = borrowed_sensor_worker(&root, &domain, held.clone());
    let client = Client::from_lane(worker.client(), worker.control_client(), 32, 4);
    let identity = create(&client, 1, ServiceSet::empty());
    let (add, prepared) = begin(&client, &identity, TransitionTarget::AddService(SENSOR));
    completed(&client, add, prepared.clone());
    let (enable, prepared) = begin(&client, &identity, TransitionTarget::Enable);
    completed(&client, enable, prepared.clone());
    held.lock().unwrap().take();
    let before = wait(identity.snapshot()).unwrap().identity;
    let ready = Arc::new(Gate::default());
    let (entered, observed) = std::sync::mpsc::channel();
    Commands::from_lane(worker.client())
        .submit(SensorJob::DeferMeasure {
            identity: identity.reference(),
            measurement: Measurement {
                id: [31; 16],
                value: 72,
            },
            ready: ready.clone(),
            entered,
        })
        .unwrap();
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    let completion = worker.closed();
    worker.request_close();
    drop(worker);
    drop(client);
    assert!(completion.status().is_none());
    assert!(Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)).is_err());
    // Only the retained effect's weak waker can make progress now; ordinary
    // extension requests have closed. No caller retries drive this recovery.
    ready.release();
    wait(completion).unwrap();
    let rows = std::fs::read(domain.0.join("measurements")).unwrap();
    assert_eq!(rows.len(), 17);
    assert_eq!(&rows[..16], &[31; 16]);
    assert_eq!(rows[16], 72);
    let owner = Only::open_existing(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    assert_eq!(
        wait(
            owner
                .client()
                .identity(before.reference())
                .unwrap()
                .snapshot()
        )
        .unwrap()
        .identity,
        before
    );
    wait(owner.close()).unwrap();
}

#[cfg(feature = "contact-card")]
#[path = "owned/contact_card.rs"]
mod contact_card;

struct CountWake {
    inner: Waker,
    calls: Arc<AtomicUsize>,
}
impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.wake_by_ref();
    }
}
fn recovery_sensor(
    path: PathBuf,
    activation: Arc<Gate>,
    drain: Arc<Gate>,
    started: Arc<AtomicBool>,
    held: Arc<Mutex<Option<KeyLease>>>,
    wakes: Arc<AtomicUsize>,
) -> impl FnOnce(Waker) -> Result<Sensor, rt::Fault> {
    move |wake| {
        Ok(Sensor {
            root: path,
            wake: Waker::from(Arc::new(CountWake {
                inner: wake,
                calls: wakes,
            })),
            activation,
            drain,
            started,
            fail_drain: Arc::new(AtomicBool::new(false)),
            panic_activation: false,
            keys: Vec::new(),
            effect: None,
            held,
        })
    }
}

#[test]
fn recovery_cleanup_busy_waits_for_owned_wake_without_caller_pumping_or_spin() {
    type SensorOwner = Owner<4, 32, 4, 16, Sensor>;
    let root = Root::new();
    let domain = Root::new();
    std::fs::create_dir(&domain.0).unwrap();
    let activation = Arc::new(Gate::default());
    activation.release();
    let drain = Arc::new(Gate::default());
    drain.release();
    let held = Arc::new(Mutex::new(None));
    let started = Arc::new(AtomicBool::new(false));
    let wakes = Arc::new(AtomicUsize::new(0));
    let owner = SensorOwner::create_new(
        &root.0,
        options(),
        recovery_sensor(
            domain.0.clone(),
            activation.clone(),
            drain,
            started.clone(),
            held.clone(),
            wakes.clone(),
        ),
    )
    .unwrap();
    let catalog = owner.client();
    let first = create(&catalog, 81, ServiceSet::empty());
    let second = create(&catalog, 82, ServiceSet::empty());
    for identity in [&first, &second] {
        let (token, prepared) = begin(&catalog, identity, TransitionTarget::AddService(SENSOR));
        completed(&catalog, token, prepared);
        let (token, prepared) = begin(&catalog, identity, TransitionTarget::Enable);
        completed(&catalog, token, prepared);
    }
    let first_ref = first.reference();
    let second_ref = second.reference();
    held.lock().unwrap().take();
    wait(owner.close()).unwrap();
    // The process stopped after accepting Pause, before its physical drain.
    let (stop, command) = {
        let mut disk = prns_identities::IdentityStore::<4, 32, 4>::open(&root.0).unwrap();
        let mut current = Catalog::<4, 32, 4>::new([0; 16]);
        let mut staged = current.clone();
        let mut coordinator = Coordinator::open(&mut disk, &mut current, &mut staged).unwrap();
        let prepared = coordinator
            .prepare(
                second_ref.id,
                TransitionTarget::Pause,
                DependencyReport::new(&[Sensor::dependency()], 0).unwrap(),
            )
            .unwrap();
        let command = prns_identities_core::Command::Transition(prepared);
        let token = coordinator
            .reserve(&mut disk, command, CommandDigest([83; 32]))
            .unwrap();
        coordinator.begin(&mut disk, token, &command).unwrap();
        (token, Command::from(command))
    };
    let drain = Arc::new(Gate::default());
    started.store(false, Ordering::SeqCst);
    let owner = SensorOwner::open_existing(
        &root.0,
        options(),
        recovery_sensor(
            domain.0.clone(),
            activation,
            drain.clone(),
            started.clone(),
            held.clone(),
            wakes.clone(),
        ),
    )
    .unwrap();
    let catalog = owner.client();
    let first = catalog.identity(first_ref).unwrap();
    let second = catalog.identity(second_ref).unwrap();
    wait(second.recover()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while drain.wake.lock().unwrap().is_none() {
        assert!(Instant::now() < deadline, "retained cleanup did not start");
        std::thread::sleep(Duration::from_millis(2));
    }
    wait(first.recover()).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert!(!started.load(Ordering::SeqCst));
    assert!(owner.diagnostics().latest().is_none());
    let quiet = wakes.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        quiet,
        "Busy self-woke the worker"
    );
    drain.release();
    // No reads/recovery retries enter the lane after the wake. The other
    // identity must reach actual native activation using retained scheduling.
    while !started.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "cleanup completion stranded activation"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(owner.diagnostics().latest().is_none());
    assert!(matches!(
        wait(catalog.operation(stop, command)).unwrap(),
        OperationState::Completed(OperationResult::Paused { .. })
    ));
    held.lock().unwrap().take();
    wait(owner.close()).unwrap();
}
