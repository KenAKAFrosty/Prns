#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::panic)]
use prns_host_runtime::HostEmbedding;
use prns_identities::{api, creation::CreateRequestId};
use prns_identities_core as identity;
use prns_identities_runtime as runtime;
use prns_service_runtime::groups::standalone::{
    CarrierOptions, Error, NetworkPhase, Options, Owner, Phase, StartupRecovery,
};
use std::{
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};
const QUERY_PAGE: prns_groups::PageLimits = prns_groups::PageLimits {
    max_rows: 64,
    max_bytes: 64 * 1024,
};

struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
fn wait<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return value;
        }
        assert!(Instant::now() < deadline, "native owner stalled");
        std::thread::park_timeout(Duration::from_millis(5));
    }
}
static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "prns-standalone-{}-{}",
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
fn host() -> prns_host::HostConfig {
    prns_host::HostConfig {
        identity: prns_host::IdentityConfig::GenerateEphemeral,
        persistence: prns_host::PersistenceConfig::Ephemeral,
        role: prns_host::HostRole::Endpoint,
        destinations: vec![],
        required_capabilities: vec![],
        limits: prns_host::PrnsLimits::balanced(),
    }
}
fn options() -> Options {
    Options {
        startup_recovery: StartupRecovery::Dormant,
        queue_capacity: 32,
        identity_budget: runtime::Budget {
            capacity: runtime::Resources {
                held_keys: 4,
                destinations: 8,
                request_handlers: 8,
                links: 32,
                queue_slots: 256,
            },
            infrastructure_reserve: runtime::Resources::ZERO,
        },
        cache_bytes_per_identity: 1 << 20,
        total_cache_bytes: 4 << 20,
        group_client: prns_groups::ClientLimits::default(),
        validation_cache_entries: 4,
        retention: prns_groups::retained::Limits {
            max_operations: 32,
            max_bytes: 2 << 20,
            ..Default::default()
        },
        catalog: prns_groups::enrollment::CatalogLimits::default(),
        #[cfg(feature = "archive")]
        archive: Some(prns_groups::archive::Limits {
            max_records: 256,
            max_bytes: 8 << 20,
        }),
        carrier: CarrierOptions {
            operations: 16,
            outgoing_links: 8,
            sync_frame_bytes: 131072,
            sync_page_items: 8,
            sync_peers: 4,
            sync_inbound_jobs: 16,
            sync_timeout_millis: 5000,
            enrollment_frame_bytes: prns_groups_reticulum::enrollment::MAX_FRAME_BYTES as u32,
            enrollment_inbound_jobs: 4,
            enrollment_timeout_millis: 5000,
        },
        sync_policy: prns_groups::SyncPolicy::default(),
        announce_interval_millis: None,
    }
}
#[test]
fn actual_owner_needs_no_ambient_runtime_and_finalization_drains_borrowed_clients() {
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    assert_eq!(owner.status().phase, Phase::Ready);
    let host = owner.host().unwrap();
    let catalog = owner.identities().unwrap();
    let result = wait(catalog.create_or_recover(
        CreateRequestId {
            scope: wait(catalog.creation_scope()).unwrap(),
            nonce: [1; 32],
        },
        api::CreateRequest {
            source: identity::KeySource::Generate,
            metadata: identity::Metadata::new("field sensor", None).unwrap(),
            services: vec![],
        },
        None,
    ))
    .unwrap();
    assert!(matches!(
        result.result,
        identity::OperationResult::Created { .. }
    ));
    let record = wait(catalog.identities()).unwrap().remove(0);
    let groups = owner.groups(record.reference()).unwrap();
    assert!(
        wait(groups.conversations(None, QUERY_PAGE)).is_err(),
        "construction did not configure a service"
    );
    let finished = owner.closed();
    drop(owner);
    wait(finished).unwrap();
    assert!(wait(catalog.identities()).is_err());
    assert!(host.identity_hash().is_err());
    let reopened = Owner::open_existing(&root.0, crate::host(), options()).unwrap();
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    assert_eq!(
        wait(reopened.identities().unwrap().identities()).unwrap()[0].reference(),
        record.reference()
    );
    wait(reopened.close()).unwrap();
    assert!(reopened.status().cleanup_complete);
}
#[test]
fn local_data_is_ready_during_blocked_network_start_and_full_close_still_drains() {
    let root = Root::new();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let embedding = HostEmbedding {
        prepare_interfaces: Some(Box::new(move |_| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(vec![])
        })),
        ..HostEmbedding::default()
    };
    let owner = Owner::create_new_with_embedding(&root.0, host(), options(), embedding).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(owner.status().phase, Phase::Ready);
    assert_eq!(owner.network_status().phase, NetworkPhase::Starting);
    let catalog = owner.identities().unwrap();
    assert!(wait(catalog.identities()).unwrap().is_empty());
    let mut cancelled = Box::pin(owner.network_ready());
    assert!(cancelled
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    drop(cancelled);
    let close = owner.close();
    assert!(close.status().is_none());
    assert!(matches!(owner.identities(), Err(Error::Closing)));
    assert_eq!(wait(owner.ready()), Err(Error::Closing));
    release_tx.send(()).unwrap();
    wait(close).unwrap();
    assert!(owner.status().cleanup_complete);
    assert!(
        root.0.join("identities").exists(),
        "network cleanup must retain the independently opened local catalog"
    );
}
#[test]
fn invalid_options_precede_effects_and_failed_reopen_is_inspectable() {
    let root = Root::new();
    let mut bad = options();
    bad.queue_capacity = 0;
    assert!(matches!(
        Owner::create_new(&root.0, host(), bad),
        Err(Error::InvalidOptions)
    ));
    assert!(!root.0.exists());
    let mut bad = options();
    bad.validation_cache_entries = 0;
    assert!(matches!(
        Owner::create_new(&root.0, host(), bad),
        Err(Error::InvalidOptions)
    ));
    assert!(!root.0.exists());
    let mut bad = options();
    bad.group_client.max_pending_job_bytes = 0;
    assert!(matches!(
        Owner::create_new(&root.0, host(), bad),
        Err(Error::InvalidOptions)
    ));
    assert!(!root.0.exists());
    let owner = Owner::open_existing(&root.0, host(), options()).unwrap();
    let failure = wait(owner.ready()).unwrap_err();
    assert!(owner.identities().is_err());
    assert_eq!(owner.status().failure, Some(failure.clone()));
    assert_eq!(wait(owner.network_ready()), Err(failure));
    assert!(wait(owner.close()).is_err());
    assert!(owner.status().cleanup_complete);
    assert!(!root.0.exists());
}

