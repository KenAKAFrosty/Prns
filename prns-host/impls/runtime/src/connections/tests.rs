#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use prns_host::{
    Bitrate, HostConfig, HostRole, IdentityConfig, InterfaceConfig, PersistenceConfig, PrnsLimits,
};
use std::{
    future::Future,
    net::TcpListener,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    task::{Context, Poll, Wake},
    time::{Duration, Instant},
};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);
pub(super) struct Root(pub PathBuf);
impl Root {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "prns-profiles-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        prns_storage::files::create_private_directory(&path).unwrap();
        Self(path)
    }
    pub fn file(&self) -> PathBuf {
        self.0.join("connections.redb")
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(super) fn options() -> Options {
    Options {
        limits: Limits {
            profiles: 16,
            encoded_bytes: 32 * 1024,
        },
        queue_capacity: std::num::NonZeroUsize::new(8).unwrap(),
        cache_bytes: 1024 * 1024,
    }
}
pub(super) fn profile(id: u8, port: u16) -> Profile {
    Profile {
        id: ProfileId::new([id; 16]).unwrap(),
        label: "TCP peer".into(),
        enabled: true,
        config: tcp_client_config("127.0.0.1", port, Bitrate::Auto).unwrap(),
        routing: None,
    }
}
fn host_config() -> HostConfig {
    HostConfig {
        identity: IdentityConfig::GenerateEphemeral,
        persistence: PersistenceConfig::Ephemeral,
        role: HostRole::Endpoint,
        destinations: Vec::new(),
        required_capabilities: Vec::new(),
        limits: PrnsLimits::balanced(),
    }
}
struct Park(std::thread::Thread);
impl Wake for Park {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
// An ordinary foreign-style executor: no Tokio context around any public call.
pub(super) fn run<T>(future: impl Future<Output = T>) -> T {
    let waker = std::task::Waker::from(Arc::new(Park(std::thread::current())));
    let mut future = Box::pin(future);
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return value;
        }
        assert!(Instant::now() < until, "operation did not settle");
        std::thread::park_timeout(Duration::from_millis(10));
    }
}
fn applied(client: &BindingClient, revision: u64) -> BindingSnapshot {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = run(client.snapshot()).unwrap();
        if snapshot.applied_revision == Some(revision) {
            return snapshot;
        }
        assert!(
            Instant::now() < until,
            "reconciliation did not settle: {snapshot:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn stopped_save_reopen_and_revision_or_incarnation_conflict() {
    let root = Root::new();
    let owner = ProfileOwner::create_new(root.file(), vec![profile(1, 4242)], options()).unwrap();
    let client = owner.client();
    let first = run(client.snapshot()).unwrap();
    let removed = run(client.replace(first.guard, Vec::new())).unwrap();
    assert_eq!(removed.guard.revision, 1);
    assert!(matches!(
        run(client.replace(first.guard, vec![profile(2, 4343)])),
        Err(Error::Conflict(_))
    ));
    let mut wrong_file = removed.guard;
    wrong_file.incarnation = [2; 16];
    assert!(matches!(
        run(client.replace(wrong_file, Vec::new())),
        Err(Error::Conflict(_))
    ));
    run(owner.close()).unwrap();
    assert!(matches!(run(client.snapshot()), Err(Error::Closed)));
    let reopened = ProfileOwner::open_existing(root.file(), options()).unwrap();
    assert_eq!(run(reopened.client().snapshot()).unwrap(), removed);
    assert_eq!(
        run(reopened.client().replace(removed.guard, Vec::new()))
            .unwrap()
            .guard,
        removed.guard
    );
    run(reopened.close()).unwrap();
}
#[test]
fn accepted_save_survives_dropped_waiter_and_owner_close() {
    let root = Root::new();
    let owner = ProfileOwner::create_new(root.file(), Vec::new(), options()).unwrap();
    let client = owner.client();
    let first = run(client.snapshot()).unwrap();
    let (entered, wait_entered) = std::sync::mpsc::sync_channel(1);
    let (release, wait_release) = std::sync::mpsc::channel();
    owner
        .worker
        .client()
        .try_submit(Job::Barrier(entered, wait_release))
        .ok()
        .unwrap();
    wait_entered.recv().unwrap();
    let mut saving = Box::pin(client.replace(first.guard, vec![profile(1, 4242)]));
    assert!(saving
        .as_mut()
        .poll(&mut Context::from_waker(std::task::Waker::noop()))
        .is_pending());
    drop(saving);
    let completion = owner.close();
    drop(owner);
    release.send(()).unwrap();
    run(completion).unwrap();
    let reopened = ProfileOwner::open_existing(root.file(), options()).unwrap();
    let saved = run(reopened.client().snapshot()).unwrap();
    assert_eq!(saved.guard.revision, 1);
    assert_eq!(saved.profiles, vec![profile(1, 4242)]);
    run(reopened.close()).unwrap();
}
#[test]
fn native_generations_restore_tcp_without_javascript_and_fence_before_retirement() {
    let root = Root::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let desired = profile(1, listener.local_addr().unwrap().port());
    let owner = ProfileOwner::create_new(root.file(), vec![desired], options()).unwrap();
    let mut previous_generation = 0;
    for _ in 0..2 {
        let host = run(OwnedSession::open(host_config())).unwrap();
        let binding = run(owner.bind(&host)).unwrap();
        let borrowed = binding.client();
        let snapshot = applied(&borrowed, 0);
        assert!(snapshot.generation > previous_generation);
        previous_generation = snapshot.generation;
        let until = Instant::now() + Duration::from_secs(10);
        let _socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                e => panic!("TCP not restored: {e:?}"),
            }
        };
        let interface = snapshot.attachments[0].interface.unwrap();
        binding.fence();
        assert_eq!(borrowed.status().phase, BindingPhase::Fenced);
        assert!(matches!(
            run(borrowed.retry(snapshot.saved, snapshot.attachments[0].profile.id)),
            Err(Error::Closed)
        ));
        // Fencing leaves the attachment usable for domain drainage.
        assert!(run(host.client().snapshot())
            .unwrap()
            .interfaces
            .iter()
            .any(|i| i.interface_id == interface));
        run(binding.close()).unwrap();
        assert_eq!(borrowed.status().phase, BindingPhase::Closed);
        assert!(!run(host.client().snapshot())
            .unwrap()
            .interfaces
            .iter()
            .any(|i| i.interface_id == interface));
        run(host.stop()).unwrap();
    }
    run(owner.close()).unwrap();
}
#[test]
fn saved_edits_after_fence_do_not_reactivate_and_borrowers_do_not_own_cleanup() {
    let root = Root::new();
    let owner = ProfileOwner::create_new(root.file(), Vec::new(), options()).unwrap();
    let host = run(OwnedSession::open(host_config())).unwrap();
    let binding = run(owner.bind(&host)).unwrap();
    let borrowed = binding.client();
    let original = applied(&borrowed, 0);
    binding.fence();
    let updated = run(owner
        .client()
        .replace(original.saved, vec![profile(1, 4242)]))
    .unwrap();
    let snapshot = run(borrowed.snapshot()).unwrap();
    assert_eq!(snapshot.saved, updated.guard);
    assert_eq!(snapshot.applied_revision, None);
    assert!(snapshot.attachments.is_empty());
    let closed = binding.closed();
    drop(binding);
    run(closed).unwrap();
    assert_eq!(borrowed.status().phase, BindingPhase::Closed);
    run(host.stop()).unwrap();
    run(owner.close()).unwrap();
}

