//! Bootstrap traffic crosses independent Reticulum nodes, without MLS peers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
#[path = "support/wire.rs"]
mod wire;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use prns_groups_reticulum::{enrollment, NetworkError};
use prns_groups_service::{Endpoint, EndpointId, EndpointIncarnation};

fn endpoint(peer: usize) -> Endpoint {
    Endpoint {
        id: EndpointId([(peer + 1) as u8; 32]),
        incarnation: EndpointIncarnation::new(1).unwrap(),
    }
}

async fn no_links(owner: &prns_host_runtime::owner::OwnedSession) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while owner.client().snapshot().await.unwrap().active_link_count != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the exchange closes its link without stopping the shared host");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_carrier_authenticates_explicit_identity_and_fences_without_stopping_host() {
    use prns_host_runtime::service_scope::HostServiceScopeError;
    let mut nodes = wire::Nodes::open_scoped().await;
    let alice = Arc::clone(&nodes.enrollment_networks[0]);
    let bob = Arc::clone(&nodes.enrollment_networks[1]);
    let carol = Arc::clone(&nodes.enrollment_networks[2]);
    for (index, scope) in nodes.scopes.iter().enumerate() {
        assert_ne!(
            scope.identity_hash(),
            nodes.owners[index].client().identity_hash().unwrap()
        );
    }
    assert!(matches!(
        enrollment::Network::new(
            nodes.owners[0].client(),
            enrollment::destination_for(alice.local_identity()).unwrap(),
            wire::enrollment_limits()
        ),
        Err(NetworkError::IdentityMismatch)
    ));
    assert!(matches!(
        enrollment::Network::scoped(
            nodes.scopes[0].clone(),
            enrollment::destination_for(bob.local_identity()).unwrap(),
            wire::enrollment_limits()
        ),
        Err(NetworkError::IdentityMismatch)
    ));

    let mut bob_jobs = nodes.enrollment_jobs[1].take().unwrap();
    let expected_alice = alice.local_identity();
    let bob_id = bob.local_identity();
    let sending =
        tokio::spawn(async move { alice.exchange(bob_id, b"admitted before pause").await });
    let job = tokio::time::timeout(Duration::from_secs(5), bob_jobs.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.requester(), expected_alice);
    nodes.scopes[0].fence();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), nodes.scopes[0].wait_drained())
            .await
            .is_err()
    );
    assert_eq!(
        nodes.enrollment_networks[0]
            .exchange(bob_id, b"stale generation")
            .await,
        Err(NetworkError::Scope(HostServiceScopeError::Fenced))
    );
    bob.respond(&job, b"durable reply").await.unwrap();
    assert_eq!(sending.await.unwrap().unwrap(), b"durable reply");
    tokio::time::timeout(Duration::from_secs(5), nodes.scopes[0].wait_drained())
        .await
        .unwrap();
    no_links(&nodes.owners[0]).await;
    assert!(nodes.scopes[0].outgoing_links().unwrap().is_empty());

    // Another identity continues using its host and the same peer normally.
    let expected_carol = carol.local_identity();
    let send = carol.exchange(bob_id, b"unrelated owner");
    let receive = async {
        let job = bob_jobs.recv().await.unwrap();
        assert_eq!(job.requester(), expected_carol);
        bob.respond(&job, b"still running").await.unwrap();
    };
    let (reply, ()) = tokio::join!(send, receive);
    assert_eq!(reply.unwrap(), b"still running");
    assert!(nodes.owners[0].client().snapshot().await.is_ok());
    nodes.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_group_and_enrollment_paths_need_no_unsolicited_announces() {
    // Registration/authorization creates no broadcast. The caller knows only
    // the target identity; the path response must supply its verified key.
    let mut nodes = wire::Nodes::open().await;
    let alice = Arc::clone(&nodes.enrollment_networks[0]);
    let bob = Arc::clone(&nodes.enrollment_networks[1]);
    let mut jobs = nodes.enrollment_jobs[1].take().unwrap();
    assert_eq!(nodes.state.unsolicited_announces.load(Ordering::SeqCst), 0);
    assert_eq!(nodes.state.path_responses.load(Ordering::SeqCst), 0);
    let send = alice.exchange(bob.local_identity(), b"cold enrollment");
    let receive = async {
        let job = jobs.recv().await.unwrap();
        assert_eq!(job.requester(), alice.local_identity());
        bob.respond(&job, b"enrollment response").await.unwrap();
    };
    let (reply, ()) = tokio::join!(send, receive);
    assert_eq!(reply.unwrap(), b"enrollment response");

    let network = Arc::clone(&nodes.networks[0]);
    let remote = Arc::clone(&nodes.networks[1]);
    let mut jobs = nodes.jobs[1].take().unwrap();
    let peer = network.peer(endpoint(1)).unwrap();
    assert_eq!(network.path_capabilities(peer).await.unwrap().hops, None);
    let connection = network.connect(peer).await.unwrap();
    let send = connection.request_bounded(b"cold sync", 13);
    let receive = async {
        let job = jobs.recv().await.unwrap();
        remote.respond(&job, b"sync response").await.unwrap();
    };
    let (reply, ()) = tokio::join!(send, receive);
    assert_eq!(reply.unwrap(), b"sync response");
    connection.close().await.unwrap();
    assert!(nodes.state.path_responses.load(Ordering::SeqCst) >= 2);
    assert_eq!(nodes.state.unsolicited_announces.load(Ordering::SeqCst), 0);
    nodes.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fast_path_allows_a_reply_during_reticulums_native_processing_grace() {
    let mut nodes = wire::Nodes::open().await;
    let alice = Arc::clone(&nodes.networks[0]);
    let bob = Arc::clone(&nodes.networks[1]);
    let mut jobs = nodes.jobs[1].take().unwrap();
    let connection = alice
        .connect(alice.peer(endpoint(1)).unwrap())
        .await
        .unwrap();
    assert_eq!(connection.capabilities().local_bitrate_bps, Some(1_000_000));
    let send = connection.request_bounded(b"request", 4);
    let receive = async {
        let job = jobs.recv().await.unwrap();
        // Host operations run on independent node runtimes, so advancing only
        // the test runtime's virtual clock would not test their actual timers.
        tokio::time::sleep(Duration::from_secs(11)).await;
        bob.respond(&job, b"late").await.unwrap();
    };
    let (response, ()) = tokio::join!(send, receive);
    assert_eq!(response.unwrap(), b"late");
    connection.close().await.unwrap();
    no_links(&nodes.owners[0]).await;
    nodes.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enrollment_authenticates_identity_and_large_frames_without_group_membership() {
    let mut nodes = wire::Nodes::open().await;
    let alice = Arc::clone(&nodes.enrollment_networks[0]);
    let bob = Arc::clone(&nodes.enrollment_networks[1]);
    // The fourth host is deliberately absent from the group sync allow list.
    let stranger = Arc::clone(&nodes.enrollment_networks[3]);
    let mut bob_jobs = nodes.enrollment_jobs[1].take().unwrap();
    let mut alice_jobs = nodes.enrollment_jobs[0].take().unwrap();
    let alice_id = alice.local_identity();
    let bob_id = bob.local_identity();
    let stranger_id = stranger.local_identity();

    let request: Vec<_> = (0_u8..=255).cycle().take(65_536 + 128).collect();
    let response = vec![41; enrollment::MAX_FRAME_BYTES];
    let send = alice.exchange(bob_id, &request);
    let receive = async {
        let job = bob_jobs.recv().await.unwrap();
        assert_eq!(job.requester(), alice_id);
        assert_eq!(job.data(), request);
        // A job cannot accidentally be answered through the sync destination.
        assert_eq!(
            nodes.networks[1].respond(&job, b"wrong channel").await,
            Err(NetworkError::UnknownPeer)
        );
        assert_eq!(
            bob.respond(&job, &vec![0; enrollment::MAX_FRAME_BYTES + 1])
                .await,
            Err(NetworkError::FrameTooLarge)
        );
        bob.respond(&job, &response).await.unwrap();
    };
    let (received, ()) = tokio::join!(send, receive);
    assert_eq!(received.unwrap(), response);
    assert!(nodes.state.resources.load(Ordering::SeqCst) >= 2);
    assert!(nodes.state.resource_proofs.load(Ordering::SeqCst) >= 2);

    // Carrier identity is not invitation authorization. The application receives
    // the actual stranger's identity, even when its payload claims to be Alice.
    let claimed = alice_id.into_bytes();
    let send = stranger.exchange(bob_id, &claimed);
    let receive = async {
        let job = bob_jobs.recv().await.unwrap();
        assert_eq!(job.requester(), stranger_id);
        assert_eq!(job.data(), claimed);
        bob.respond(&job, b"unauthorized invitation").await.unwrap();
    };
    let (received, ()) = tokio::join!(send, receive);
    assert_eq!(received.unwrap(), b"unauthorized invitation");
    assert_eq!(nodes.state.jobs[1].load(Ordering::SeqCst), 0);

    // Both hosts may wait on outgoing requests while independently replying.
    let alice_send = alice.exchange(bob_id, b"from alice");
    let bob_send = bob.exchange(alice_id, b"from bob");
    let alice_receive = async {
        let job = alice_jobs.recv().await.unwrap();
        assert_eq!(job.requester(), bob_id);
        alice.respond(&job, b"alice reply").await.unwrap();
    };
    let bob_receive = async {
        let job = bob_jobs.recv().await.unwrap();
        assert_eq!(job.requester(), alice_id);
        bob.respond(&job, b"bob reply").await.unwrap();
    };
    let (a, b, (), ()) = tokio::join!(alice_send, bob_send, alice_receive, bob_receive);
    assert_eq!(a.unwrap(), b"bob reply");
    assert_eq!(b.unwrap(), b"alice reply");
    no_links(&nodes.owners[0]).await;
    no_links(&nodes.owners[1]).await;
    nodes.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enrollment_bounds_and_cancellation_preserve_the_shared_host() {
    let mut nodes = wire::Nodes::open().await;
    let alice = Arc::clone(&nodes.enrollment_networks[0]);
    let bob_id = nodes.enrollment_networks[1].local_identity();
    let mut bob_jobs = nodes.enrollment_jobs[1].take().unwrap();
    assert!(matches!(
        enrollment::Network::new(
            nodes.owners[0].client(),
            prns_groups_reticulum::destination_for(alice.local_identity()).unwrap(),
            wire::enrollment_limits(),
        ),
        Err(NetworkError::IdentityMismatch)
    ));
    let before = nodes.state.frames[0].load(Ordering::SeqCst);
    assert_eq!(
        alice
            .exchange(bob_id, &vec![0; enrollment::MAX_FRAME_BYTES + 1])
            .await,
        Err(NetworkError::FrameTooLarge)
    );
    assert_eq!(
        alice.exchange(alice.local_identity(), b"self").await,
        Err(NetworkError::InvalidPeer)
    );
    assert_eq!(nodes.state.frames[0].load(Ordering::SeqCst), before);
    assert!(bob_jobs.try_recv().is_err());

    let waiting = tokio::spawn(async move { alice.exchange(bob_id, b"pending enrollment").await });
    let job = tokio::time::timeout(Duration::from_secs(5), bob_jobs.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.data(), b"pending enrollment");
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    no_links(&nodes.owners[0]).await;

    // Path discovery and Link establishment alone do not identify the requester.
    // A raw request without identify reaches no application queue and expires.
    let destination = enrollment::destination_for(bob_id).unwrap();
    nodes.owners[0]
        .client()
        .protocol_operation(move |protocol| async move {
            let link = protocol
                .establish_link_with_rtt(personal_rns::wire::DestinationHash::new(
                    destination.into_bytes(),
                ))
                .await
                .unwrap();
            let response = protocol
                .request_with_options(
                    link.link_id,
                    personal_rns::routing::request_handlers::RequestPathHash::of(
                        enrollment::REQUEST_PATH,
                    ),
                    &[0xc4, 1, 42],
                    personal_rns::runtime::RequestOptions {
                        response_timeout: personal_rns::engine::RequestResponseTimeout::Exact(
                            personal_rns::units::DurationMillis(250),
                        ),
                        maximum_response_bytes: personal_rns::units::ByteLimit::Maximum(256),
                    },
                )
                .await;
            assert!(response.is_err());
            assert!(protocol.close_link(link.link_id));
        })
        .await
        .unwrap();
    assert!(bob_jobs.try_recv().is_err());
    no_links(&nodes.owners[0]).await;

    // The caller's deadline covers path discovery as well as request delivery.
    nodes.state.set_online(2, false);
    let bounded = enrollment::Network::new(
        nodes.owners[0].client(),
        enrollment::destination_for(nodes.enrollment_networks[0].local_identity()).unwrap(),
        enrollment::Limits {
            exchange_timeout: Duration::from_millis(100),
            ..wire::enrollment_limits()
        },
    )
    .unwrap()
    .with_sync_policy(prns_groups_core::sync::SyncPolicy {
        min_exchange_millis: 100,
        max_exchange_millis: 100,
        unknown_rtt_millis: 10,
        ..Default::default()
    })
    .unwrap();
    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        bounded.exchange(
            nodes.enrollment_networks[2].local_identity(),
            b"unavailable",
        ),
    )
    .await
    .expect("unavailable enrollment is bounded by the caller deadline");
    assert_eq!(outcome, Err(NetworkError::Timeout));
    assert!(
        nodes.owners[0]
            .client()
            .snapshot()
            .await
            .unwrap()
            .runtime
            .running
    );
    nodes.stop().await;
}

#[cfg(feature = "contact-card")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_card_bootstraps_identified_enrollment_without_lxmf_or_announce() {
    use prns_contact_card::{decode_uri, encode_identity, encode_uri, verify};
    use prns_core::identity::PrivateIdentityMaterial;
    use prns_groups_reticulum::contact_card::{enrollment_recipient, enrollment_service_name_hash};

    let mut nodes = wire::Nodes::open_scoped().await;
    let alice = Arc::clone(&nodes.enrollment_networks[0]);
    let bob = Arc::clone(&nodes.enrollment_networks[1]);
    let mut jobs = nodes.enrollment_jobs[1].take().unwrap();
    // Same deterministic identity installed by this fixture's dedicated service.
    let bob_identity = PrivateIdentityMaterial::from_bytes([0x61; 64]);
    let mut signed = [0; prns_contact_card::MAX_CARD_BYTES];
    let signed_len = encode_identity(
        &bob_identity,
        &enrollment_service_name_hash(),
        "Bob",
        &mut signed,
    )
    .unwrap();
    let mut uri = [0; prns_contact_card::MAX_URI_BYTES];
    let uri_len = encode_uri(&signed[..signed_len], &mut uri).unwrap();
    let mut received = [0; prns_contact_card::MAX_CARD_BYTES];
    let received_len =
        decode_uri(std::str::from_utf8(&uri[..uri_len]).unwrap(), &mut received).unwrap();
    let verified = verify(&received[..received_len]).unwrap();
    let selected = enrollment_recipient(&verified).unwrap();
    assert_eq!(selected, bob.local_identity());
    assert_eq!(
        verified.destination_hash(),
        enrollment::destination_for(selected).unwrap().as_bytes()
    );
    assert!(jobs.try_recv().is_err(), "offline import sends nothing");
    assert_eq!(nodes.state.unsolicited_announces.load(Ordering::SeqCst), 0);
    assert_eq!(nodes.state.path_responses.load(Ordering::SeqCst), 0);

    // An explicit consumer action starts ordinary identified enrollment traffic.
    // This tests bootstrap routing/authentication, not membership acceptance.
    let send = alice.exchange(selected, b"explicit first contact");
    let receive = async {
        let job = jobs.recv().await.unwrap();
        assert_eq!(job.requester(), alice.local_identity());
        bob.respond(&job, b"authenticated enrollment reply")
            .await
            .unwrap();
    };
    let (reply, ()) = tokio::join!(send, receive);
    assert_eq!(reply.unwrap(), b"authenticated enrollment reply");
    assert!(nodes.state.path_responses.load(Ordering::SeqCst) > 0);
    assert_eq!(nodes.state.unsolicited_announces.load(Ordering::SeqCst), 0);
    nodes.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_path_discovery_does_not_open_a_link_publication_window() {
    use personal_rns::runtime::{Diagnostic, PrnsEvent};
    use prns_host_runtime::service_scope::HostServiceScope;
    let nodes = wire::Nodes::open_scoped().await;
    let scope = HostServiceScope::new(
        nodes.owners[0].client(),
        nodes.scopes[0].identity_hash(),
        nodes.owners[0].client().destination_hashes().unwrap(),
        4,
        1,
    )
    .unwrap();
    let network = enrollment::Network::scoped(
        scope.clone(),
        enrollment::destination_for(scope.identity_hash()).unwrap(),
        enrollment::Limits {
            exchange_timeout: Duration::from_secs(1),
            ..wire::enrollment_limits()
        },
    )
    .unwrap()
    .with_sync_policy(prns_groups_core::sync::SyncPolicy {
        min_exchange_millis: 1000,
        max_exchange_millis: 1000,
        unknown_rtt_millis: 10,
        ..Default::default()
    })
    .unwrap();
    let before = nodes.state.frames[0].load(Ordering::SeqCst);
    let sending = tokio::spawn(async move {
        network
            .exchange(prns_host::IdentityHash::new([0x77; 16]), b"no route")
            .await
    });
    tokio::time::timeout(Duration::from_millis(500), async {
        while nodes.state.frames[0].load(Ordering::SeqCst) == before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual offline path request was emitted");
    assert!(!sending.is_finished());
    assert_eq!(
        nodes.owners[0]
            .client()
            .snapshot()
            .await
            .unwrap()
            .active_link_count,
        0
    );
    // Host-wide closure callbacks can belong to unrelated inbound services.
    // During discovery no Link has been dispatched and no early-close evidence
    // for this attempt needs retention, even with the smallest Link selection.
    for byte in [0xA1, 0xA2] {
        scope.on_event_ref(&PrnsEvent::Diagnostic(Diagnostic::LinkClosed {
            link_id: personal_rns::routing::links::LinkId::new([byte; 16]),
            reason: personal_rns::engine::LinkClosedReason::LocallyClosed,
        }));
    }
    assert_eq!(scope.check_open(), Ok(()));
    assert_eq!(sending.await.unwrap(), Err(NetworkError::Timeout));
    scope.fence();
    tokio::time::timeout(Duration::from_secs(2), scope.wait_drained())
        .await
        .unwrap();
    assert!(scope.outgoing_links().unwrap().is_empty());
    assert!(
        nodes.owners[0]
            .client()
            .snapshot()
            .await
            .unwrap()
            .runtime
            .running
    );
    nodes.stop().await;
}