#[test]
fn failed_group_reopen_retains_its_domain_cause_without_reinitializing_state() {
    use prns_service_runtime::groups::standalone::AttachmentFailure;
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let identity = create_identity(&owner, 94);
    let catalog = owner.identities().unwrap();
    let prepared = wait(identity.prepare(identity::TransitionTarget::AddService(
        prns_service_runtime::groups::SERVICE,
    )))
    .unwrap();
    let token = wait(catalog.reserve(prepared.clone(), identity::CommandDigest([94; 32]))).unwrap();
    wait(identity.begin(token, prepared)).unwrap();
    await_identity(&identity, |snapshot| snapshot.identity.pending.is_none());
    let path =
        prns_identities::scoped_store::path_under(identity.reference(), &root.0.join("domains"))
            .unwrap();
    wait(owner.close()).unwrap();
    drop(owner);

    // Remove one required schema while retaining the physical identity binding.
    // A strict reopen must diagnose this partial state, never initialize it.
    let table: redb::TableDefinition<u8, &[u8]> =
        redb::TableDefinition::new("prns_group_enrollment_v1");
    let db = redb::Database::open(&path).unwrap();
    let tx = db.begin_write().unwrap();
    assert!(tx.delete_table(table).unwrap());
    tx.commit().unwrap();
    drop(db);

    let reopened = Owner::open_existing_offline(&root.0, options()).unwrap();
    let failure = wait(reopened.ready()).unwrap_err();
    assert!(matches!(
        &failure,
        Error::Attachment {
            reason: AttachmentFailure::Group {
                error: prns_groups::ApiError::NotProvisioned
            },
            ..
        }
    ));
    assert_eq!(reopened.status().failure, Some(failure.clone()));
    assert_eq!(reopened.network_status().phase, NetworkPhase::Offline);
    assert!(wait(reopened.close()).is_err());
    assert!(reopened.status().cleanup_complete);
    assert_eq!(reopened.status().failure, Some(failure));
    drop(reopened);
    let db = redb::Database::open(&path).unwrap();
    use redb::ReadableDatabase;
    assert!(db.begin_read().unwrap().open_table(table).is_err());
    assert!(root.0.join("identities").exists());
}

#[test]
fn platform_callback_is_combined_before_start_and_retained_until_host_stop() {
    let root = Root::new();
    let retained = Arc::new(());
    let callback = retained.clone();
    let embedding = HostEmbedding {
        application_events: prns_host_runtime::ApplicationEventDispatch::EmbeddingCallback,
        on_event: Some(Box::new(move |_| {
            let _ = Arc::strong_count(&callback);
        })),
        ..HostEmbedding::default()
    };
    let owner = Owner::create_new_with_embedding(&root.0, host(), options(), embedding).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    assert_eq!(Arc::strong_count(&retained), 2);
    wait(owner.close()).unwrap();
    assert_eq!(
        Arc::strong_count(&retained),
        1,
        "platform callback retained after joined host shutdown"
    );
}

#[test]
fn retained_identity_diagnostic_is_historical_and_does_not_replace_cleanup_result() {
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let catalog = owner.identities().unwrap();
    wait(catalog.create_or_recover(
        CreateRequestId {
            scope: wait(catalog.creation_scope()).unwrap(),
            nonce: [2; 32],
        },
        api::CreateRequest {
            source: identity::KeySource::Generate,
            metadata: identity::Metadata::new("sensor", None).unwrap(),
            services: vec![],
        },
        None,
    ))
    .unwrap();
    let record = wait(catalog.identities()).unwrap().remove(0);
    let identity = catalog.identity(record.reference()).unwrap();
    let rejected = wait(identity.update_metadata(
        identity::MetadataGuard {
            id: record.id,
            public_hash: record.public_hash,
            metadata_revision: record.metadata_revision + 1,
        },
        identity::Metadata::new("unchanged", None).unwrap(),
    ))
    .unwrap_err();
    let diagnostic = owner.status().latest_identity_diagnostic.unwrap();
    assert_eq!(diagnostic.identity, Some(record.reference()));
    assert_eq!(diagnostic.error, rejected);
    let observer = owner.diagnostics().unwrap();
    assert_eq!(observer.latest().unwrap().identity, diagnostic.identity);
    assert_eq!(
        owner.status().phase,
        Phase::Ready,
        "a historical diagnostic must not rewrite lifecycle state"
    );
    wait(owner.close()).unwrap();
    let closed = owner.status();
    assert_eq!(closed.phase, Phase::Closed);
    assert!(closed.cleanup_complete);
    assert!(closed.failure.is_none());
    assert_eq!(closed.latest_identity_diagnostic, Some(diagnostic));
    assert!(observer.latest().is_some());
}