struct WaitingSupervisor;
impl personal_rns::interfaces::ReportsStatus for WaitingSupervisor {}
impl personal_rns::runtime::InterfaceSupervisor for WaitingSupervisor {
    const KIND: personal_rns::interfaces::InterfaceKind =
        personal_rns::interfaces::InterfaceKind::AutoWifi;
    fn channel_tag(&self) -> &[u8] {
        b"profile-retirement-fence"
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
#[test]
fn retirement_is_observable_and_late_disable_prevents_reattachment() {
    let root = Root::new();
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release = Arc::new(tokio::sync::Notify::new());
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let factory_entered = entered.clone();
    let factory_release = release.clone();
    let factory_count = count.clone();
    let embedding = crate::HostEmbedding {
        auto_lan_factory: Some(Arc::new(move |host, _, _| {
            factory_count.fetch_add(1, Ordering::AcqRel);
            let entered = factory_entered.clone();
            let release = factory_release.clone();
            Ok(crate::HostPreparedAttachment::ManagedSupervisor {
                attachment: host.supervise(WaitingSupervisor),
                kind: prns_host::InterfaceKind::AutoLan,
                cleanup: Arc::new(move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    Box::pin(async move {
                        entered.store(true, Ordering::Release);
                        release.notified().await;
                        Ok(())
                    })
                }),
            })
        })),
        ..crate::HostEmbedding::default()
    };
    let lan = Profile {
        id: ProfileId::new([1; 16]).unwrap(),
        label: "LAN".into(),
        enabled: true,
        routing: None,
        config: InterfaceConfig::AutoLan {
            group_id: None,
            discovery_scope: None,
            discovery_port: None,
            data_port: None,
            devices: vec![],
            ignored_devices: vec![],
            multicast_address_type: None,
        },
    };
    let owner = ProfileOwner::create_new(root.file(), vec![lan], options()).unwrap();
    let host = run(OwnedSession::open_with_embedding(host_config(), embedding)).unwrap();
    let binding = run(owner.bind(&host)).unwrap();
    let client = binding.client();
    let snapshot = applied(&client, 0);
    run(client.retry(snapshot.saved, snapshot.attachments[0].profile.id)).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !entered.load(Ordering::Acquire) {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    let before = Instant::now();
    let pending = run(client.snapshot()).unwrap();
    assert!(before.elapsed() < Duration::from_millis(250));
    // Cancelled cleanup observers do not consume the bounded waiter budget.
    for _ in 0..100 {
        let mut cancelled = binding.closed();
        assert!(cancelled
            .as_mut()
            .poll(&mut Context::from_waker(std::task::Waker::noop()))
            .is_pending());
    }
    assert_eq!(pending.attachments[0].state, AttachmentState::Retiring);
    let removed = run(owner.client().replace(snapshot.saved, Vec::new())).unwrap();
    release.notify_one();
    let settled = applied(&client, removed.guard.revision);
    assert!(settled.attachments.is_empty());
    assert_eq!(count.load(Ordering::Acquire), 1);
    run(binding.close()).unwrap();
    run(host.stop()).unwrap();
    run(owner.close()).unwrap();
}

#[test]
fn failed_write_and_lost_commit_confirmation_recover_by_reading_exact_intent() {
    for (fault, expected_revision) in [
        (store::Fault::BeforeCommit, 0),
        (store::Fault::AfterCommit, 1),
    ] {
        let root = Root::new();
        let owner = ProfileOwner::create_new(root.file(), Vec::new(), options()).unwrap();
        let client = owner.client();
        let before = run(client.snapshot()).unwrap();
        let (send, receive) = oneshot::channel();
        owner
            .worker
            .client()
            .try_submit(Job::Fault(fault, send))
            .ok()
            .unwrap();
        run(receive).unwrap();
        assert!(matches!(
            run(client.replace(before.guard, vec![profile(1, 4242)])),
            Err(Error::File(_))
        ));
        let recovered = run(client.snapshot()).unwrap();
        assert_eq!(recovered.guard.revision, expected_revision);
        assert_eq!(recovered.profiles.len(), expected_revision as usize);
        run(owner.close()).unwrap();
        let reopened = ProfileOwner::open_existing(root.file(), options()).unwrap();
        assert_eq!(run(reopened.client().snapshot()).unwrap(), recovered);
        run(reopened.close()).unwrap();
    }
}

#[test]
fn cancelling_bind_after_admission_does_not_leave_a_live_generation() {
    let root = Root::new();
    let owner = ProfileOwner::create_new(root.file(), Vec::new(), options()).unwrap();
    let host = run(OwnedSession::open(host_config())).unwrap();
    let (entered, wait_entered) = std::sync::mpsc::sync_channel(1);
    let (release, wait_release) = std::sync::mpsc::channel();
    owner
        .worker
        .client()
        .try_submit(Job::Barrier(entered, wait_release))
        .ok()
        .unwrap();
    wait_entered.recv().unwrap();
    let mut pending = Box::pin(owner.bind(&host));
    assert!(pending
        .as_mut()
        .poll(&mut Context::from_waker(std::task::Waker::noop()))
        .is_pending());
    drop(pending);
    release.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let binding = loop {
        match run(owner.bind(&host)) {
            Ok(binding) => break binding,
            Err(Error::AlreadyBound) if Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(5))
            }
            result => panic!("cancelled binding did not retire: {:?}", result.err()),
        }
    };
    run(binding.close()).unwrap();
    run(host.stop()).unwrap();
    run(owner.close()).unwrap();
}
