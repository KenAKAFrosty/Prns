#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{local, stores};
use groups::Clock as _;
use owned::Driver as _;
use prns_host_runtime::{owner::OwnedSession, HostEmbedding};
use prns_identities::{api, creation::CreateRequestId};
use std::{
    future::Future,
    num::NonZeroUsize,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

const QUERY_PAGE: prns_groups::PageLimits = prns_groups::PageLimits {
    max_rows: 64,
    max_bytes: 64 * 1024,
};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "prns-network-owner-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn executor() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
async fn within<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(8), future)
        .await
        .expect("retained native operation stalled")
}
fn options() -> OwnerOptions {
    OwnerOptions {
        storage: assembly::Options::new(
            local::Options {
                identities: owned::Options {
                    queue_capacity: NonZeroUsize::new(16).unwrap(),
                    budget: runtime::Budget {
                        capacity: runtime::Resources {
                            held_keys: 2,
                            destinations: 4,
                            request_handlers: 4,
                            links: 2,
                            queue_slots: 32,
                        },
                        infrastructure_reserve: runtime::Resources::ZERO,
                    },
                },
                stores: stores::Options {
                    per_identity: prns_storage::database::Options {
                        cache_bytes: 1024 * 1024,
                    },
                    total_cache_bytes: 2 * 1024 * 1024,
                },
            },
            groups::retained::Limits {
                max_operations: 16,
                max_bytes: 1024 * 1024,
                ..Default::default()
            },
            groups::enrollment::CatalogLimits::default(),
            groups::ClientLimits::default(),
        ),
        service: RunOptions {
            carrier: Options {
                operations: 4,
                outgoing_links: 1,
                sync: transport::Limits {
                    max_frame_bytes: 131_072,
                    max_page_items: 8,
                    max_peers: 4,
                    inbound_jobs: 2,
                    exchange_timeout: Duration::from_secs(1),
                },
                enrollment: transport::enrollment::Limits {
                    max_frame_bytes: 131_072,
                    inbound_jobs: 2,
                    exchange_timeout: Duration::from_secs(1),
                },
            },
            policy: groups::SyncPolicy::default(),
            announce_interval_millis: None,
        },
    }
}
async fn host() -> (Arc<OwnedSession>, Context) {
    let router = Router::new(2).unwrap();
    let mut embedding = HostEmbedding::default();
    router.install(&mut embedding).unwrap();
    let host = OwnedSession::open_with_embedding(
        prns_host::HostConfig {
            identity: prns_host::IdentityConfig::GenerateEphemeral,
            persistence: prns_host::PersistenceConfig::Ephemeral,
            role: prns_host::HostRole::Endpoint,
            destinations: vec![],
            required_capabilities: vec![],
            limits: prns_host::PrnsLimits::balanced(),
        },
        embedding,
    )
    .await
    .unwrap();
    let context =
        Context::attach(host.client(), tokio::runtime::Handle::current(), router).unwrap();
    (host, context)
}
async fn create(client: &owned::Client, key: u8) -> owned::IdentityClient {
    let secret = prns_identities::IdentitySecretKey::new([key; 64]);
    let expected_hash = core::PublicIdentityHash(
        *prns_core::identity::PrivateIdentityMaterial::from(secret.clone())
            .identity_hash()
            .as_bytes(),
    );
    let scope = client.creation_scope().await.unwrap();
    let result = client
        .create_or_recover(
            CreateRequestId {
                scope,
                nonce: [key; 32],
            },
            api::CreateRequest {
                source: core::KeySource::Import { expected_hash },
                metadata: core::Metadata::new("standalone network", None).unwrap(),
                services: vec![],
            },
            Some(Arc::new(secret.into())),
        )
        .await
        .unwrap();
    let core::OperationResult::Created { identity } = result.result else {
        panic!("unexpected identity creation result");
    };
    let record = client
        .identities()
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.id == identity)
        .unwrap();
    client.identity(record.reference()).unwrap()
}
async fn transition(
    client: &owned::Client,
    identity: &owned::IdentityClient,
    target: core::TransitionTarget,
) -> core::OperationResult {
    within(async {
        let prepared = loop {
            match identity.prepare(target).await {
                Ok(prepared) => break prepared,
                Err(_) => tokio::time::sleep(Duration::from_millis(2)).await,
            }
        };
        let token = client
            .reserve(prepared.clone(), core::CommandDigest([5; 32]))
            .await
            .unwrap();
        identity.begin(token, prepared.clone()).await.unwrap();
        loop {
            if let core::OperationState::Completed(result) = client
                .operation(token, api::Command::Transition(prepared.clone()))
                .await
                .unwrap()
            {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
}
async fn configured(client: &owned::Client, key: u8) -> owned::IdentityClient {
    let identity = create(client, key).await;
    assert!(matches!(
        transition(
            client,
            &identity,
            core::TransitionTarget::AddService(SERVICE)
        )
        .await,
        core::OperationResult::ServiceAdded { .. }
    ));
    identity
}
async fn retire(endpoint: &ServiceEndpoint) {
    let pause = endpoint.prepare_pause(vec![]).await.unwrap();
    within(async {
        while !pause.is_drained() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    endpoint.finish_pause(&pause).await.unwrap();
}
fn registration(
    key: u8,
    enrollment: bool,
) -> prns_host_runtime::service_registration::ServiceRegistration {
    let options = options().service.carrier;
    let config = if enrollment {
        transport::enrollment::destination_config(options.enrollment).unwrap()
    } else {
        transport::destination_config(options.sync).unwrap()
    };
    staged_service_registration(config, IdentitySecretKey::new([key; 64])).unwrap()
}
async fn assert_registered(host: &OwnedSession, key: u8) {
    let endpoint = host
        .client()
        .service_client()
        .unwrap()
        .protocols()
        .service_endpoint();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match endpoint.register(registration(key, false)).await {
            Err(ServiceLifecycleError::Conflict) => return,
            Ok(_) => panic!("canonical activation completed before its retained registration"),
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "endpoint registration probe: {error:?}"
                );
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    }
}

#[test]
fn public_factory_drains_without_changing_enablement_and_strictly_reopens() {
    executor().block_on(async {
        let root = Root::new();
        let (host, context) = host().await;
        let owner = create_new::<2, 24, 2, 8>(&root.0, options(), context.clone()).unwrap();
        let client = owner.identities();
        let identity = configured(&client, 41).await;
        assert!(matches!(
            transition(&client, &identity, core::TransitionTarget::Enable).await,
            core::OperationResult::Enabled { .. }
        ));
        assert_registered(&host, 41).await;
        let before = identity.snapshot().await.unwrap().identity;
        let group_client = assembly::client(
            owner.submission(),
            identity.reference(),
            super::super::Command::Group,
        );
        assert_eq!(
            group_client.identity().await.unwrap().as_bytes(),
            &identity.reference().public_hash.0
        );
        let completion = owner.close();
        drop(owner);
        within(completion).await.unwrap();
        assert!(group_client.identity().await.is_err());

        // Both real endpoint tokens have retired before the catalog lock reopens.
        for enrollment in [false, true] {
            let endpoint = host
                .client()
                .service_client()
                .unwrap()
                .protocols()
                .service_endpoint();
            endpoint
                .register(registration(41, enrollment))
                .await
                .unwrap();
            retire(&endpoint).await;
        }
        let owner = open_existing::<2, 24, 2, 8>(&root.0, options(), context.clone()).unwrap();
        let identity = owner.identities().identity(identity.reference()).unwrap();
        let reopened = identity.snapshot().await.unwrap();
        assert_eq!(reopened.identity, before);
        assert_eq!(reopened.runtime, core::RuntimeStatus::Unavailable);
        identity.recover().await.unwrap();
        within(async {
            while identity.snapshot().await.unwrap().runtime != core::RuntimeStatus::Running {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        assert_registered(&host, 41).await;
        within(owner.close()).await.unwrap();
        assert!(
            context.diagnostics().latest().is_none(),
            "{:?}",
            context.diagnostics().latest()
        );
        host.stop().await.unwrap();
    });
}

#[test]
fn invalid_service_bounds_have_no_file_effects() {
    executor().block_on(async {
        let root = Root::new();
        let (host, context) = host().await;
        let mut invalid = options();
        invalid.service.carrier.operations = 0;
        assert!(create_new::<2, 24, 2, 8>(&root.0, invalid, context).is_err());
        assert!(!root.0.exists());
        host.stop().await.unwrap();
    });
}

#[test]
fn carrier_payload_allowance_also_funds_the_owned_job_before_file_effects() {
    executor().block_on(async {
        let root = Root::new();
        let (host, context) = host().await;
        let mut invalid = options();
        let payload = invalid.service.carrier.enrollment.max_frame_bytes as u64;
        invalid.storage = invalid.storage.with_client_limits(groups::ClientLimits {
            max_job_bytes: payload,
            max_pending_job_bytes: payload * 2,
        });
        assert!(create_new::<2, 24, 2, 8>(&root.0, invalid, context).is_err());
        assert!(!root.0.exists());
        host.stop().await.unwrap();
    });
}

#[test]
fn carrier_capabilities_reject_fresh_and_persisted_profiles_without_changing_them() {
    executor().block_on(async {
        let root = Root::new();
        let (host, context) = host().await;
        let profile = groups::GroupProfile::default();
        let mut narrow = options();
        // The owning projection accounts for the Submit wrapper, so a frame
        // equal to the MLS wire bound cannot carry that bound.
        narrow.service.carrier.sync.max_frame_bytes = profile.max_wire_bytes as usize;
        let owner = create_new::<2, 24, 2, 8>(&root.0, narrow, context.clone()).unwrap();
        let identities = owner.identities();
        let identity = configured(&identities, 51).await;
        assert!(matches!(
            transition(&identities, &identity, core::TransitionTarget::Enable).await,
            core::OperationResult::Enabled { .. }
        ));
        let client = assembly::client(
            owner.submission(),
            identity.reference(),
            super::super::Command::Group,
        );
        let intent = || groups::Intent::Create {
            profile,
            title: "Exact carrier support".into(),
        };
        let execute = |id| groups::Request {
            id,
            intent: intent(),
            original_scope: None,
        };
        let context_now = || groups::ExecutionContext {
            clock: groups::SystemClock.now(),
        };
        let rejected = groups::create_request_id().unwrap();
        assert_eq!(
            client.execute(execute(rejected), context_now()).await,
            Err(groups::ApiError::Rejected {
                reason: prns_groups_core::RejectionReason::UnsupportedProfile,
            })
        );
        assert_eq!(
            client.operation(rejected).await.unwrap().unwrap().state,
            groups::retained::State::Rejected(
                prns_groups_core::RejectionReason::UnsupportedProfile
            )
        );
        let page = client.conversations(None, QUERY_PAGE).await.unwrap();
        assert!(page.items.is_empty() && page.next.is_none());
        within(owner.close()).await.unwrap();

        let broad = open_existing::<2, 24, 2, 8>(&root.0, options(), context.clone()).unwrap();
        let identity = broad.identities().identity(identity.reference()).unwrap();
        identity.recover().await.unwrap();
        within(async {
            while identity.snapshot().await.unwrap().runtime != core::RuntimeStatus::Running {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        let client = assembly::client(
            broad.submission(),
            identity.reference(),
            super::super::Command::Group,
        );
        let created = client
            .execute(execute(groups::create_request_id().unwrap()), context_now())
            .await
            .unwrap();
        assert!(matches!(created, groups::Outcome::Created(_)));
        within(broad.close()).await.unwrap();

        let mut narrow = options();
        narrow.service.carrier.enrollment.max_frame_bytes =
            profile.max_enrollment_frame_bytes as usize - 1;
        assert!(matches!(
            open_existing::<2, 24, 2, 8>(&root.0, narrow, context.clone()),
            Err(local::Error::Identity(owned::Error::Driver(
                DriverError::Local(assembly::Error::Schema(assembly::SchemaError::Group(
                    groups::Error::Enrollment(
                        groups::enrollment::EnrollmentError::UnsupportedProfile
                    )
                )))
            )))
        ));
        // The failed network selection is read-only and released both the
        // catalog and the retained cache's database allocation.
        let restored = open_existing::<2, 24, 2, 8>(&root.0, options(), context).unwrap();
        let client = assembly::client(
            restored.submission(),
            identity.reference(),
            super::super::Command::Group,
        );
        let page = client.conversations(None, QUERY_PAGE).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.next.is_none());
        within(restored.close()).await.unwrap();
        host.stop().await.unwrap();
    });
}

#[test]
fn partial_registration_failure_retires_its_first_endpoint_and_preserves_foreign_owner() {
    executor().block_on(async {
        let root = Root::new();
        let (host, context) = host().await;
        let foreign = host
            .client()
            .service_client()
            .unwrap()
            .protocols()
            .service_endpoint();
        foreign.register(registration(43, true)).await.unwrap();
        let owner = create_new::<2, 24, 2, 8>(&root.0, options(), context.clone()).unwrap();
        let client = owner.identities();
        let identity = configured(&client, 43).await;
        assert!(matches!(
            transition(&client, &identity, core::TransitionTarget::Enable).await,
            core::OperationResult::ActivationFailed { .. }
        ));
        assert!(matches!(
            context.diagnostics().latest().unwrap().error.as_ref(),
            Failure::Service(ServiceLifecycleError::Conflict)
        ));
        within(owner.close()).await.unwrap();
        foreign.activate_ingress().await.unwrap();
        let sync = host
            .client()
            .service_client()
            .unwrap()
            .protocols()
            .service_endpoint();
        sync.register(registration(43, false)).await.unwrap();
        retire(&sync).await;
        retire(&foreign).await;
        // A new owner can use the same native Router after failed startup cleanup.
        let owner = open_existing::<2, 24, 2, 8>(&root.0, options(), context).unwrap();
        let client = owner.identities();
        let identity = client.identity(identity.reference()).unwrap();
        assert!(matches!(
            transition(&client, &identity, core::TransitionTarget::Enable).await,
            core::OperationResult::Enabled { .. }
        ));
        within(owner.close()).await.unwrap();
        host.stop().await.unwrap();
    });
}

// A selected native test domain adds observation commands without changing the
// group command inventory or adding another queue/authority table.
// The shared bounded lane keeps one fixed job size without an extra allocation.
#[allow(clippy::large_enum_variant)]
enum TestCommand {
    Group(assembly::Command),
    Overflow(oneshot::Sender<()>),
    Endpoints(oneshot::Sender<[ServiceEndpoint; 2]>),
    ReleaseStartup,
    #[cfg(feature = "contact-card")]
    ContactInvariants(oneshot::Sender<()>),
    #[cfg(feature = "contact-card")]
    ContactState(oneshot::Sender<(HostServiceScope, Option<String>, Option<String>)>),
    HoldLane(
        oneshot::Sender<DispatchLease>,
        std::sync::mpsc::Receiver<()>,
    ),
}
impl owned::CommandHandler<2, TestCommand> for Driver<2, 2, TestCommand> {
    fn command(&mut self, job: TestCommand, admission: &mut dyn owned::Admission) {
        match job {
            TestCommand::Group(job) => self.local.command(job, admission),
            #[cfg(feature = "contact-card")]
            TestCommand::ContactState(reply) => {
                let entry = self.owners.iter().find(|entry| entry.opened).unwrap();
                let _ = reply.send((
                    entry.route.as_ref().unwrap().scope().clone(),
                    entry.contact.installed.map(|name| name.as_str().to_owned()),
                    entry.contact.name.map(|name| name.as_str().to_owned()),
                ));
            }
            #[cfg(feature = "contact-card")]
            TestCommand::ContactInvariants(reply) => {
                self.check_contact_refresh_retains_manual_intent();
                let _ = reply.send(());
            }
            TestCommand::ReleaseStartup => unreachable!("wrapper owns the held notification"),
            TestCommand::HoldLane(entered, release) => {
                let entry = self
                    .owners
                    .iter()
                    .find(|entry| entry.runner.is_some())
                    .unwrap();
                entered
                    .send(entry.dispatch.clone())
                    .unwrap_or_else(|_| panic!("lane probe dropped"));
                release.recv_timeout(Duration::from_secs(8)).unwrap();
            }
            TestCommand::Endpoints(reply) => {
                let entry = self
                    .owners
                    .iter()
                    .find(|entry| entry.runner.is_some())
                    .unwrap();
                let _ = reply.send(entry.endpoints.as_ref().unwrap().clone());
            }
            TestCommand::Overflow(reply) => {
                let entry = self
                    .owners
                    .iter()
                    .find(|entry| entry.dispatch.is_open() && !entry.quiesced)
                    .unwrap();
                let scope = entry.route.as_ref().unwrap().scope();
                let reservation = scope.reserve_link().unwrap();
                for byte in [81, 82] {
                    scope.on_event_ref(&prns_runtime::runtime::PrnsEvent::Diagnostic(
                        prns_runtime::runtime::Diagnostic::LinkClosed {
                            link_id: prns_runtime::routing::links::LinkId::new([byte; 16]),
                            reason: prns_runtime::engine::LinkClosedReason::LocallyClosed,
                        },
                    ));
                }
                reservation.track(prns_host::LinkId::new([82; 16]));
                assert_eq!(
                    scope.outgoing_links().unwrap(),
                    vec![prns_host::LinkId::new([82; 16])]
                );
                let _ = reply.send(());
            }
        }
    }
    fn reject(job: TestCommand) {
        if let TestCommand::Group(job) = job {
            Local::<2>::reject(job);
        }
    }
}
struct StartupRelease(Option<std::sync::mpsc::Sender<()>>);
impl Drop for StartupRelease {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}

struct StartupProbe {
    dispatch: DispatchLease,
    stopping: watch::Receiver<bool>,
    runner: tokio::task::AbortHandle,
}
struct StartupQueryGate {
    entered: Option<oneshot::Sender<StartupProbe>>,
    run: std::sync::mpsc::Receiver<()>,
    completed: oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    failure: Option<groups::Error>,
}

struct Capturing {
    inner: Driver<2, 2, TestCommand>,
    permit: Arc<std::sync::Mutex<Option<runtime::NetworkPermit>>>,
    hold_start: bool,
    startup_query: Option<StartupQueryGate>,
}
impl owned::Driver<2> for Capturing {
    fn poll_recovery(
        &mut self,
        context: &mut TaskContext<'_>,
        admission: &mut dyn owned::RecoveryAdmission,
    ) {
        self.inner.poll_recovery(context, admission);
    }
    fn poll_observations(&mut self, context: &mut TaskContext<'_>) {
        self.inner.poll_observations(context);
    }
    fn bind_dispatch(&mut self, owner: runtime::OwnerToken, lease: DispatchLease) {
        self.inner.bind_dispatch(owner, lease);
    }
    fn validate_metadata(&self, metadata: core::Metadata) -> Result<(), core::Fault> {
        self.inner.validate_metadata(metadata)
    }
    fn bind_metadata(
        &mut self,
        owner: runtime::OwnerToken,
        identity: core::IdentityReference,
        metadata: core::Metadata,
    ) {
        self.inner.bind_metadata(owner, identity, metadata);
    }
    fn bind_key(&mut self, owner: runtime::OwnerToken, key: KeyLease) {
        self.inner.bind_key(owner, key);
    }
    fn dispatch_opened(&mut self, permit: runtime::NetworkPermit) {
        *self.permit.lock().unwrap() = Some(permit);
        if !self.hold_start {
            self.inner.dispatch_opened(permit);
        }
    }
    fn activation_plan(
        &self,
        record: &core::IdentityRecord<2>,
    ) -> Result<runtime::ActivationPlan<2>, DriverError> {
        self.inner.activation_plan(record)
    }
    fn dependencies(
        &mut self,
        identity: core::IdentityReference,
        services: core::ServiceSet<2>,
    ) -> Result<core::DependencyReport<2>, DriverError> {
        self.inner.dependencies(identity, services)
    }
    fn classify_failure(&self, error: &DriverError) -> core::ServiceFailure {
        self.inner.classify_failure(error)
    }
}
impl runtime::configuration::ServiceInitializer for Capturing {
    type Error = DriverError;
    fn initialize_service(
        &mut self,
        ticket: core::EffectTicket,
        identity: core::IdentityGuard,
        key: core::KeyReference,
        service: core::ServiceId,
    ) -> Result<core::InitializedService, DriverError> {
        self.inner
            .initialize_service(ticket, identity, key, service)
    }
}
impl runtime::ScopedDriver<2> for Capturing {
    type Error = DriverError;
    fn preflight(
        &mut self,
        scope: runtime::Scope,
        plan: &runtime::ActivationPlan<2>,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::Resources>>, DriverError> {
        self.inner.preflight(scope, plan)
    }
    fn prepare_service(
        &mut self,
        scope: runtime::Scope,
        key: core::KeyReference,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::PreparedService>>, DriverError> {
        self.inner.prepare_service(scope, key, plan)
    }
    fn activate_service(
        &mut self,
        scope: runtime::Scope,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<()>>, DriverError> {
        self.inner.activate_service(scope, plan)
    }
    fn fence_previous_workers(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<bool>>, DriverError> {
        self.inner.fence_previous_workers(scope)
    }
    fn quiesce_service(
        &mut self,
        scope: runtime::Scope,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::QuiescedService>>, DriverError> {
        self.inner.quiesce_service(scope, plan)
    }
    fn release_identity(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::ReleasedIdentity<2>>>, DriverError>
    {
        self.inner.release_identity(scope)
    }
}
impl owned::CommandHandler<2, TestCommand> for Capturing {
    fn command(&mut self, job: TestCommand, admission: &mut dyn owned::Admission) {
        if let TestCommand::ReleaseStartup = job {
            self.hold_start = false;
            self.inner
                .dispatch_opened(self.permit.lock().unwrap().unwrap());
        } else {
            let mut gate = if matches!(&job, TestCommand::Group(_)) {
                self.startup_query.take()
            } else {
                None
            };
            if let Some(gate) = gate.as_mut() {
                let entry = self.inner.owners.iter().find(|entry| entry.opened).unwrap();
                gate.entered
                    .take()
                    .unwrap()
                    .send(StartupProbe {
                        dispatch: entry.dispatch.clone(),
                        stopping: entry.stop.as_ref().unwrap().subscribe(),
                        runner: entry.runner.as_ref().unwrap().completion_probe().unwrap(),
                    })
                    .unwrap_or_else(|_| panic!("startup probe waiter dropped"));
            }
            if let Some(gate) = gate.as_mut() {
                // Handshake before completing the already-admitted startup
                // read. This forces Close to precede its reply, not merely race
                // the runner's next transport constructor.
                gate.run.recv_timeout(Duration::from_secs(8)).unwrap();
            }
            match (job, gate.as_mut().and_then(|gate| gate.failure.take())) {
                (TestCommand::Group(job), Some(error)) => job.job.fail(error),
                (job, None) => self.inner.command(job, admission),
                _ => unreachable!("only a group reply can be failed"),
            }
            if let Some(gate) = gate {
                let _ = gate.completed.send(());
                // Keep the owner before its observation/Stop poll until the
                // runner has consumed that exact reply.
                gate.release.recv_timeout(Duration::from_secs(8)).unwrap();
            }
        }
    }
    fn reject(job: TestCommand) {
        Driver::<2, 2, TestCommand>::reject(job);
    }
}
#[test]
fn captured_runner_generation_is_rejected_after_pause_and_overflow_candidates_retire() {
    executor().block_on(async {
        let root = Root::new();
        let (host, context) = host().await;
        let options = options();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let permit = captured.clone();
        let input = assembly::SubmissionState::new::<2>(groups::InputBudget::default()).unwrap();
        let worker_input = input.clone();
        let owner = local::Owner::<2, 24, 2, 8, Capturing, TestCommand>::create_new(
            &root.0,
            options.storage.local,
            move |stores, catalog, wake, commands| {
                let local = assembly::factory::configure(
                    stores,
                    catalog,
                    wake.clone(),
                    options.storage,
                    worker_input,
                )?;
                Ok(Capturing {
                    inner: Driver::new(
                        local,
                        context,
                        commands,
                        TestCommand::Group,
                        options.service,
                        wake,
                    )?,
                    permit,
                    // This fixture inspects exact scope candidates before
                    // retirement, so keep unrelated runner operations staged.
                    hold_start: true,
                    startup_query: None,
                })
            },
        )
        .unwrap();
        let owner = assembly::Owner::from_local(owner, input);
        let client = owner.identities();
        let identity = configured(&client, 44).await;
        assert!(matches!(
            transition(&client, &identity, core::TransitionTarget::Enable).await,
            core::OperationResult::Enabled { .. }
        ));
        let old = assembly::network_client(
            owner.submission(),
            identity.reference(),
            captured.lock().unwrap().unwrap(),
            TestCommand::Group,
        )
        .unwrap();
        old.identity().await.unwrap();
        old.maintain(groups::SystemClock.now()).await.unwrap();
        assert!(matches!(
            transition(&client, &identity, core::TransitionTarget::Pause).await,
            core::OperationResult::Paused { .. }
        ));
        assert!(old.identity().await.is_err());
        assert!(matches!(
            transition(&client, &identity, core::TransitionTarget::Enable).await,
            core::OperationResult::Enabled { .. }
        ));
        let current = assembly::network_client(
            owner.submission(),
            identity.reference(),
            captured.lock().unwrap().unwrap(),
            TestCommand::Group,
        )
        .unwrap();
        current.identity().await.unwrap();
        assert!(old.identity().await.is_err());
        assert!(old.maintain(groups::SystemClock.now()).await.is_err());
        // A lost early LinkClosed leaves a conservative candidate. Exact endpoint
        // retirement, not an empty candidate list, permits native route release.
        let (reply, receive) = oneshot::channel();
        owner
            .commands()
            .submit(TestCommand::Overflow(reply))
            .unwrap();
        within(receive).await.unwrap();
        assert!(matches!(
            transition(&client, &identity, core::TransitionTarget::Pause).await,
            core::OperationResult::Paused { .. }
        ));
        within(owner.close()).await.unwrap();
        host.stop().await.unwrap();
    });
}

#[test]
fn post_confirmation_startup_failure_is_observable_and_pause_still_drains() {
    executor().block_on(async {
        let root = Root::new();
        let (host, context) = host().await;
        let diagnostics = context.diagnostics();
        let options = options();
        let worker_input =
            assembly::SubmissionState::new::<2>(groups::InputBudget::default()).unwrap();
        let owner = local::Owner::<2, 24, 2, 8, Capturing, TestCommand>::create_new(
            &root.0,
            options.storage.local,
            move |stores, catalog, wake, commands| {
                let local = assembly::factory::configure(
                    stores,
                    catalog,
                    wake.clone(),
                    options.storage,
                    worker_input,
                )?;
                Ok(Capturing {
                    inner: Driver::new(
                        local,
                        context,
                        commands,
                        TestCommand::Group,
                        options.service,
                        wake,
                    )?,
                    permit: Arc::new(std::sync::Mutex::new(None)),
                    hold_start: true,
                    startup_query: None,
                })
            },
        )
        .unwrap();
        let client = owner.identities();
        let identity = configured(&client, 45).await;
        assert!(matches!(
            transition(&client, &identity, core::TransitionTarget::Enable).await,
            core::OperationResult::Enabled { .. }
        ));
        let (reply, receive) = oneshot::channel();
        owner
            .commands()
            .submit(TestCommand::Endpoints(reply))
            .unwrap();
        let endpoints = within(receive).await.unwrap();
        // Force a real host-side failure after canonical confirmation, before
        // delivering the already-owned runner's post-confirmation notification.
        retire(&endpoints[0]).await;
        owner
            .commands()
            .submit(TestCommand::ReleaseStartup)
            .unwrap();
        within(async {
            while diagnostics.latest().is_none() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        assert!(matches!(
            diagnostics.latest().unwrap().error.as_ref(),
            Failure::Service(ServiceLifecycleError::NotRegistered)
        ));
        let before = identity.snapshot().await.unwrap();
        assert_eq!(before.identity.desired, core::DesiredState::Enabled);
        assert!(matches!(
            transition(&client, &identity, core::TransitionTarget::Pause).await,
            core::OperationResult::Paused { .. }
        ));
        assert!(matches!(
            endpoints[1].activate_ingress().await,
            Err(ServiceLifecycleError::NotRegistered)
        ));
        within(owner.close()).await.unwrap();
        assert!(diagnostics.latest().is_some());
        host.stop().await.unwrap();
    });
}

#[test]
fn close_between_runner_startup_reads_drains_without_a_false_failure() {
    executor().block_on(close_startup_query(None));
}

#[test]
fn admitted_startup_failure_is_not_reclassified_by_concurrent_close() {
    executor().block_on(close_startup_query(Some(groups::Error::Unavailable)));
}

async fn close_startup_query(failure: Option<groups::Error>) {
    let expect_failure = failure.is_some();
    let root = Root::new();
    let (host, context) = host().await;
    let diagnostics = context.diagnostics();
    let reopen_context = context.clone();
    let options = options();
    let (entered, query_entered) = oneshot::channel();
    let (completed, query_completed) = oneshot::channel();
    let (run, await_close) = std::sync::mpsc::channel();
    let run = StartupRelease(Some(run));
    let (release, held) = std::sync::mpsc::channel();
    let release = StartupRelease(Some(release));
    let worker_input = assembly::SubmissionState::new::<2>(groups::InputBudget::default()).unwrap();
    let owner = local::Owner::<2, 24, 2, 8, Capturing, TestCommand>::create_new(
        &root.0,
        options.storage.local,
        move |stores, catalog, wake, commands| {
            let local = assembly::factory::configure(
                stores,
                catalog,
                wake.clone(),
                options.storage,
                worker_input,
            )?;
            Ok(Capturing {
                inner: Driver::new(
                    local,
                    context,
                    commands,
                    TestCommand::Group,
                    options.service,
                    wake,
                )?,
                permit: Arc::new(std::sync::Mutex::new(None)),
                hold_start: true,
                startup_query: Some(StartupQueryGate {
                    entered: Some(entered),
                    run: await_close,
                    completed,
                    release: held,
                    failure,
                }),
            })
        },
    )
    .unwrap();
    let identity_client = owner.identities();
    let identity = configured(&identity_client, 46).await;
    assert!(matches!(
        transition(&identity_client, &identity, core::TransitionTarget::Enable).await,
        core::OperationResult::Enabled { .. }
    ));
    owner
        .commands()
        .submit(TestCommand::ReleaseStartup)
        .unwrap();
    let StartupProbe {
        dispatch,
        stopping,
        runner,
    } = within(query_entered).await.unwrap();
    assert!(dispatch.is_open());
    assert!(!*stopping.borrow());
    let completion = owner.close();
    assert!(!dispatch.is_open());
    drop(run);
    within(query_completed).await.unwrap();
    // Poll the actual retained runner after the completed profile reply. Its
    // next transport constructor sees the exact fence before the Stop watch;
    // an already-produced failure must instead retain its original result.
    within(async {
        while !runner.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(!*stopping.borrow());
    drop(release);
    within(completion).await.unwrap();
    if expect_failure {
        assert!(matches!(
            diagnostics.latest().unwrap().error.as_ref(),
            Failure::Group(groups::Error::Unavailable)
        ));
    } else {
        assert!(diagnostics.latest().is_none(), "{:?}", diagnostics.latest());
    }
    let reopened = open_existing::<2, 24, 2, 8>(&root.0, options, reopen_context).unwrap();
    assert_eq!(
        reopened
            .identities()
            .identity(identity.reference())
            .unwrap()
            .snapshot()
            .await
            .unwrap()
            .identity
            .desired,
        core::DesiredState::Enabled,
    );
    within(reopened.close()).await.unwrap();
    host.stop().await.unwrap();
}

#[test]
fn private_runner_closed_admissions_preserve_request_provenance() {
    executor().block_on(async {
        let root = Root::new();
        let (host, context) = host().await;
        let diagnostics = context.diagnostics();
        let options = options();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let permit = captured.clone();
        let input = assembly::SubmissionState::new::<2>(groups::InputBudget::default()).unwrap();
        let worker_input = input.clone();
        let owner = local::Owner::<2, 24, 2, 8, Capturing, TestCommand>::create_new(
            &root.0,
            options.storage.local,
            move |stores, catalog, wake, commands| {
                let local = assembly::factory::configure(
                    stores,
                    catalog,
                    wake.clone(),
                    options.storage,
                    worker_input,
                )?;
                Ok(Capturing {
                    inner: Driver::new(
                        local,
                        context,
                        commands,
                        TestCommand::Group,
                        options.service,
                        wake,
                    )?,
                    permit,
                    hold_start: true,
                    startup_query: None,
                })
            },
        )
        .unwrap();
        let identity_client = owner.identities();
        let identity = configured(&identity_client, 47).await;
        assert!(matches!(
            transition(&identity_client, &identity, core::TransitionTarget::Enable).await,
            core::OperationResult::Enabled { .. }
        ));
        let (entered, held) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let release = StartupRelease(Some(release));
        owner
            .commands()
            .submit(TestCommand::HoldLane(entered, wait))
            .unwrap();
        let dispatch = within(held).await.unwrap();
        let submission = assembly::Submission::new(owner.commands(), input);
        let permit = captured.lock().unwrap().unwrap();
        let private = assembly::client::runner_client(
            submission.clone(),
            identity.reference(),
            permit,
            TestCommand::Group,
            dispatch.clone(),
        )
        .unwrap();
        let public = assembly::network_client(
            submission.clone(),
            identity.reference(),
            permit,
            TestCommand::Group,
        )
        .unwrap();
        let ordinary = assembly::client(submission, identity.reference(), TestCommand::Group);
        let request = || groups::Request {
            id: groups::create_request_id().unwrap(),
            original_scope: None,
            intent: groups::Intent::Create {
                title: "never executed".into(),
                profile: groups::GroupProfile::default(),
            },
        };
        let context = groups::ExecutionContext {
            clock: groups::SystemClock.now(),
        };
        // All three are admitted while the same owner lane is still open.
        let queued_private = private.admit_execute(request(), context).unwrap();
        let queued_public = public.admit_execute(request(), context).unwrap();
        let queued_ordinary = ordinary.admit_execute(request(), context).unwrap();
        let completion = owner.close();
        assert!(!dispatch.is_open());
        // Distinguish synchronous Closed from Reentrant or an executed reply.
        assert_eq!(private.identity().await, Err(groups::ApiError::Busy));
        assert_eq!(public.identity().await, Err(groups::ApiError::Unavailable));
        assert_eq!(
            ordinary.identity().await,
            Err(groups::ApiError::Unavailable)
        );
        drop(release);
        assert!(matches!(
            within(queued_private).await,
            Err(groups::ApiError::Busy)
        ));
        assert!(matches!(
            within(queued_public).await,
            Err(groups::ApiError::Unavailable)
        ));
        assert!(matches!(
            within(queued_ordinary).await,
            Err(groups::ApiError::Unavailable)
        ));
        within(completion).await.unwrap();
        assert!(diagnostics.latest().is_none(), "{:?}", diagnostics.latest());
        host.stop().await.unwrap();
    });
}

#[test]
fn announcement_controls_stay_local_and_preserve_identity_settings_on_reopen() {
    use prns_identities::announces::{ManualRequest, Settings};
    executor().block_on(async {
        let root = Root::new();
        let owner = super::super::create_new_detached::<2, 16, 4, 4>(&root.0, options()).unwrap();
        let identities = owner.identities();
        let first = create(&identities, 21).await;
        let second = create(&identities, 22).await;
        let control = |reference| {
            super::super::announcements(
                owner.commands(),
                reference,
                super::super::Command::Announce,
            )
        };
        let first_control = control(first.reference());
        let second_control = control(second.reference());
        assert_eq!(
            first_control.settings().await.unwrap(),
            Settings {
                interval_millis: None
            }
        );
        first_control
            .set_settings(Settings {
                interval_millis: Some(900_000),
            })
            .await
            .unwrap();
        second_control
            .set_settings(Settings {
                interval_millis: None,
            })
            .await
            .unwrap();
        assert_eq!(
            first_control.announce().await.unwrap(),
            ManualRequest::Disabled
        );
        let mut foreign = first.reference();
        foreign.public_hash = core::PublicIdentityHash([0; 16]);
        assert!(matches!(
            control(foreign).settings().await,
            Err(core::ApiError::Authority { .. })
        ));
        owner.close().await.unwrap();
        assert!(first_control.settings().await.is_err());
        drop(owner);
        let owner =
            super::super::open_existing_detached::<2, 16, 4, 4>(&root.0, options()).unwrap();
        let control = |reference| {
            super::super::announcements(
                owner.commands(),
                reference,
                super::super::Command::Announce,
            )
        };
        assert_eq!(
            control(first.reference()).settings().await.unwrap(),
            Settings {
                interval_millis: Some(900_000)
            }
        );
        assert_eq!(
            control(second.reference()).settings().await.unwrap(),
            Settings {
                interval_millis: None
            }
        );
        assert_eq!(
            control(first.reference()).announce().await.unwrap(),
            ManualRequest::Disabled
        );
        owner.close().await.unwrap();
    });
}

#[cfg(feature = "contact-card")]
#[test]
fn contact_publication_preserves_manual_work_and_legacy_name_messaging() {
    executor().block_on(async {
        let root = Root::new();
        let (host, context) = host().await;
        let options = options();
        let input = assembly::SubmissionState::new::<2>(groups::InputBudget::default()).unwrap();
        let worker_input = input.clone();
        let owner =
            local::Owner::<2, 24, 2, 8, Driver<2, 2, TestCommand>, TestCommand>::create_new(
                &root.0,
                options.storage.local,
                move |stores, catalog, wake, commands| {
                    let local = assembly::factory::configure(
                        stores,
                        catalog,
                        wake.clone(),
                        options.storage,
                        worker_input,
                    )?;
                    Driver::new(
                        local,
                        context,
                        commands,
                        TestCommand::Group,
                        options.service,
                        wake,
                    )
                },
            )
            .unwrap();
        let owner = assembly::Owner::from_local(owner, input);
        let client = owner.identities();
        let identity = configured(&client, 51).await;
        transition(&client, &identity, core::TransitionTarget::Enable).await;
        // Group owner identity read goes through the running carrier startup lane.
        within(async {
            loop {
                if assembly::client(owner.submission(), identity.reference(), TestCommand::Group)
                    .synchronization_progress(prns_groups_core::GroupId([0; 32]))
                    .running
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        let (reply, result) = oneshot::channel();
        owner
            .commands()
            .submit(TestCommand::ContactInvariants(reply))
            .unwrap();
        within(result).await.unwrap();
        let state = || async {
            let (reply, result) = oneshot::channel();
            owner
                .commands()
                .submit(TestCommand::ContactState(reply))
                .unwrap();
            within(result).await.unwrap()
        };
        let (scope, old_name, _) = state().await;
        let release = Arc::new(tokio::sync::Notify::new());
        let entered = Arc::new(AtomicUsize::new(0));
        let mut holds = Vec::new();
        for _ in 0..options.service.carrier.operations {
            let scope = scope.clone();
            let release = release.clone();
            let entered = entered.clone();
            holds.push(tokio::spawn(async move {
                scope
                    .protocol_operation(move |_| async move {
                        let notified = release.notified();
                        tokio::pin!(notified);
                        notified.as_mut().enable();
                        entered.fetch_add(1, Ordering::SeqCst);
                        notified.await;
                    })
                    .await
                    .unwrap();
            }));
        }
        within(async {
            while entered.load(Ordering::SeqCst) != options.service.carrier.operations {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        let metadata = identity.snapshot().await.unwrap().identity.metadata_guard();
        let renamed = identity
            .update_metadata(
                metadata,
                core::Metadata::new("private", Some("Renamed under load")).unwrap(),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        let newer = identity
            .update_metadata(
                renamed,
                core::Metadata::new("private", Some("Newer pending name")).unwrap(),
            )
            .await
            .unwrap();
        identity
            .update_metadata(
                newer,
                core::Metadata::new("private", Some("Renamed under load")).unwrap(),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(110)).await;
        let (_, installed, desired) = state().await;
        assert_eq!(installed, old_name);
        assert_eq!(desired.as_deref(), Some("Renamed under load"));
        assert_eq!(
            identity.snapshot().await.unwrap().runtime,
            core::RuntimeStatus::Running
        );
        release.notify_waiters();
        for hold in holds {
            within(hold).await.unwrap();
        }
        within(async {
            while state().await.1.as_deref() != Some("Renamed under load") {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        within(owner.close()).await.unwrap();
        host.stop().await.unwrap();
    });
}