struct PauseReply {
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    once: std::sync::atomic::AtomicBool,
}
impl Wake for PauseReply {
    fn wake(self: Arc<Self>) {
        if !self.once.swap(true, Ordering::SeqCst) {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
    }
}
struct ReleaseReply(Option<std::sync::mpsc::SyncSender<()>>);
impl Drop for ReleaseReply {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.try_send(());
        }
    }
}
#[test]
fn close_fences_borrowed_host_before_local_drain_and_actual_host_stop() {
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let host = owner.host().unwrap();
    let catalog = owner.identities().unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let release = ReleaseReply(Some(release_tx));
    let waker = Waker::from(Arc::new(PauseReply {
        entered: entered_tx,
        release: std::sync::Mutex::new(release_rx),
        once: std::sync::atomic::AtomicBool::new(false),
    }));
    // Pause the actual identity lane in an accepted reply callback. This keeps
    // local drainage pending while the composed host is deliberately available.
    // If a fast reply wins registration, retry this harmless bounded read.
    let mut pending = None;
    for _ in 0..32 {
        let mut reply = Box::pin(catalog.creation_scope());
        if reply
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
        {
            pending = Some(reply);
            break;
        }
    }
    let pending = pending.expect("registered a reply observer");
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let close = owner.close();
    assert!(close.status().is_none());
    assert_eq!(owner.status().phase, Phase::Closing);
    assert_eq!(
        host.events().lifecycle().state,
        prns_host::LifecycleState::Running
    );
    assert!(wait(host.snapshot()).unwrap().interfaces.is_empty());
    assert_eq!(
        wait(host.execute(prns_host::HostCommand::AttachTcpServer {
            bind: "127.0.0.1:0".into(),
            bitrate: prns_host::Bitrate::Auto,
        })),
        Err(prns_host_runtime::owner::SessionError::Stopped)
    );
    assert_eq!(
        wait(host.close_remote_control_pairing()),
        Err(prns_host_runtime::owner::SessionError::Stopped)
    );
    assert!(wait(host.snapshot()).unwrap().interfaces.is_empty());
    // Losing the accepted identity waiter does not cancel the lane's completion.
    drop(pending);
    drop(release);
    wait(close).unwrap();
    assert!(owner.status().cleanup_complete);
    assert!(host.events().lifecycle().state.is_terminal());
}

fn create_identity(owner: &Owner, nonce: u8) -> api::IdentityClient {
    let catalog = owner.identities().unwrap();
    let outcome = wait(catalog.create_or_recover(
        CreateRequestId {
            scope: wait(catalog.creation_scope()).unwrap(),
            nonce: [nonce; 32],
        },
        api::CreateRequest {
            source: identity::KeySource::Generate,
            metadata: identity::Metadata::new("startup fixture", None).unwrap(),
            services: vec![],
        },
        None,
    ))
    .unwrap();
    let identity::OperationResult::Created { identity } = outcome.result else {
        panic!("expected fresh identity")
    };
    let record = wait(catalog.identities())
        .unwrap()
        .into_iter()
        .find(|record| record.id == identity)
        .unwrap();
    catalog.identity(record.reference()).unwrap()
}

fn await_identity(
    client: &api::IdentityClient,
    predicate: impl Fn(&api::Snapshot) -> bool,
) -> api::Snapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = wait(client.snapshot()).unwrap();
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "identity did not settle: {snapshot:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn enable(owner: &Owner, client: &api::IdentityClient) -> api::Snapshot {
    let prepared = wait(client.prepare(identity::TransitionTarget::Enable)).unwrap();
    let token = wait(
        owner
            .identities()
            .unwrap()
            .reserve(prepared.clone(), identity::CommandDigest([93; 32])),
    )
    .unwrap();
    wait(client.begin(token, prepared)).unwrap();
    await_identity(client, |snapshot| {
        snapshot.runtime == identity::RuntimeStatus::Running && snapshot.identity.pending.is_none()
    })
}

fn recover_options() -> Options {
    Options {
        startup_recovery: StartupRecovery::RecoverCurrent,
        ..options()
    }
}

