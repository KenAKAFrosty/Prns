//! Two independent native embeddings use the public owner, generated identity
//! keys, signed claims and ordinary group Client over real authenticated Links.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[allow(dead_code)]
#[path = "../../../prns-groups/tests/support/journey.rs"]
mod journey;
#[allow(dead_code)]
#[path = "../../../prns-groups/transport/reticulum/tests/support/link_wire.rs"]
mod wire;
use journey::*;

use groups::network;
use prns_groups::{
    self as domain, enrollment::InvitationState, Clock, GroupLifecycle, Intent, SystemClock,
};
use prns_host_runtime::{owner::OwnedSession, HostEmbedding, HostPreparedAttachment};
use prns_identities::{api, creation::CreateRequestId, owned};
use prns_identities_core as identity;
use prns_identities_runtime as runtime;
use prns_service_runtime::{groups, local, stores};
use std::{
    num::NonZeroUsize,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "composed-peers-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        prns_storage::files::create_private_directory(&path).unwrap();
        Self(path)
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
                    queue_capacity: NonZeroUsize::new(64).unwrap(),
                    budget: runtime::Budget {
                        capacity: runtime::Resources {
                            held_keys: 2,
                            destinations: 4,
                            request_handlers: 4,
                            links: 16,
                            queue_slots: 128,
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
            domain::retained::Limits {
                max_operations: 32,
                max_bytes: 2 * 1024 * 1024,
                ..Default::default()
            },
            domain::enrollment::CatalogLimits::default(),
            domain::ClientLimits::default(),
        )
        .with_archive(domain::archive::Limits {
            max_records: 256,
            max_bytes: 8 * 1024 * 1024,
        }),
        service: network::RunOptions {
            carrier: network::Options {
                operations: 16,
                outgoing_links: 8,
                sync: prns_groups_reticulum::Limits {
                    max_frame_bytes: 131_072,
                    max_page_items: 8,
                    max_peers: 4,
                    inbound_jobs: 16,
                    exchange_timeout: Duration::from_secs(5),
                },
                enrollment: prns_groups_reticulum::enrollment::Limits {
                    max_frame_bytes: prns_groups_reticulum::enrollment::MAX_FRAME_BYTES,
                    inbound_jobs: 4,
                    exchange_timeout: Duration::from_secs(5),
                },
            },
            policy: domain::SyncPolicy::default(),
            announce_interval_millis: None,
        },
    }
}
async fn named(client: &owned::Client, nonce: u8, name: &str) -> owned::IdentityClient {
    let scope = client.creation_scope().await.unwrap();
    let result = client
        .create_or_recover(
            CreateRequestId {
                scope,
                nonce: [nonce; 32],
            },
            api::CreateRequest {
                source: identity::KeySource::Generate,
                metadata: identity::Metadata::new(name, None).unwrap(),
                services: vec![],
            },
            None,
        )
        .await
        .unwrap();
    let identity::OperationResult::Created { identity: id } = result.result else {
        panic!("identity creation");
    };
    let record = client
        .identities()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.id == id)
        .unwrap();
    client.identity(record.reference()).unwrap()
}
async fn transition(
    client: &owned::Client,
    target: &owned::IdentityClient,
    action: identity::TransitionTarget,
) -> identity::OperationResult {
    tokio::time::timeout(Duration::from_secs(20), async {
        let prepared = loop {
            match target.prepare(action).await {
                Ok(value) => break value,
                Err(_) => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        };
        let token = client
            .reserve(prepared.clone(), identity::CommandDigest([7; 32]))
            .await
            .unwrap();
        target.begin(token, prepared.clone()).await.unwrap();
        loop {
            if let identity::OperationState::Completed(result) = client
                .operation(token, api::Command::Transition(prepared.clone()))
                .await
                .unwrap()
            {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("native identity transition completes")
}
async fn card(client: &owned::IdentityClient, name: &str) -> prns_host::IdentityHash {
    let bytes = client
        .contact_card(
            prns_groups_reticulum::contact_card::enrollment_service_name_hash(),
            name.into(),
        )
        .await
        .unwrap();
    let mut uri = [0; prns_contact_card::MAX_URI_BYTES];
    let length = prns_contact_card::encode_uri(&bytes, &mut uri).unwrap();
    let mut decoded = [0; prns_contact_card::MAX_CARD_BYTES];
    let length =
        prns_contact_card::decode_uri(std::str::from_utf8(&uri[..length]).unwrap(), &mut decoded)
            .unwrap();
    let verified = prns_contact_card::verify(&decoded[..length]).unwrap();
    let recipient = prns_groups_reticulum::contact_card::enrollment_recipient(&verified).unwrap();
    assert_eq!(recipient.as_bytes(), &client.reference().public_hash.0);
    assert_eq!(verified.display_name(), name);
    assert_eq!(
        verified.destination_hash(),
        prns_groups_reticulum::enrollment_destination_for(recipient)
            .unwrap()
            .as_bytes()
    );
    recipient
}

type Owner = network::Owner<2, 32, 2, 16>;
#[test]
fn generated_named_identities_bootstrap_deliver_and_recover_on_supplied_hosts() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let root = Root::new();
            let (interfaces, state, forwarding) = wire::prepare::<2>();
            let mut hosts = Vec::new();
            let mut contexts = Vec::new();
            for interface in interfaces {
                let router = network::Router::new(2).unwrap();
                let mut embedding = HostEmbedding {
                    prepare_interfaces: Some(Box::new(move |client| {
                        Ok(vec![HostPreparedAttachment::Interface {
                            attachment: client.protocols().attach(interface),
                            kind: prns_host::InterfaceKind::Pipe,
                        }])
                    })),
                    ..HostEmbedding::default()
                };
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
                contexts.push(
                    network::Context::attach(
                        host.client(),
                        tokio::runtime::Handle::current(),
                        router,
                    )
                    .unwrap(),
                );
                hosts.push(host);
            }
            let forwarder = tokio::spawn(forwarding);
            let paths = [root.0.join("alice"), root.0.join("bob")];
            let mut owners: Vec<Option<Owner>> = paths
                .iter()
                .zip(&contexts)
                .map(|(path, context)| {
                    Some(network::create_new(path, options(), context.clone()).unwrap())
                })
                .collect();
            let mut named_clients = Vec::new();
            let mut identities = Vec::new();
            for (index, name) in ["Alice", "Bob"].into_iter().enumerate() {
                let catalog = owners[index].as_ref().unwrap().identities();
                let identity = named(&catalog, index as u8 + 1, name).await;
                // Signing an offline claim creates neither schema nor network service.
                identities.push(card(&identity, name).await);
                assert!(identity
                    .snapshot()
                    .await
                    .unwrap()
                    .identity
                    .services
                    .is_empty());
                assert_eq!(
                    std::fs::read_dir(paths[index].join("domains"))
                        .unwrap()
                        .count(),
                    0
                );
                assert!(matches!(
                    transition(
                        &catalog,
                        &identity,
                        identity::TransitionTarget::AddService(groups::SERVICE)
                    )
                    .await,
                    identity::OperationResult::ServiceAdded { .. }
                ));
                assert!(matches!(
                    transition(&catalog, &identity, identity::TransitionTarget::Enable).await,
                    identity::OperationResult::Enabled { .. }
                ));
                named_clients.push(identity);
            }
            let mut clients: Vec<_> = owners
                .iter()
                .zip(&named_clients)
                .map(|(owner, identity)| {
                    groups::client(
                        owner.as_ref().unwrap().submission(),
                        identity.reference(),
                        network::Command::Group,
                    )
                })
                .collect();
            for client in &clients {
                let page = client.conversations(None, QUERY_PAGE).await.unwrap();
                assert!(page.items.is_empty() && page.next.is_none());
            }
            let (group_a, group_b) = tokio::join!(
                create(&clients[0], "Alice initiated"),
                create(&clients[1], "Bob initiated")
            );
            assert_ne!(group_a, group_b);
            let (offer_a, offer_b) = tokio::join!(
                invite(&clients[0], group_a, identities[1]),
                invite(&clients[1], group_b, identities[0])
            );
            for (recipient, offer) in [(&clients[1], offer_a), (&clients[0], offer_b)] {
                eventually(
                    "invitation reaches the intended catalog identity",
                    || async {
                        let page = recipient
                            .invitations(now(), None, QUERY_PAGE)
                            .await
                            .unwrap();
                        assert!(
                            page.next.is_none(),
                            "the fixture invitation catalog fits this page"
                        );
                        page.items
                            .iter()
                            .any(|item| item.id == offer && item.state == InvitationState::Pending)
                    },
                )
                .await;
                let page = recipient.conversations(None, QUERY_PAGE).await.unwrap();
                assert!(page.next.is_none());
                assert_eq!(page.items.len(), 1, "no membership before consent");
            }
            tokio::join!(
                execute(
                    &clients[1],
                    Intent::Respond {
                        invitation: offer_a,
                        accept: true
                    }
                ),
                execute(
                    &clients[0],
                    Intent::Respond {
                        invitation: offer_b,
                        accept: true
                    }
                )
            );
            tokio::join!(
                members(&clients[0], group_a),
                members(&clients[1], group_a),
                members(&clients[0], group_b),
                members(&clients[1], group_b)
            );
            let payload = binary(group_a, 2048);
            let expected = payload.body.clone();
            let first = publish(&clients[0], payload).await;
            let second = publish(&clients[1], binary(group_b, 128)).await;
            let received = record(&clients[1], group_a, first, false).await;
            assert_eq!(received.record.body.as_slice(), expected.as_slice());
            assert_eq!(received.author_identity, identities[0]);
            tokio::join!(
                record(&clients[0], group_a, first, true),
                record(&clients[0], group_b, second, false),
                record(&clients[1], group_b, second, true)
            );

            // Pause drains the actual runner/key/endpoints while ordinary local
            // history remains available. Reopening has no activation side effects.
            let reference = named_clients[1].reference();
            let catalog = owners[1].as_ref().unwrap().identities();
            assert!(matches!(
                transition(
                    &catalog,
                    &named_clients[1],
                    identity::TransitionTarget::Pause
                )
                .await,
                identity::OperationResult::Paused { .. }
            ));
            assert_eq!(
                clients[1]
                    .records(group_a, None, 100, SystemClock.now())
                    .await
                    .unwrap()
                    .records
                    .len(),
                1
            );
            let offline = publish(&clients[0], binary(group_a, 1024)).await;
            assert_eq!(
                record(&clients[0], group_a, offline, false)
                    .await
                    .progress
                    .summary
                    .pending,
                1
            );
            let owner = owners[1].take().unwrap();
            owner.close().await.unwrap();
            drop(owner);
            assert!(clients[1].identity().await.is_err());
            let reopened: Owner =
                network::open_existing(&paths[1], options(), contexts[1].clone()).unwrap();
            let catalog = reopened.identities();
            named_clients[1] = catalog.identity(reference).unwrap();
            assert_eq!(
                named_clients[1].snapshot().await.unwrap().identity.desired,
                identity::DesiredState::Paused
            );
            clients[1] = groups::client(reopened.submission(), reference, network::Command::Group);
            assert!(matches!(
                transition(
                    &catalog,
                    &named_clients[1],
                    identity::TransitionTarget::Enable
                )
                .await,
                identity::OperationResult::Enabled { .. }
            ));
            owners[1] = Some(reopened);
            tokio::join!(
                record(&clients[1], group_a, offline, false),
                record(&clients[0], group_a, offline, true)
            );
            execute(&clients[0], Intent::Close { group: group_a }).await;
            lifecycle(&clients[1], group_a, GroupLifecycle::Closed).await;
            execute(&clients[1], Intent::Close { group: group_b }).await;
            lifecycle(&clients[0], group_b, GroupLifecycle::Closed).await;
            for owner in owners.into_iter().flatten() {
                owner.close().await.unwrap();
            }
            for (index, path) in paths.iter().enumerate() {
                let owner: Owner =
                    network::open_existing(path, options(), contexts[index].clone()).unwrap();
                let identity = owner
                    .identities()
                    .identity(named_clients[index].reference())
                    .unwrap();
                assert_eq!(
                    identity.snapshot().await.unwrap().runtime,
                    identity::RuntimeStatus::Unavailable
                );
                let client = groups::client(
                    owner.submission(),
                    identity.reference(),
                    network::Command::Group,
                );
                assert_eq!(
                    client
                        .records(group_a, None, 100, SystemClock.now())
                        .await
                        .unwrap()
                        .records
                        .len(),
                    2
                );
                for group in [group_a, group_b] {
                    execute(&client, Intent::DeleteLocal { group }).await;
                }
                let page = client.conversations(None, QUERY_PAGE).await.unwrap();
                assert!(page.items.is_empty() && page.next.is_none());
                owner.close().await.unwrap();
            }
            assert!(state.resources.load(Ordering::SeqCst) > 0);
            assert!(state.resource_proofs.load(Ordering::SeqCst) > 0);
            eprintln!(
                "composed peer proof: frames={:?}, resources={}, resource_proofs={}",
                state
                    .frames
                    .iter()
                    .map(|count| count.load(Ordering::SeqCst))
                    .collect::<Vec<_>>(),
                state.resources.load(Ordering::SeqCst),
                state.resource_proofs.load(Ordering::SeqCst)
            );
            for host in hosts {
                host.stop().await.unwrap();
            }
            forwarder.abort();
        });
}

#[test]
fn enrollment_advertises_committed_published_name_and_updates_without_extra_traffic() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let root = Root::new();
            let (interfaces, wire, forwarding) = wire::prepare::<2>();
            let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
            let mut hosts = Vec::new();
            let mut contexts = Vec::new();
            for (index, interface) in interfaces.into_iter().enumerate() {
                let router = network::Router::new(2).unwrap();
                let capture = observed.clone();
                let mut embedding = HostEmbedding {
                    prepare_interfaces: Some(Box::new(move |client| {
                        Ok(vec![HostPreparedAttachment::Interface {
                            attachment: client.protocols().attach(interface),
                            kind: prns_host::InterfaceKind::Pipe,
                        }])
                    })),
                    accepted_announces: if index == 1 {
                        Some(Box::new(move |event| {
                            if !event.app_data.is_empty() {
                                let mut values = capture.lock().unwrap();
                                if values.len() < 8 {
                                    values.push(event.app_data.to_vec());
                                }
                            }
                        }))
                    } else {
                        None
                    },
                    ..HostEmbedding::default()
                };
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
                contexts.push(
                    network::Context::attach(
                        host.client(),
                        tokio::runtime::Handle::current(),
                        router,
                    )
                    .unwrap(),
                );
                hosts.push(host);
            }
            let forwarder = tokio::spawn(forwarding);
            let path = root.0.join("advertiser");
            let owner: Owner = network::create_new(&path, options(), contexts[0].clone()).unwrap();
            let catalog = owner.identities();
            let identity = named(&catalog, 9, "private local label").await;
            let before = identity.snapshot().await.unwrap().identity;
            let guard = identity
                .update_metadata(
                    before.metadata_guard(),
                    identity::Metadata::new("private local label", Some("Public Alice")).unwrap(),
                )
                .await
                .unwrap();
            let added = transition(
                &catalog,
                &identity,
                identity::TransitionTarget::AddService(groups::SERVICE),
            )
            .await;
            assert!(
                matches!(added, identity::OperationResult::ServiceAdded { .. }),
                "{added:?}"
            );
            let enabled = transition(&catalog, &identity, identity::TransitionTarget::Enable).await;
            assert!(
                matches!(enabled, identity::OperationResult::Enabled { .. }),
                "{enabled:?}; {:?}",
                contexts[0].diagnostics().latest()
            );
            let group_client = groups::client(
                owner.submission(),
                identity.reference(),
                network::Command::Group,
            );
            tokio::time::timeout(Duration::from_secs(4), async {
                while !group_client
                    .synchronization_progress(prns_groups_core::GroupId([0; 32]))
                    .running
                {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
            let destination = prns_groups_reticulum::enrollment_destination_for(
                prns_host::IdentityHash::new(identity.reference().public_hash.0),
            )
            .unwrap();
            let advertising_host = hosts[0].client();
            let announce = || {
                advertising_host.execute(prns_host::HostCommand::Announce {
                    destination,
                    interface: None,
                })
            };
            announce().await.unwrap().unwrap();
            tokio::time::timeout(Duration::from_secs(4), async {
                while observed.lock().unwrap().is_empty() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            let first = observed.lock().unwrap()[0].clone();
            let card = prns_contact_card::verify(&first).unwrap();
            assert_eq!(card.display_name(), "Public Alice");
            assert_eq!(card.identity_hash(), &identity.reference().public_hash.0);
            assert_eq!(card.destination_hash(), destination.as_bytes());
            assert_eq!(
                hosts[1]
                    .client()
                    .destination_public_key(destination)
                    .await
                    .unwrap()
                    .unwrap()
                    .as_slice(),
                card.public_identity()
            );

            // Invalid new names and stale commits never change the published claim.
            assert!(identity
                .update_metadata(
                    guard,
                    identity::Metadata::new("local", Some("bad\u{202e}name")).unwrap()
                )
                .await
                .is_err());
            let name = "A".repeat(128);
            let current = identity
                .update_metadata(
                    guard,
                    identity::Metadata::new("another private label", Some(&name)).unwrap(),
                )
                .await
                .unwrap();
            assert!(identity
                .update_metadata(
                    guard,
                    identity::Metadata::new("stale", Some("Stale name")).unwrap()
                )
                .await
                .is_err());
            let count = wire.unsolicited_announces.load(Ordering::SeqCst);
            // Route acceptance requires a newer whole-second emission stamp.
            // This also observes that publishing metadata itself emits nothing.
            tokio::time::sleep(Duration::from_millis(1100)).await;
            assert_eq!(
                wire.unsolicited_announces.load(Ordering::SeqCst),
                count,
                "metadata publication must not send unsolicited announcements"
            );
            announce().await.unwrap().unwrap();
            tokio::time::timeout(Duration::from_secs(4), async {
                while observed.lock().unwrap().len() < 2 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            let latest = observed.lock().unwrap().last().unwrap().clone();
            assert_eq!(latest.len(), prns_contact_card::MAX_CARD_BYTES);
            assert!(
                prns_core::routing::announce::emit::AnnounceAppDataBytes::from_slice(&latest)
                    .is_ok()
            );
            assert_eq!(
                prns_contact_card::verify(&latest).unwrap().display_name(),
                name
            );
            assert_eq!(
                identity.snapshot().await.unwrap().identity.metadata_guard(),
                current
            );
            assert_eq!(
                identity.snapshot().await.unwrap().runtime,
                identity::RuntimeStatus::Running
            );
            owner.close().await.unwrap();
            for host in hosts {
                host.stop().await.unwrap();
            }
            forwarder.abort();
            let _ = forwarder.await;
        });
}
