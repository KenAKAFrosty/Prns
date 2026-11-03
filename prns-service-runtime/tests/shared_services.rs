//! Two active services, one identity store/lane, one supplied service executor.
//! The private consumer has durable settings and bounded post-commit delivery;
//! group networking is the actual production adapter, not a mocked service.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "shared_services/driver.rs"]
mod driver;
use driver::{Controls, Driver, Job, SETTINGS};
use prns_groups::{self as domain, Clock};
use prns_host_runtime::{owner::OwnedSession, HostEmbedding};
use prns_identities::{api, creation::CreateRequestId, owned};
use prns_identities_core as core;
use prns_identities_runtime as rt;
use prns_service_runtime::{groups, groups::network, local, stores};
use std::{
    future::Future,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

type Owner = groups::Owner<1, 16, 2, 8, Driver, Job>;
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "prns-shared-services-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn options() -> network::OwnerOptions {
    network::OwnerOptions {
        storage: groups::Options::new(
            local::Options {
                identities: owned::Options {
                    queue_capacity: NonZeroUsize::new(4).unwrap(),
                    budget: rt::Budget {
                        capacity: rt::Resources {
                            held_keys: 1,
                            destinations: 2,
                            request_handlers: 2,
                            links: 1,
                            queue_slots: 32,
                        },
                        infrastructure_reserve: rt::Resources::ZERO,
                    },
                },
                stores: stores::Options {
                    per_identity: prns_storage::database::Options {
                        cache_bytes: 1024 * 1024,
                    },
                    total_cache_bytes: 1024 * 1024,
                },
            },
            domain::retained::Limits {
                max_operations: 16,
                max_bytes: 1024 * 1024,
                ..Default::default()
            },
            domain::enrollment::CatalogLimits::default(),
            domain::ClientLimits::default(),
        ),
        service: network::RunOptions {
            carrier: network::Options {
                operations: 4,
                outgoing_links: 1,
                sync: prns_groups_reticulum::Limits {
                    max_frame_bytes: 131_072,
                    max_page_items: 8,
                    max_peers: 4,
                    inbound_jobs: 2,
                    exchange_timeout: Duration::from_secs(1),
                },
                enrollment: prns_groups_reticulum::enrollment::Limits {
                    max_frame_bytes: 131_072,
                    inbound_jobs: 2,
                    exchange_timeout: Duration::from_secs(1),
                },
            },
            policy: domain::SyncPolicy::default(),
            announce_interval_millis: None,
        },
    }
}
async fn within<F: Future>(phase: &'static str, future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(15), future)
        .await
        .unwrap_or_else(|_| panic!("bounded shared-service progress: {phase}"))
}
async fn transition(
    client: &owned::Client,
    identity: &owned::IdentityClient,
    target: core::TransitionTarget,
) -> core::OperationResult {
    eprintln!("shared-services transition: {target:?}");
    within("identity transition", async {
        let prepared = loop {
            match identity.prepare(target).await {
                Ok(value) => break value,
                Err(_) => tokio::time::sleep(Duration::from_millis(2)).await,
            }
        };
        let token = client
            .reserve(prepared.clone(), core::CommandDigest([3; 32]))
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
async fn named(client: &owned::Client) -> owned::IdentityClient {
    let scope = client.creation_scope().await.unwrap();
    let result = client
        .create_or_recover(
            CreateRequestId {
                scope,
                nonce: [7; 32],
            },
            api::CreateRequest {
                source: core::KeySource::Generate,
                metadata: core::Metadata::new("Shared identity", None).unwrap(),
                services: vec![],
            },
            None,
        )
        .await
        .unwrap();
    let core::OperationResult::Created { identity: id } = result.result else {
        panic!("created identity")
    };
    let record = client
        .identities()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.id == id)
        .unwrap();
    client.identity(record.reference()).unwrap()
}
fn open(
    root: &Root,
    create: bool,
    context: network::Context,
    controls: Arc<Controls>,
) -> Result<Owner, local::Error<network::DriverError>> {
    let input = groups::SubmissionState::new::<1>(domain::InputBudget::default()).unwrap();
    let worker_input = input.clone();
    let factory = move |stores, catalog: &core::Catalog<1, 16, 2>, wake, commands| {
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "owner lane uses no ambient runtime"
        );
        driver::factory(
            stores,
            catalog,
            wake,
            commands,
            context,
            controls,
            worker_input,
        )
    };
    let owner = if create {
        local::Owner::create_new(&root.0, options().storage.local, factory)
    } else {
        local::Owner::open_existing(&root.0, options().storage.local, factory)
    }?;
    Ok(Owner::from_local(owner, input))
}
async fn settings(
    commands: &owned::Commands<Job>,
    identity: core::IdentityReference,
    value: Option<u64>,
) -> u64 {
    within("private durable command", async {
        loop {
            let (reply, receive) = tokio::sync::oneshot::channel();
            match commands.submit(Job::Settings {
                identity,
                value,
                reply,
            }) {
                Ok(()) => return receive.await.unwrap().unwrap(),
                Err(owned::SubmitFailure::Full) => {
                    tokio::time::sleep(Duration::from_millis(1)).await
                }
                Err(error) => panic!("private submission failed: {error:?}"),
            }
        }
    })
    .await
}
fn flood(client: &domain::Client, request: &domain::Request) -> (usize, usize) {
    let mut accepted = 0;
    let mut full = 0;
    // A finite producer fills the one common lane. It cannot keep starving the
    // other service through an unbounded retry/flood loop.
    for _ in 0..256 {
        match client.admit_execute(
            request.clone(),
            domain::ExecutionContext {
                clock: prns_groups_core::ClockReading::Unknown,
            },
        ) {
            Ok(observer) => {
                accepted += 1;
                drop(observer);
            }
            Err(domain::ApiError::Busy) => full += 1,
            Err(error) => panic!("unexpected group admission: {error:?}"),
        }
    }
    (accepted, full)
}

#[test]
fn active_private_and_group_services_share_owner_under_load_and_retained_close() {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        let executor_thread = std::thread::current().id();
        let root = Root::new();
        let router = network::Router::new(1).unwrap();
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
            network::Context::attach(host.client(), executor.handle().clone(), router).unwrap();
        let controls = Controls::new();
        let owner = open(&root, true, context.clone(), controls.clone()).unwrap();
        let identities = owner.identities();
        let identity = named(&identities).await;
        let reference = identity.reference();
        transition(
            &identities,
            &identity,
            core::TransitionTarget::AddService(groups::SERVICE),
        )
        .await;
        transition(
            &identities,
            &identity,
            core::TransitionTarget::AddService(SETTINGS),
        )
        .await;
        assert!(matches!(
            transition(&identities, &identity, core::TransitionTarget::Enable).await,
            core::OperationResult::Enabled { .. }
        ));
        assert_eq!(
            identity.snapshot().await.unwrap().runtime,
            core::RuntimeStatus::Running
        );
        assert!(
            controls.group_prepared.load(Ordering::Acquire),
            "real network registration completed"
        );
        let opened = controls.opened.lock().unwrap().clone();
        assert_eq!(opened.len(), 2);
        for service in [groups::SERVICE, SETTINGS] {
            let permit = opened.iter().find(|p| p.service() == service).unwrap();
            assert_eq!(permit.admission().identity.id, reference.id);
            assert_eq!(
                permit.admission().identity.public_hash,
                reference.public_hash
            );
            assert!(controls
                .scopes
                .lock()
                .unwrap()
                .iter()
                .any(|scope| scope.owner == permit.owner() && scope.service == Some(service)));
        }
        let domains = root.0.join("domains");
        assert!(domains.join("announces").is_dir());
        assert_eq!(
            std::fs::read_dir(domains)
                .unwrap()
                .map(Result::unwrap)
                .filter(|entry| entry.file_name() != "announces")
                .count(),
            1
        );
        let client = groups::client(owner.submission(), reference, Job::Group);
        let request = domain::Request {
            id: domain::create_request_id().unwrap(),
            intent: domain::Intent::Create {
                profile: domain::GroupProfile::default(),
                title: "Accepted without a waiter".into(),
            },
            original_scope: None,
        };
        let observer = client
            .admit_execute(
                request.clone(),
                domain::ExecutionContext {
                    clock: domain::SystemClock.now(),
                },
            )
            .unwrap();
        drop(observer);
        let operation = within("dropped observer completed", async {
            loop {
                if let Some(value) = client.operation(request.id).await.unwrap() {
                    if value.state.is_terminal() {
                        break value;
                    }
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        assert!(matches!(
            operation.state,
            domain::retained::State::Completed(domain::Outcome::Created(_))
        ));
        let (accepted, full) = flood(&client, &request);
        assert!(
            accepted > 0 && full > 0,
            "bounded group admission was saturated"
        );
        assert_eq!(settings(&owner.commands(), reference, Some(42)).await, 42);
        within("private notification", async {
            while controls.notification.load(Ordering::Acquire) != 42 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(controls.group_commands.load(Ordering::Acquire) > 1);
        assert!(controls.settings_commands.load(Ordering::Acquire) > 0);
        {
            let workers = controls.workers.lock().unwrap();
            assert!(!workers.is_empty());
            assert!(workers.iter().all(|thread| *thread == workers[0]));
            assert_ne!(workers[0], executor_thread);
            let databases = controls.databases.lock().unwrap();
            assert!(!databases.is_empty());
            assert!(databases.iter().all(|pointer| *pointer == databases[0]));
            assert_eq!(
                controls.executor_threads.lock().unwrap().as_slice(),
                &[executor_thread]
            );
        }
        let (_, full) = flood(&client, &request);
        assert!(full > 0);
        let completion = owner.close();
        drop(owner);
        assert!(matches!(
            client.admit_execute(
                request.clone(),
                domain::ExecutionContext {
                    clock: prns_groups_core::ClockReading::Unknown
                }
            ),
            Err(domain::ApiError::Unavailable)
        ));
        let mut completion = std::pin::pin!(completion);
        assert!(completion
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        within("private teardown started", async {
            while !controls.settings_waiting.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        assert!(
            controls.group_quiesced.load(Ordering::Acquire),
            "network endpoints retired while private drainage remains owned"
        );
        assert!(
            open(&root, false, context.clone(), Controls::new()).is_err(),
            "original private key/task keeps the owner locked"
        );
        assert!(completion
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        controls.drain.add_permits(1);
        within("original owner drained", completion).await.unwrap();
        assert!(
            context.diagnostics().latest().is_none(),
            "{:?}",
            context.diagnostics().latest()
        );
        assert!(
            host.client().identity_hash().is_ok(),
            "the supplied host remains owned by its caller"
        );

        let reopened = open(&root, false, context.clone(), Controls::new()).unwrap();
        let restored = reopened.identities().identity(reference).unwrap();
        assert_ne!(
            restored.snapshot().await.unwrap().runtime,
            core::RuntimeStatus::Running
        );
        assert_eq!(settings(&reopened.commands(), reference, None).await, 42);
        let recovered = groups::client(reopened.submission(), reference, Job::Group);
        assert_eq!(
            recovered.operation(request.id).await.unwrap().unwrap(),
            operation
        );
        assert!(matches!(
            recovered
                .execute(
                    request,
                    domain::ExecutionContext {
                        clock: prns_groups_core::ClockReading::Unknown
                    }
                )
                .await
                .unwrap(),
            domain::Outcome::Created(_)
        ));
        within("reopened owner closed", reopened.close())
            .await
            .unwrap();
        host.stop().await.unwrap();
    });
}