#[test]
fn selected_startup_recovers_current_work_and_keeps_reservations_inert() {
    assert_eq!(StartupRecovery::default(), StartupRecovery::Dormant);
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let enabled = create_identity(&owner, 31);
    let enabled_before = enable(&owner, &enabled);
    let paused = create_identity(&owner, 32);
    let reserved = create_identity(&owner, 33);
    let pending = create_identity(&owner, 34);
    let prepared = wait(reserved.prepare(identity::TransitionTarget::Enable)).unwrap();
    let reserved_token = wait(
        owner
            .identities()
            .unwrap()
            .reserve(prepared.clone(), identity::CommandDigest([94; 32])),
    )
    .unwrap();
    let enabled_ref = enabled.reference();
    let paused_ref = paused.reference();
    let reserved_ref = reserved.reference();
    let pending_ref = pending.reference();
    wait(owner.close()).unwrap();

    // Stop after the canonical accepted AddService commit, before any runtime
    // effect. No manufactured file format or initializer substitutes for it.
    let (pending_token, pending_command) = {
        let mut disk =
            prns_identities::IdentityStore::<4, 32, 4>::open(root.0.join("identities")).unwrap();
        let mut current = identity::Catalog::<4, 32, 4>::new([0; 16]);
        let mut staged = current.clone();
        let mut coordinator =
            identity::Coordinator::open(&mut disk, &mut current, &mut staged).unwrap();
        let prepared = coordinator
            .prepare(
                pending_ref.id,
                identity::TransitionTarget::AddService(prns_service_runtime::groups::SERVICE),
                identity::DependencyReport::empty(),
            )
            .unwrap();
        let command = identity::Command::Transition(prepared);
        let token = coordinator
            .reserve(&mut disk, command, identity::CommandDigest([95; 32]))
            .unwrap();
        assert_eq!(
            coordinator.begin(&mut disk, token, &command).unwrap(),
            identity::OperationState::Transitioning
        );
        (token, api::Command::from(command))
    };
    let dormant = Owner::open_existing(&root.0, host(), options()).unwrap();
    wait(dormant.ready()).unwrap();
    wait(dormant.network_ready()).unwrap();
    let catalog = dormant.identities().unwrap();
    assert_eq!(
        wait(catalog.identity(enabled_ref).unwrap().snapshot())
            .unwrap()
            .runtime,
        identity::RuntimeStatus::Unavailable
    );
    assert!(wait(catalog.identity(pending_ref).unwrap().snapshot())
        .unwrap()
        .identity
        .pending
        .is_some());
    wait(dormant.close()).unwrap();

    let restored = Owner::open_existing(&root.0, host(), recover_options()).unwrap();
    wait(restored.ready()).unwrap();
    wait(restored.network_ready()).unwrap();
    let catalog = restored.identities().unwrap();
    let enabled_after = await_identity(&catalog.identity(enabled_ref).unwrap(), |snapshot| {
        snapshot.runtime == identity::RuntimeStatus::Running
    });
    assert_eq!(enabled_after.identity, enabled_before.identity);
    let configured = await_identity(&catalog.identity(pending_ref).unwrap(), |snapshot| {
        snapshot.identity.pending.is_none()
    });
    assert_eq!(
        configured.identity.services,
        vec![prns_service_runtime::groups::SERVICE]
    );
    assert_eq!(configured.identity.desired, identity::DesiredState::Paused);
    assert!(matches!(
        wait(catalog.operation(pending_token, pending_command)).unwrap(),
        identity::OperationState::Completed(identity::OperationResult::ServiceAdded { .. })
    ));
    for reference in [paused_ref, reserved_ref] {
        let snapshot = wait(catalog.identity(reference).unwrap().snapshot()).unwrap();
        assert_eq!(snapshot.identity.desired, identity::DesiredState::Paused);
        assert_eq!(snapshot.runtime, identity::RuntimeStatus::Unavailable);
        assert!(snapshot.identity.pending.is_none());
    }
    assert_eq!(
        wait(catalog.operation(reserved_token, api::Command::Transition(prepared))).unwrap(),
        identity::OperationState::Reserved
    );
    wait(restored.close()).unwrap();
}

fn blocked_embedding() -> (
    HostEmbedding,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::SyncSender<()>,
) {
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    (
        HostEmbedding {
            prepare_interfaces: Some(Box::new(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                Ok(vec![])
            })),
            ..HostEmbedding::default()
        },
        entered_rx,
        release_tx,
    )
}

