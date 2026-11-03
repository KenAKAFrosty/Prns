//! The same database lane handles queries and incoming/outgoing sync phases.
//! Independent real Reticulum nodes receive bytes only through their carrier.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod support;
use support::PublicationClient;

use prns_groups::{fixture, run, ApiError, Client, RunnerControl, RunnerOptions};
use prns_groups_core::sync::SyncPolicy;
use prns_groups_reticulum::{Network, SyncBudget};
use prns_groups_service::{GroupId, Timestamp};
use std::{sync::Arc, time::Duration};
use support::{clock, TestClock, Worker, NOW};
use tokio::sync::watch;

fn options() -> RunnerOptions {
    RunnerOptions {
        poll_interval: Duration::from_millis(100),
        retry_initial: Duration::from_millis(100),
        retry_max: Duration::from_millis(500),
        budget: SyncBudget {
            max_requests: 4,
            max_envelopes: 1,
            max_bytes: 16_384,
        },
    }
}
fn send(
    group: GroupId,
    original_scope: prns_groups::OriginalScope,
    nonce: u8,
    text: &str,
) -> support::Prepared {
    support::text_request(
        group,
        original_scope,
        prns_groups_core::retry::RequestToken::new(NOW, [nonce; 20])
            .unwrap()
            .into_bytes(),
        text.into(),
        clock(),
    )
}
async fn wait_history(client: &Client, group: GroupId, expected: usize, receipts: Option<u64>) {
    let mut last_page = None;
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let page = match client.messages(group, None, 100, clock()).await {
                Ok(page) => page,
                Err(ApiError::Busy) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                Err(error) => panic!("history query: {error:?}"),
            };
            if page.messages.len() == expected
                && receipts.is_none_or(|count| {
                    page.messages
                        .iter()
                        .filter(|message| message.outgoing)
                        .all(|message| message.received_count == count)
                })
            {
                return;
            }
            last_page = Some(page);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "history wait {expected}/{receipts:?}: {last_page:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_runners_relay_offline_history_and_recover_durable_sends() {
    let mut nodes = support::wire::Nodes::open().await;
    let identities: Vec<_> = nodes.owners[..3]
        .iter()
        .map(|owner| owner.client().identity_hash().unwrap())
        .collect();
    let fixtures = fixture::generate("Development group", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let workers: Vec<_> = fixtures
        .iter()
        .enumerate()
        .map(|(i, fixture)| {
            Worker::open(
                directory.path().join(format!("peer-{i}.redb")),
                fixture.clone(),
                identities[i],
            )
        })
        .collect();
    let initial = workers[0].client.network_config().await.unwrap();
    let group = initial.group;
    let original_scope = prns_groups::OriginalScope::Control {
        control: initial.control,
        incarnation: initial.incarnation,
    };
    let bob_scope = workers[1].client.group_scope(group).await.unwrap();
    let mut controls = Vec::new();
    let mut runners = Vec::new();
    nodes.state.set_online(2, false);
    for (i, worker) in workers.iter().enumerate() {
        let config = worker.client.network_config().await.unwrap();
        let network = Arc::new(
            Network::new(
                nodes.owners[i].client(),
                nodes.owners[i].client().destination_hashes().unwrap()[0],
                config.peers,
                support::wire::limits(),
            )
            .unwrap()
            .with_sync_policy(SyncPolicy {
                min_exchange_millis: 500,
                max_exchange_millis: 2_000,
                unknown_rtt_millis: 10,
                fast_poll_millis: 100,
                constrained_poll_millis: 100,
                max_idle_millis: 500,
                ..SyncPolicy::default()
            })
            .unwrap(),
        );
        let (control, stop) = watch::channel(RunnerControl::Running);
        controls.push(control);
        runners.push(tokio::spawn(run(
            worker.client.clone(),
            network,
            nodes.jobs[i].take().unwrap(),
            options(),
            Arc::new(TestClock),
            stop,
        )));
    }
    // Both sides initiate pulls automatically, while serving reciprocal pulls.
    let (alice, bob) = tokio::join!(
        workers[0]
            .client
            .publish_request(send(group, original_scope, 1, "Alice's text")),
        workers[1]
            .client
            .publish_request(send(group, bob_scope, 2, "Bob's text"))
    );
    let alice = alice.unwrap();
    let bob = bob.unwrap();
    // Simulate the bounded shared lane being occupied by other protocols. The
    // group runner must back off rather than ending the host generation.
    workers[0]
        .busy_admissions
        .store(12, std::sync::atomic::Ordering::SeqCst);
    workers[1]
        .busy_admissions
        .store(12, std::sync::atomic::Ordering::SeqCst);
    wait_history(&workers[0].client, group, 2, Some(1)).await;
    wait_history(&workers[1].client, group, 2, Some(1)).await;
    let repeated = workers[0]
        .client
        .publish_request(send(group, original_scope, 1, "Alice's text"))
        .await
        .unwrap();
    assert_eq!(repeated, alice);
    assert!(matches!(
        workers[0]
            .client
            .publish_request(send(group, original_scope, 1, "changed"))
            .await,
        Err(prns_groups_core::ApiError::Rejected {
            reason: prns_groups_core::RejectionReason::RequestConflict
        })
    ));
    nodes.state.set_online(0, false);
    nodes.state.set_online(2, true);
    wait_history(&workers[2].client, group, 2, None).await;
    assert_eq!(
        workers[2]
            .client
            .messages(group, None, 100, clock())
            .await
            .unwrap()
            .messages
            .iter()
            .map(|m| m.id)
            .collect::<std::collections::BTreeSet<_>>(),
        [alice, bob].into_iter().collect()
    );
    nodes.state.set_online(0, true);
    wait_history(&workers[0].client, group, 2, Some(2)).await;
    wait_history(&workers[1].client, group, 2, Some(2)).await;
    let first = workers[0]
        .client
        .messages(group, None, 1, clock())
        .await
        .unwrap();
    let second = workers[0]
        .client
        .messages(group, first.next_before, 1, clock())
        .await
        .unwrap();
    assert_ne!(first.messages[0].id, second.messages[0].id);
    assert_eq!(second.next_before, None);
    for control in controls {
        control.send(RunnerControl::Stop).unwrap();
    }
    for runner in runners {
        runner.await.unwrap().unwrap();
    }
    let before = workers[0]
        .client
        .messages(group, None, 100, clock())
        .await
        .unwrap();
    for worker in workers {
        worker.stop();
    }
    // Actual database close/reopen with the original enrollment must keep the
    // advanced ratchets, outbox, request IDs, history and receipt evidence.
    let reopened = Worker::open(
        directory.path().join("peer-0.redb"),
        fixtures[0].clone(),
        identities[0],
    );
    assert_eq!(
        reopened
            .client
            .messages(group, None, 100, clock())
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        reopened
            .client
            .publish_request(send(group, original_scope, 1, "Alice's text"))
            .await
            .unwrap(),
        alice
    );
    reopened.stop();
    for owner in &nodes.owners {
        assert!(owner.client().identity_hash().is_ok());
    }
    nodes.stop().await;
}
