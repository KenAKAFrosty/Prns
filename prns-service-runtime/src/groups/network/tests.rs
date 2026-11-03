#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use prns_identities::{api, creation::CreateRequestId, owned};
use prns_identities_core as identity;
use prns_identities_runtime as runtime;
use std::{
    num::NonZeroUsize,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Captured {
    dispatch: Mutex<Option<DispatchLease>>,
    key: Mutex<Option<KeyLease>>,
}
struct Capture(Arc<Captured>);
impl owned::Driver<1> for Capture {
    fn bind_dispatch(&mut self, _: runtime::OwnerToken, lease: DispatchLease) {
        *self.0.dispatch.lock().unwrap() = Some(lease);
    }
    fn bind_key(&mut self, _: runtime::OwnerToken, key: KeyLease) {
        *self.0.key.lock().unwrap() = Some(key);
    }
    fn activation_plan(
        &self,
        record: &identity::IdentityRecord<1>,
    ) -> Result<runtime::ActivationPlan<1>, runtime::Fault> {
        <owned::IdentityOnly as owned::Driver<1>>::activation_plan(&owned::IdentityOnly, record)
    }
    fn dependencies(
        &mut self,
        identity: identity::IdentityReference,
        services: identity::ServiceSet<1>,
    ) -> Result<identity::DependencyReport<1>, runtime::Fault> {
        <owned::IdentityOnly as owned::Driver<1>>::dependencies(
            &mut owned::IdentityOnly,
            identity,
            services,
        )
    }
    fn classify_failure(&self, _: &runtime::Fault) -> identity::ServiceFailure {
        identity::ServiceFailure::StorageUnavailable
    }
}
impl runtime::configuration::ServiceInitializer for Capture {
    type Error = runtime::Fault;
    fn initialize_service(
        &mut self,
        _: identity::EffectTicket,
        _: identity::IdentityGuard,
        _: identity::KeyReference,
        _: identity::ServiceId,
    ) -> Result<identity::InitializedService, Self::Error> {
        Err(runtime::Fault::UnsupportedService)
    }
}
impl runtime::ScopedDriver<1> for Capture {
    type Error = runtime::Fault;
    fn preflight(
        &mut self,
        scope: runtime::Scope,
        plan: &runtime::ActivationPlan<1>,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::Resources>>, Self::Error> {
        <owned::IdentityOnly as runtime::ScopedDriver<1>>::preflight(
            &mut owned::IdentityOnly,
            scope,
            plan,
        )
    }
    fn prepare_service(
        &mut self,
        scope: runtime::Scope,
        key: identity::KeyReference,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::PreparedService>>, Self::Error> {
        <owned::IdentityOnly as runtime::ScopedDriver<1>>::prepare_service(
            &mut owned::IdentityOnly,
            scope,
            key,
            plan,
        )
    }
    fn activate_service(
        &mut self,
        scope: runtime::Scope,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<()>>, Self::Error> {
        <owned::IdentityOnly as runtime::ScopedDriver<1>>::activate_service(
            &mut owned::IdentityOnly,
            scope,
            plan,
        )
    }
    fn fence_previous_workers(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<bool>>, Self::Error> {
        <owned::IdentityOnly as runtime::ScopedDriver<1>>::fence_previous_workers(
            &mut owned::IdentityOnly,
            scope,
        )
    }
    fn quiesce_service(
        &mut self,
        scope: runtime::Scope,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::QuiescedService>>, Self::Error> {
        <owned::IdentityOnly as runtime::ScopedDriver<1>>::quiesce_service(
            &mut owned::IdentityOnly,
            scope,
            plan,
        )
    }
    fn release_identity(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::ReleasedIdentity<1>>>, Self::Error>
    {
        <owned::IdentityOnly as runtime::ScopedDriver<1>>::release_identity(
            &mut owned::IdentityOnly,
            scope,
        )
    }
}
static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "prns-service-router-{}-{}",
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
fn owner_options() -> owned::Options {
    owned::Options {
        queue_capacity: NonZeroUsize::new(4).unwrap(),
        budget: runtime::Budget {
            capacity: runtime::Resources {
                held_keys: 1,
                ..runtime::Resources::ZERO
            },
            infrastructure_reserve: runtime::Resources::ZERO,
        },
    }
}
fn options() -> Options {
    Options {
        operations: 2,
        outgoing_links: 1,
        sync: transport::Limits {
            max_frame_bytes: 134,
            max_page_items: 1,
            max_peers: 2,
            inbound_jobs: 1,
            exchange_timeout: Duration::from_secs(3),
        },
        enrollment: transport::enrollment::Limits {
            max_frame_bytes: 134,
            inbound_jobs: 1,
            exchange_timeout: Duration::from_secs(3),
        },
    }
}
fn request(data: &[u8], destination: DestinationHash) -> PrnsEvent<'_> {
    PrnsEvent::Message(prns_runtime::runtime::Message::Request {
        destination: prns_runtime::wire::DestinationHash::new(destination.into_bytes()),
        link_id: prns_runtime::routing::links::LinkId::new([2; 16]),
        request_id: prns_runtime::routing::links::request::RequestId([3; 16]),
        requester: Some(prns_runtime::identity::IdentityHash::new([4; 16])),
        path_hash: prns_runtime::routing::request_handlers::RequestPathHash::of(
            transport::REQUEST_PATH,
        ),
        requested_at: prns_runtime::engine::InstantMillis(0),
        rtt: prns_runtime::units::RttMillis::new(7),
        data,
    })
}

#[test]
fn router_install_is_explicit_bounded_and_does_not_replace_a_callback() {
    assert!(matches!(Router::new(0), Err(Error::InvalidCapacity)));
    assert!(matches!(
        Router::new(MAX_ROUTES + 1),
        Err(Error::InvalidCapacity)
    ));
    let router = Router::new(1).unwrap();
    let mut occupied = HostEmbedding {
        on_event: Some(Box::new(|_| {})),
        ..HostEmbedding::default()
    };
    assert!(matches!(
        router.install(&mut occupied),
        Err(Error::CallbackOccupied)
    ));
    assert_eq!(occupied.application_events, ApplicationEventDispatch::Queue);
    assert!(occupied.on_event.is_some());
    let mut embedding = HostEmbedding::default();
    router.install(&mut embedding).unwrap();
    assert_eq!(
        embedding.application_events,
        ApplicationEventDispatch::EmbeddingCallback
    );
    assert!(matches!(
        router.install(&mut embedding),
        Err(Error::CallbackOccupied)
    ));
}

#[test]
fn bounded_ingress_fences_immediately_and_cleanup_retains_the_actual_key_owner() {
    type Owner = owned::Owner<1, 8, 1, 4, Capture>;
    let root = Root::new();
    let captured = Arc::new(Captured::default());
    let owner = Owner::create_new(&root.0, owner_options(), {
        let captured = captured.clone();
        move |_| Ok(Capture(captured))
    })
    .unwrap();
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let handle = executor.handle().clone();
    executor.block_on(async {
        let client = owner.client();
        let scope = client.creation_scope().await.unwrap();
        client
            .create_or_recover(
                CreateRequestId {
                    scope,
                    nonce: [1; 32],
                },
                api::CreateRequest {
                    source: identity::KeySource::Generate,
                    metadata: identity::Metadata::new("router", None).unwrap(),
                    services: vec![],
                },
                None,
            )
            .await
            .unwrap();
        let record = client.identities().await.unwrap().remove(0);
        let selected = client.identity(record.reference()).unwrap();
        let prepared = selected
            .prepare(identity::TransitionTarget::Enable)
            .await
            .unwrap();
        let token = client
            .reserve(prepared.clone(), identity::CommandDigest([2; 32]))
            .await
            .unwrap();
        selected.begin(token, prepared.clone()).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(
            client
                .operation(token, api::Command::Transition(prepared.clone()))
                .await
                .unwrap(),
            identity::OperationState::Completed(_)
        ) {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let router = Router::new(1).unwrap();
        let mut embedding = HostEmbedding::default();
        router.install(&mut embedding).unwrap();
        let config = || prns_host::HostConfig {
            identity: prns_host::IdentityConfig::GenerateEphemeral,
            persistence: prns_host::PersistenceConfig::Ephemeral,
            role: prns_host::HostRole::Endpoint,
            destinations: vec![],
            required_capabilities: vec![],
            limits: prns_host::PrnsLimits::balanced(),
        };
        let queue_host = prns_host_runtime::owner::OwnedSession::open(config())
            .await
            .unwrap();
        assert!(matches!(
            Context::attach(queue_host.client(), handle.clone(), router.clone()),
            Err(Error::ApplicationEventsUnavailable)
        ));
        queue_host.stop().await.unwrap();
        let host = prns_host_runtime::owner::OwnedSession::open_with_embedding(config(), embedding)
            .await
            .unwrap();
        assert!(host
            .client()
            .events()
            .claim_stream(prns_host::ConsumerLane::ApplicationEvents)
            .is_err());
        let context = Context::attach(host.client(), handle, router.clone()).unwrap();
        let dispatch = captured.dispatch.lock().unwrap().take().unwrap();
        let key = captured.key.lock().unwrap().take().unwrap();
        let mut prepared = context
            .prepare(dispatch.clone(), key.clone(), options())
            .unwrap();
        assert!(matches!(
            context.prepare(dispatch.clone(), key, options()),
            Err(Error::DuplicateIdentity)
        ));
        let destination = prepared.registration.sync_destination();
        router.on_event_ref(&request(&[0xc4, 3, 9, 8, 7], destination));
        router.on_event_ref(&request(&[0xc4, 3, 6, 5, 4], destination));
        assert_eq!(prepared.sync_jobs.try_recv().unwrap().data(), [9, 8, 7]);
        assert!(prepared.sync_jobs.try_recv().is_err()); // exact bounded queue
        let link = prns_host::LinkId::new([8; 16]);
        prepared
            .registration
            .scope()
            .reserve_link()
            .unwrap()
            .track(link);
        let completion = owner.close();
        assert!(!dispatch.is_open());
        assert!(completion.status().is_none());
        router.on_event_ref(&request(&[0xc4, 1, 9], destination));
        assert!(prepared.sync_jobs.try_recv().is_err());
        assert!(Owner::open_existing(&root.0, owner_options(), {
            let captured = captured.clone();
            move |_| Ok(Capture(captured))
        })
        .is_err());
        prepared.registration.scope().fence();
        router.on_event_ref(&PrnsEvent::Diagnostic(
            prns_runtime::runtime::Diagnostic::LinkClosed {
                link_id: prns_runtime::routing::links::LinkId::new(link.into_bytes()),
                reason: prns_runtime::engine::LinkClosedReason::LocallyClosed,
            },
        ));
        // This fixture registered no host endpoints; only its actual Link scope
        // and callback route need release. Production also finishes both endpoints.
        prepared.registration.finish().unwrap();
        prepared.registration.finish().unwrap();
        assert!(completion.status().is_none()); // caller still owns the route/key
        drop(prepared);
        completion.await.unwrap();
        drop(context);
        host.stop().await.unwrap();
    });
}