#[test]
fn recovery_is_owner_retained_and_stop_prevents_late_readiness() {
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let identity = create_identity(&owner, 41);
    let before = enable(&owner, &identity);
    let reference = identity.reference();
    wait(owner.close()).unwrap();

    let (embedding, entered, release) = blocked_embedding();
    let reopened =
        Owner::open_existing_with_embedding(&root.0, host(), recover_options(), embedding).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut lost = Box::pin(reopened.network_ready());
    assert!(lost
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    drop(lost);
    release.send(()).unwrap();
    // No readiness observer is required to keep construction/recovery alive.
    let deadline = Instant::now() + Duration::from_secs(10);
    while reopened.status().phase != Phase::Ready {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let restored = reopened.identities().unwrap().identity(reference).unwrap();
    let after = await_identity(&restored, |snapshot| {
        snapshot.runtime == identity::RuntimeStatus::Running
    });
    assert_eq!(before.identity, after.identity);
    wait(reopened.close()).unwrap();

    let (embedding, entered, release) = blocked_embedding();
    let cancelled =
        Owner::open_existing_with_embedding(&root.0, host(), recover_options(), embedding).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let close = cancelled.close();
    assert_eq!(wait(cancelled.ready()), Err(Error::Closing));
    release.send(()).unwrap();
    wait(close).unwrap();
    assert_eq!(cancelled.status().phase, Phase::Closed);
    assert!(cancelled.status().cleanup_complete);
    assert!(matches!(cancelled.identities(), Err(Error::Closing)));

    let final_owner = Owner::open_existing(&root.0, host(), recover_options()).unwrap();
    wait(final_owner.ready()).unwrap();
    wait(final_owner.network_ready()).unwrap();
    let restored = final_owner
        .identities()
        .unwrap()
        .identity(reference)
        .unwrap();
    let after = await_identity(&restored, |snapshot| {
        snapshot.runtime == identity::RuntimeStatus::Running
    });
    assert_eq!(before.identity, after.identity);
    wait(final_owner.close()).unwrap();
}

#[test]
fn recovery_readiness_does_not_hide_identity_activation_failure() {
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let identity = create_identity(&owner, 51);
    let before = enable(&owner, &identity);
    let reference = identity.reference();
    wait(owner.close()).unwrap();
    let mut insufficient = recover_options();
    insufficient.identity_budget.capacity.held_keys = 0;
    let reopened = Owner::open_existing(&root.0, host(), insufficient).unwrap();
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    let identity = reopened.identities().unwrap().identity(reference).unwrap();
    let failed = await_identity(&identity, |snapshot| snapshot.last_error.is_some());
    assert_ne!(failed.runtime, identity::RuntimeStatus::Running);
    assert_eq!(failed.identity, before.identity);
    let status = reopened.status();
    assert_eq!(status.phase, Phase::Ready);
    let diagnostic = status.latest_identity_diagnostic.unwrap();
    assert_eq!(diagnostic.identity, Some(reference));
    assert_eq!(Some(diagnostic.error), failed.last_error);
    wait(reopened.close()).unwrap();
    assert!(reopened.status().cleanup_complete);
}

#[cfg(feature = "connections")]
#[test]
fn saved_connections_restore_across_composed_generations_without_a_foreign_runtime() {
    use prns_host_runtime::connections::{self, BindingPhase, ProfileOwner};
    use std::{net::TcpListener, num::NonZeroUsize};

    let root = Root::new();
    prns_storage::files::create_private_directory(&root.0).unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let profile = connections::Profile {
        id: connections::ProfileId::new([0x42; 16]).unwrap(),
        label: "Local test peer".into(),
        enabled: true,
        config: connections::tcp_client_config(
            "127.0.0.1",
            listener.local_addr().unwrap().port(),
            prns_host::Bitrate::Auto,
        )
        .unwrap(),
        routing: None,
    };
    let profiles = Arc::new(
        ProfileOwner::create_new(
            root.0.join("connections.redb"),
            vec![profile.clone()],
            connections::Options {
                limits: connections::Limits {
                    profiles: 4,
                    encoded_bytes: 16 * 1024,
                },
                queue_capacity: NonZeroUsize::new(16).unwrap(),
                cache_bytes: 1024 * 1024,
            },
        )
        .unwrap(),
    );
    let saved = profiles.client();
    let identities = root.0.join("identities");
    let owner = Owner::create_new_with_connections(
        &identities,
        host(),
        options(),
        HostEmbedding::default(),
        profiles.clone(),
    )
    .unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let first = owner.connection_binding().unwrap();
    let first_generation = first.status().generation;
    let assert_connected = |owner: &Owner| {
        let host = owner.host().unwrap();
        let binding = owner.connection_binding().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let live = wait(binding.snapshot()).unwrap();
            let connected = wait(host.snapshot())
                .unwrap()
                .interfaces
                .iter()
                .any(|interface| interface.health == prns_host::InterfaceHealth::Connected);
            if live.applied_revision == Some(live.saved.revision) && connected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "saved TCP profile did not connect"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    assert_connected(&owner);
    let close = owner.close();
    assert_ne!(first.status().phase, BindingPhase::Active);
    let before = wait(saved.snapshot()).unwrap();
    assert!(matches!(
        wait(first.retry(before.guard, profile.id)),
        Err(connections::Error::Closed)
    ));
    wait(close).unwrap();
    assert!(owner.status().cleanup_complete);
    assert_eq!(first.status().phase, BindingPhase::Closed);

    // Settings remain durably editable while the host is stopped; the next
    // generation consumes exactly that intent, with no preset reinsertion.
    let mut edited = before.profiles;
    edited[0].label = "Renamed while stopped".into();
    let committed = wait(saved.replace(before.guard, edited)).unwrap();
    let reopened = Owner::open_existing_with_connections(
        &identities,
        host(),
        options(),
        HostEmbedding::default(),
        profiles.clone(),
    )
    .unwrap();
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    assert_ne!(
        reopened.connection_binding().unwrap().status().generation,
        first_generation
    );
    assert_connected(&reopened);
    assert_eq!(wait(saved.snapshot()).unwrap(), committed);
    wait(reopened.close()).unwrap();
    assert!(reopened.status().cleanup_complete);
    wait(profiles.close()).unwrap();
}

#[cfg(feature = "connections")]
#[test]
fn connection_cleanup_failure_stays_owned_until_an_explicit_close_retry() {
    use prns_host_runtime::{connections, HostPreparedAttachment};
    use std::num::NonZeroUsize;
    struct HeldFleet;
    impl personal_rns::interfaces::ReportsStatus for HeldFleet {}
    impl personal_rns::runtime::InterfaceSupervisor for HeldFleet {
        const KIND: personal_rns::interfaces::InterfaceKind =
            personal_rns::interfaces::InterfaceKind::AutoWifi;
        fn channel_tag(&self) -> &[u8] {
            b"composed-retirement-test"
        }
        fn policy(&self) -> personal_rns::interfaces::EffectiveInterfacePolicy {
            personal_rns::interfaces::tcp::policy_for_bitrate(
                personal_rns::interfaces::tcp::TCP_BITRATE_ESTIMATE,
            )
        }
        async fn run(self, _fleet: personal_rns::Fleet) {
            std::future::pending::<()>().await;
        }
    }
    let root = Root::new();
    prns_storage::files::create_private_directory(&root.0).unwrap();
    let profiles = Arc::new(
        connections::ProfileOwner::create_new(
            root.0.join("connections.redb"),
            vec![connections::Profile {
                id: connections::ProfileId::new([0x43; 16]).unwrap(),
                label: "Held LAN".into(),
                enabled: true,
                config: prns_host::InterfaceConfig::AutoLan {
                    group_id: None,
                    discovery_scope: None,
                    discovery_port: None,
                    data_port: None,
                    devices: vec![],
                    ignored_devices: vec![],
                    multicast_address_type: None,
                },
                routing: None,
            }],
            connections::Options {
                limits: connections::Limits {
                    profiles: 4,
                    encoded_bytes: 16 * 1024,
                },
                queue_capacity: NonZeroUsize::new(16).unwrap(),
                cache_bytes: 1024 * 1024,
            },
        )
        .unwrap(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let cleanup_calls = calls.clone();
    let (release_retry, retry_gate) = tokio::sync::oneshot::channel();
    let retry_gate = Arc::new(std::sync::Mutex::new(Some(retry_gate)));
    let cleanup: prns_host_runtime::HostAttachmentCleanup = Arc::new(move || {
        let calls = cleanup_calls.clone();
        let retry_gate = retry_gate.clone();
        Box::pin(async move {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => Err("retirement not yet complete".into()),
                1 => {
                    let gate = retry_gate.lock().unwrap().take().unwrap();
                    gate.await
                        .map_err(|_| "test retry gate was dropped".to_string())?;
                    Err("explicit retirement retry still incomplete".into())
                }
                _ => Ok(()),
            }
        })
    });
    let embedding = HostEmbedding {
        auto_lan_factory: Some(Arc::new(move |host, _, _| {
            Ok(HostPreparedAttachment::ManagedSupervisor {
                attachment: host.supervise(HeldFleet),
                kind: prns_host::InterfaceKind::AutoLan,
                cleanup: cleanup.clone(),
            })
        })),
        ..HostEmbedding::default()
    };
    let owner = Owner::create_new_with_connections(
        root.0.join("identities"),
        host(),
        options(),
        embedding,
        profiles.clone(),
    )
    .unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let binding = owner.connection_binding().unwrap();
    let host = owner.host().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let live = wait(binding.snapshot()).unwrap();
        if live.applied_revision == Some(live.saved.revision) {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    let first_close = owner.close();
    let until = Instant::now() + Duration::from_secs(5);
    while owner.status().phase != Phase::Unresolved {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!owner.status().cleanup_complete);
    assert!(binding.status().cleanup_failure.is_some());
    assert!(
        wait(host.snapshot()).is_ok(),
        "host remains owned until attachment cleanup"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut first_close = std::pin::pin!(first_close);
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    assert!(first_close
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    let mut retry_close = std::pin::pin!(owner.close());
    assert_eq!(owner.status().phase, Phase::Closing);
    let until = Instant::now() + Duration::from_secs(5);
    while calls.load(Ordering::SeqCst) != 2 {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Another observer during the gated retry must join it, not pre-authorize a
    // third cleanup attempt if this one subsequently fails.
    let mut concurrent_close = std::pin::pin!(owner.close());
    assert_eq!(owner.status().phase, Phase::Closing);
    assert!(retry_close
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    assert!(concurrent_close
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    release_retry.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while owner.status().phase != Phase::Unresolved {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!owner.status().cleanup_complete);
    assert!(retry_close
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    assert!(concurrent_close
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    assert!(
        matches!(wait(owner.close()), Err(prns_storage::CloseError::Failed(error))
        if matches!(error.as_ref(), Error::ConnectionDrainUnresolved { .. }))
    );
    assert!(owner.status().cleanup_complete);
    assert_eq!(owner.status().phase, Phase::Closed);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(binding.status().phase, connections::BindingPhase::Closed);
    assert!(
        binding.status().cleanup_failure.is_some(),
        "historical failure remains visible"
    );
    assert!(wait(profiles.client().snapshot()).is_ok());
    wait(profiles.close()).unwrap();
}

#[test]
fn stopped_network_keeps_same_data_clients_and_durable_enabled_intent() {
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), recover_options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let catalog = owner.identities().unwrap();
    let identity = create_identity(&owner, 61);
    let prepared = wait(identity.prepare(identity::TransitionTarget::AddService(
        prns_service_runtime::groups::SERVICE,
    )))
    .unwrap();
    let token = wait(catalog.reserve(prepared.clone(), identity::CommandDigest([61; 32]))).unwrap();
    wait(identity.begin(token, prepared)).unwrap();
    await_identity(&identity, |snapshot| snapshot.identity.pending.is_none());
    let before = enable(&owner, &identity);
    let groups = owner.groups(identity.reference()).unwrap();
    use prns_groups::Clock;
    let outcome = wait(groups.execute(
        prns_groups::Request {
            id: prns_groups::create_request_id().unwrap(),
            intent: prns_groups::Intent::Create {
                profile: Default::default(),
                title: "Kept offline".into(),
            },
            original_scope: None,
        },
        prns_groups::ExecutionContext {
            clock: prns_groups::SystemClock.now(),
        },
    ))
    .unwrap();
    let prns_groups::Outcome::Created(group) = outcome else {
        panic!("group was not created")
    };

    let original_host = owner.host().unwrap();
    let first_generation = owner.network_status().generation;
    let stopped = owner.stop_networking().unwrap();
    // Losing one observer cannot cancel the retained stop.
    drop(stopped.clone());
    wait(stopped.wait()).unwrap();
    assert_eq!(owner.status().phase, Phase::Ready);
    assert_eq!(owner.network_status().phase, NetworkPhase::Offline);
    assert!(!owner.status().cleanup_complete);
    assert!(owner.host().is_err());
    assert!(original_host.identity_hash().is_err());
    let offline = wait(identity.snapshot()).unwrap();
    assert_eq!(offline.identity, before.identity);
    assert_eq!(offline.runtime, identity::RuntimeStatus::Unavailable);
    assert_eq!(wait(catalog.identities()).unwrap().len(), 1);
    let detail = wait(groups.conversation(group, QUERY_PAGE)).unwrap();
    assert_eq!(detail.title, "Kept offline");
    assert_eq!(
        wait(groups.conversations(None, QUERY_PAGE))
            .unwrap()
            .items
            .len(),
        1
    );
    let competing = Owner::open_existing(&root.0, host(), options()).unwrap();
    assert!(
        wait(competing.ready()).is_err(),
        "suspension released the exclusive data owner"
    );
    let _ = wait(competing.close());
    wait(owner.start_networking(host()).unwrap().wait()).unwrap();
    assert!(owner.network_status().generation > first_generation);
    let restored = await_identity(&identity, |snapshot| {
        snapshot.runtime == identity::RuntimeStatus::Running
    });
    assert_eq!(restored.identity, before.identity);
    assert_eq!(wait(catalog.identities()).unwrap().len(), 1);
    wait(owner.close()).unwrap();
    assert!(wait(catalog.identities()).is_err());
    let reopened = Owner::open_existing(&root.0, host(), recover_options()).unwrap();
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    assert_eq!(
        wait(reopened.identities().unwrap().identities()).unwrap()[0].reference(),
        identity.reference()
    );
    wait(reopened.close()).unwrap();
}

#[test]
fn failed_network_start_keeps_local_storage_and_allows_fresh_preparation_retry() {
    let root = Root::new();
    let embedding = HostEmbedding {
        prepare_interfaces: Some(Box::new(|_| {
            Err(prns_host_runtime::HostStartError::Runtime(
                "offline fixture".into(),
            ))
        })),
        ..HostEmbedding::default()
    };
    let owner =
        Owner::create_new_with_embedding(&root.0, host(), recover_options(), embedding).unwrap();
    wait(owner.ready()).unwrap();
    assert!(matches!(
        wait(owner.network_ready()),
        Err(Error::HostStart { .. })
    ));
    assert_eq!(owner.network_status().phase, NetworkPhase::Failed);
    assert_eq!(owner.status().phase, Phase::Ready);
    let identity = create_identity(&owner, 62);
    let before = wait(identity.snapshot()).unwrap();
    wait(
        owner
            .start_networking_with_embedding(host(), HostEmbedding::default())
            .unwrap()
            .wait(),
    )
    .unwrap();
    assert_eq!(wait(identity.snapshot()).unwrap().identity, before.identity);
    wait(owner.stop_networking().unwrap().wait()).unwrap();
    assert_eq!(
        wait(owner.identities().unwrap().identities())
            .unwrap()
            .len(),
        1
    );
    wait(owner.close()).unwrap();
}

#[test]
fn stop_during_network_start_does_not_recover_saved_enabled_identities() {
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), recover_options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let identity = create_identity(&owner, 63);
    let before = enable(&owner, &identity);
    wait(owner.stop_networking().unwrap().wait()).unwrap();
    let (embedding, entered, release) = blocked_embedding();
    let start = owner
        .start_networking_with_embedding(host(), embedding)
        .unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let stop = owner.stop_networking().unwrap();
    assert_eq!(owner.network_status().phase, NetworkPhase::Stopping);
    assert_eq!(
        wait(identity.snapshot()).unwrap().runtime,
        identity::RuntimeStatus::Unavailable
    );
    release.send(()).unwrap();
    assert_eq!(wait(start.wait()), Err(Error::StartupCancelled));
    wait(stop.wait()).unwrap();
    let after = wait(identity.snapshot()).unwrap();
    assert_eq!(after.identity, before.identity);
    assert_eq!(after.runtime, identity::RuntimeStatus::Unavailable);
    assert_eq!(owner.network_status().phase, NetworkPhase::Offline);
    wait(owner.close()).unwrap();
}

#[test]
fn cold_offline_owner_reads_and_reopens_without_ever_selecting_a_host() {
    let root = Root::new();
    let owner = Owner::create_new_offline(&root.0, options()).unwrap();
    wait(owner.ready()).unwrap();
    assert_eq!(owner.network_status().phase, NetworkPhase::Offline);
    assert_eq!(owner.network_status().generation, 0);
    assert!(matches!(owner.host(), Err(Error::NotReady)));
    let identity = create_identity(&owner, 93);
    let before = wait(identity.snapshot()).unwrap().identity;
    assert_eq!(owner.network_status().generation, 0);
    wait(owner.close()).unwrap();
    let reopened = Owner::open_existing_offline(&root.0, options()).unwrap();
    wait(reopened.ready()).unwrap();
    assert_eq!(reopened.network_status().phase, NetworkPhase::Offline);
    assert_eq!(reopened.network_status().generation, 0);
    let rows = wait(reopened.identities().unwrap().identities()).unwrap();
    assert_eq!(rows, vec![before]);
    wait(reopened.start_networking(host()).unwrap().wait()).unwrap();
    assert_eq!(reopened.network_status().generation, 1);
    wait(reopened.close()).unwrap();
}

#[test]
fn stop_after_failed_offline_open_settles_and_preserves_startup_failure() {
    let root = Root::new();
    let owner = Owner::open_existing_offline(&root.0, options()).unwrap();
    let failure = wait(owner.ready()).unwrap_err();
    let deadline = Instant::now() + Duration::from_secs(12);
    while !owner.status().cleanup_complete {
        assert!(
            Instant::now() < deadline,
            "failed local owner did not finish cleanup"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
    wait(owner.stop_networking().unwrap().wait()).unwrap();
    assert_eq!(owner.network_status().phase, NetworkPhase::Offline);
    assert_eq!(owner.network_status().generation, 0);
    assert_eq!(owner.status().failure, Some(failure.clone()));
    assert_eq!(wait(owner.ready()), Err(failure.clone()));
    assert!(matches!(owner.start_networking(host()), Err(error) if error == failure));
    assert!(wait(owner.close()).is_err());
    assert!(owner.status().cleanup_complete);
    assert!(!root.0.exists());
}

#[test]
fn public_group_observations_follow_only_their_retained_identity_runner() {
    use prns_groups::{Clock, GroupSyncProgress};
    use prns_groups_core::GroupId;
    fn progress(
        client: &prns_groups::Client,
        group: GroupId,
        predicate: impl Fn(&GroupSyncProgress) -> bool,
    ) -> GroupSyncProgress {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let value = client.synchronization_progress(group);
            if predicate(&value) {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "progress did not settle: {value:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn transition(owner: &Owner, client: &api::IdentityClient, target: identity::TransitionTarget) {
        let prepared = wait(client.prepare(target)).unwrap();
        let token = wait(
            owner
                .identities()
                .unwrap()
                .reserve(prepared.clone(), identity::CommandDigest([71; 32])),
        )
        .unwrap();
        wait(client.begin(token, prepared)).unwrap();
        await_identity(client, |value| value.identity.pending.is_none());
    }
    let root = Root::new();
    let owner = Owner::create_new(&root.0, host(), recover_options()).unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let first = create_identity(&owner, 71);
    let second = create_identity(&owner, 72);
    for identity in [&first, &second] {
        transition(
            &owner,
            identity,
            identity::TransitionTarget::AddService(prns_service_runtime::groups::SERVICE),
        );
    }
    // Exactly the public construction used by the app, before activation.
    let first_client = owner.groups(first.reference()).unwrap();
    let second_client = owner.groups(second.reference()).unwrap();
    let unknown = GroupId([71; 32]);
    assert!(!first_client.synchronization_progress(unknown).running);
    assert!(!second_client.synchronization_progress(unknown).running);
    let mut stale_reference = first.reference();
    stale_reference.id.incarnation[0] ^= 1;
    let stale_client = owner.groups(stale_reference).unwrap();
    enable(&owner, &first);
    progress(&first_client, unknown, |value| value.running);
    assert!(!second_client.synchronization_progress(unknown).running);
    assert!(!stale_client.synchronization_progress(unknown).running);
    // Repeated construction shares the runner; it does not start a second one.
    assert!(
        owner
            .groups(first.reference())
            .unwrap()
            .synchronization_progress(unknown)
            .running
    );
    let prns_groups::Outcome::Created(group) = wait(first_client.execute(
        prns_groups::Request {
            id: prns_groups::create_request_id().unwrap(),
            original_scope: None,
            intent: prns_groups::Intent::Create {
                title: "Observed".into(),
                profile: Default::default(),
            },
        },
        prns_groups::ExecutionContext {
            clock: prns_groups::SystemClock.now(),
        },
    ))
    .unwrap() else {
        panic!("created group expected")
    };
    let detail = wait(first_client.conversation(group, QUERY_PAGE)).unwrap();
    let observed = progress(&first_client, group, |value| value.observed);
    assert_eq!(observed.control, Some(detail.control));
    assert!(observed.running && observed.peers.is_empty());
    enable(&owner, &second);
    progress(&second_client, unknown, |value| value.running);
    transition(&owner, &first, identity::TransitionTarget::Pause);
    progress(&first_client, group, |value| !value.running);
    assert!(second_client.synchronization_progress(unknown).running);
    enable(&owner, &first);
    progress(&first_client, group, |value| {
        value.running && value.observed
    });
    wait(owner.stop_networking().unwrap().wait()).unwrap();
    assert!(!first_client.synchronization_progress(group).running);
    assert!(!second_client.synchronization_progress(unknown).running);
    wait(owner.start_networking(host()).unwrap().wait()).unwrap();
    progress(&first_client, group, |value| {
        value.running && value.observed
    });
    progress(&second_client, unknown, |value| value.running);
    wait(owner.close()).unwrap();
    assert!(!first_client.synchronization_progress(group).running);
    let reopened = Owner::open_existing(&root.0, host(), recover_options()).unwrap();
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    let replacement = reopened.groups(first.reference()).unwrap();
    progress(&replacement, group, |value| value.running && value.observed);
    assert!(
        !first_client.synchronization_progress(group).running,
        "closed owner observed replacement store lifetime"
    );
    assert!(!second_client.synchronization_progress(unknown).running);
    wait(reopened.close()).unwrap();
    assert!(!replacement.synchronization_progress(group).running);
}
