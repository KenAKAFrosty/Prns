#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::panic)]
use prns_groups as group;
use prns_groups::Clock;
use prns_identities::{api, creation::CreateRequestId, owned};
use prns_identities_core as core;
use prns_identities_runtime as rt;
use prns_service_runtime::{groups, local, stores};
use std::{
    future::Future,
    num::NonZeroUsize,
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
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
fn wait<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        assert!(Instant::now() < deadline, "native completion stalled");
        std::thread::park_timeout(Duration::from_millis(5));
    }
}
static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "prns-assembly-{}-{}",
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
fn options() -> groups::Options {
    groups::Options::new(
        local::Options {
            identities: owned::Options {
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
            },
            stores: stores::Options {
                per_identity: prns_storage::database::Options {
                    cache_bytes: 1024 * 1024,
                },
                total_cache_bytes: 4 * 1024 * 1024,
            },
        },
        group::retained::Limits {
            max_operations: 16,
            max_bytes: 1024 * 1024,
            ..Default::default()
        },
        group::enrollment::CatalogLimits::default(),
        group::ClientLimits::default(),
    )
}
fn create(
    client: &owned::Client,
    nonce: u8,
    services: core::ServiceSet<4>,
) -> owned::IdentityClient {
    let scope = wait(client.creation_scope()).unwrap();
    let result = wait(client.create_or_recover(
        CreateRequestId {
            scope,
            nonce: [nonce; 32],
        },
        api::CreateRequest {
            source: core::KeySource::Generate,
            metadata: core::Metadata::new("local", None).unwrap(),
            services: services.iter().collect(),
        },
        None,
    ))
    .unwrap();
    let core::OperationResult::Created { identity } = result.result else {
        panic!("unexpected creation result");
    };
    let record = wait(client.identities())
        .unwrap()
        .into_iter()
        .find(|record| record.id == identity)
        .unwrap();
    client.identity(record.reference()).unwrap()
}
fn begin(
    client: &owned::Client,
    identity: &owned::IdentityClient,
    target: core::TransitionTarget,
) -> (core::OperationToken, api::PreparedTransition) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let prepared = loop {
        match wait(identity.prepare(target)) {
            Ok(value) => break value,
            Err(error) => {
                assert!(Instant::now() < deadline, "prepare failed: {error:?}");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    };
    let token = wait(client.reserve(prepared.clone(), core::CommandDigest([3; 32]))).unwrap();
    wait(identity.begin(token, prepared.clone())).unwrap();
    (token, prepared)
}
fn completed(
    client: &owned::Client,
    token: core::OperationToken,
    prepared: api::PreparedTransition,
) -> core::OperationResult {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let core::OperationState::Completed(result) =
            wait(client.operation(token, api::Command::Transition(prepared.clone()))).unwrap()
        {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "transition stalled: {:?}",
            wait(
                client
                    .identity(core::IdentityReference {
                        id: prepared.identity.id,
                        public_hash: prepared.identity.public_hash,
                    })
                    .unwrap()
                    .snapshot()
            )
            .unwrap()
            .last_error
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn transition(
    client: &owned::Client,
    identity: &owned::IdentityClient,
    target: core::TransitionTarget,
) -> core::OperationResult {
    let (token, prepared) = begin(client, identity, target);
    completed(client, token, prepared)
}

#[test]
fn dynamic_group_schema_ordinary_client_and_strict_reopen_share_one_owner() {
    let root = Root::new();
    let owner = groups::create_new::<4, 32, 4, 16>(&root.0, options()).unwrap();
    let client = owner.identities();
    let identity = create(&client, 1, core::ServiceSet::empty());
    let reference = identity.reference();
    assert_eq!(
        std::fs::read_dir(root.0.join("domains")).unwrap().count(),
        0
    );
    let unavailable = groups::client(owner.submission(), reference, |job| job);
    assert!(wait(unavailable.conversations(None, QUERY_PAGE)).is_err());
    let mut wrong = reference;
    wrong.public_hash.0[0] ^= 1;
    let wrong = groups::client(owner.submission(), wrong, |job| job);
    assert!(wait(wrong.conversations(None, QUERY_PAGE)).is_err());
    assert_eq!(
        std::fs::read_dir(root.0.join("domains")).unwrap().count(),
        0
    );
    assert!(matches!(
        transition(
            &client,
            &identity,
            core::TransitionTarget::AddService(groups::SERVICE)
        ),
        core::OperationResult::ServiceAdded { .. }
    ));
    assert_eq!(
        std::fs::read_dir(root.0.join("domains")).unwrap().count(),
        1
    );
    let groups = groups::client(owner.submission(), reference, |job| job);
    let observation = wait(groups.subscribe()).unwrap();
    let page = wait(groups.conversations(None, QUERY_PAGE)).unwrap();
    assert!(page.items.is_empty() && page.next.is_none());
    let mut next = std::pin::pin!(observation.next());
    assert!(
        next.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending(),
        "borrowed manager drop closed retained attachment"
    );
    let request = group::create_request_id().unwrap();
    assert!(matches!(
        wait(groups.submit(
            request,
            group::Intent::Create {
                profile: group::GroupProfile::default(),
                title: "blocked while paused".into()
            },
            group::ExecutionContext {
                clock: group::SystemClock.now()
            }
        )),
        Err(group::ApiError::Unavailable)
    ));
    assert!(
        wait(groups.operation(request)).unwrap().is_none(),
        "denied fresh work must not persist intent"
    );
    assert!(matches!(
        transition(&client, &identity, core::TransitionTarget::Enable),
        core::OperationResult::ActivationFailed {
            reason: core::ServiceFailure::Unavailable(1),
            ..
        }
    ));
    assert_ne!(
        wait(identity.snapshot()).unwrap().runtime,
        core::RuntimeStatus::Running
    );
    assert!(groups::open_existing::<4, 32, 4, 16>(&root.0, options()).is_err());
    wait(owner.close()).unwrap();
    assert!(matches!(wait(next), Err(group::ApiError::Unavailable)));
    // Closed observers and retained ordinary clients must not retain the
    // attachment's Arc/database lock after the actual close acknowledgement.
    let reopened = groups::open_existing::<4, 32, 4, 16>(&root.0, options()).unwrap();
    drop(owner);
    let after = groups::client(reopened.submission(), reference, |job| job);
    let page = wait(after.conversations(None, QUERY_PAGE)).unwrap();
    assert!(page.items.is_empty() && page.next.is_none());
    let path =
        prns_identities::scoped_store::path_under(reference, &root.0.join("domains")).unwrap();
    wait(reopened.close()).unwrap();
    drop(reopened);
    std::fs::remove_file(&path).unwrap();
    assert!(groups::open_existing::<4, 32, 4, 16>(&root.0, options()).is_err());
    assert!(
        !path.exists(),
        "ordinary reopen must not recreate configured storage"
    );
}

#[test]
fn invalid_options_and_unknown_configured_service_preserve_local_state() {
    let root = Root::new();
    let mut bad = options();
    bad.retention.max_operations = 0;
    assert!(groups::create_new::<4, 32, 4, 16>(&root.0, bad).is_err());
    assert!(!root.0.exists());
    let bad = options().with_client_limits(group::ClientLimits {
        max_job_bytes: 0,
        max_pending_job_bytes: 0,
    });
    assert!(groups::create_new::<4, 32, 4, 16>(&root.0, bad).is_err());
    assert!(!root.0.exists());
    let owner = groups::create_new::<4, 32, 4, 16>(&root.0, options()).unwrap();
    let identity = create(
        &owner.identities(),
        1,
        core::ServiceSet::new(&[core::ServiceId(900)]).unwrap(),
    );
    assert!(wait(identity.prepare(core::TransitionTarget::Retire)).is_err());
    wait(owner.close()).unwrap();
    drop(owner);
    assert!(groups::open_existing::<4, 32, 4, 16>(&root.0, options()).is_err());
    assert_eq!(
        std::fs::read_dir(root.0.join("domains")).unwrap().count(),
        0
    );
}

#[path = "local/private.rs"]
mod private;

#[path = "local/catalog.rs"]
mod catalog;

#[cfg(feature = "archive")]
#[path = "local/archive.rs"]
mod archive;
