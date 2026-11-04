//! New durable sends and their receipts do not wait for a reciprocal poll.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod support;
use support::PublicationClient;

use prns_groups::{fixture, run, Client, RunnerControl, RunnerOptions};
use prns_groups_core::sync::SyncPolicy;
use prns_groups_reticulum::{Network, SyncBudget};
use prns_groups_service::{GroupId, Timestamp};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use support::{clock, TestClock, Worker, NOW};
use tokio::sync::{mpsc, watch};

async fn wait_receipt(client: &Client, group: GroupId, expected: usize) {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let page = client.messages(group, None, 100, clock()).await.unwrap();
            if page.messages.len() == expected
                && page.messages.iter().all(|m| m.received_count == 1)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("push and same-Link receipt must beat the five-minute poll floor");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_send_and_receipt_complete_while_reciprocal_poll_is_parked() {
    let mut nodes = support::wire::Nodes::open().await;
    let identities: Vec<_> = nodes.owners[..2]
        .iter()
        .map(|node| node.client().identity_hash().unwrap())
        .collect();
    let fixtures = fixture::generate("Prompt push", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..2)
        .map(|i| directory.path().join(format!("peer-{i}.redb")))
        .collect();
    let mut workers: Vec<_> = fixtures
        .iter()
        .enumerate()
        .map(|(i, fixture)| Worker::open(paths[i].clone(), fixture.clone(), identities[i]))
        .collect();
    let initial = workers[0].client.network_config().await.unwrap();
    let group = initial.group;
    let original_scope = prns_groups::OriginalScope::Control {
        control: initial.control,
        incarnation: initial.incarnation,
    };
    let options = RunnerOptions {
        // Even an aggressive policy cannot shorten this caller timing floor.
        poll_interval: Duration::from_secs(300),
        retry_initial: Duration::from_secs(1),
        retry_max: Duration::from_secs(300),
        budget: SyncBudget {
            max_requests: 4,
            max_envelopes: 1,
            max_bytes: 16_384,
        },
    };
    let mut runners = Vec::new();
    let mut controls = Vec::new();
    let mut networks = Vec::new();
    let mut destinations = Vec::new();
    let mut forwarders = Vec::new();
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
                ..SyncPolicy::default()
            })
            .unwrap(),
        );
        let (control, stop) = watch::channel(RunnerControl::Running);
        let (sender, jobs) = mpsc::channel(16);
        let destination = Arc::new(std::sync::Mutex::new(sender));
        let current_destination = destination.clone();
        let mut source = nodes.jobs[i].take().unwrap();
        forwarders.push(tokio::spawn(async move {
            while let Some(job) = source.recv().await {
                let sender = current_destination.lock().unwrap().clone();
                let _ = sender.send(job).await;
            }
        }));
        destinations.push(destination);
        runners.push(tokio::spawn(run(
            worker.client.clone(),
            network.clone(),
            jobs,
            options,
            Arc::new(TestClock),
            stop,
        )));
        controls.push(control);
        networks.push(network);
    }
    // Let each startup snapshot finish. Later send/receipt observations must not
    // require any new request arriving at the author from the receiver.
    tokio::time::timeout(Duration::from_secs(8), async {
        while nodes.state.jobs[0].load(Ordering::SeqCst) == 0
            || nodes.state.jobs[1].load(Ordering::SeqCst) == 0
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let author_incoming = nodes.state.jobs[0].load(Ordering::SeqCst);
    let request = support::text_request(
        group,
        original_scope,
        prns_groups_core::retry::RequestToken::new(NOW, [3; 20])
            .unwrap()
            .into_bytes(),
        "This must not wait five minutes".into(),
        clock(),
    );
    // The caller may disappear after storage admission. Owner-side commit wake
    // must still discover and push the exact durable record.
    drop(workers[0].client.admit_request(request.clone()).unwrap());
    wait_receipt(&workers[0].client, group, 1).await;
    assert_eq!(
        nodes.state.jobs[0].load(Ordering::SeqCst),
        author_incoming,
        "receipt required a reciprocal receiver request instead of the submitting Link"
    );
    let first = workers[0]
        .client
        .messages(group, None, 100, clock())
        .await
        .unwrap()
        .messages[0]
        .id;
    assert_eq!(
        workers[0].client.publish_request(request).await.unwrap(),
        first
    );
    let received = workers[1]
        .client
        .messages(group, None, 100, clock())
        .await
        .unwrap();
    assert_eq!(received.messages.len(), 1);
    assert_eq!(received.messages[0].id, first);

    for control in &controls {
        control.send(RunnerControl::Stop).unwrap();
    }
    for runner in runners {
        runner.await.unwrap().unwrap();
    }
    // A newly queued durable record survives owner closure. Restart reconstructs
    // it from the ledger without another application send or replacement ID.
    let queued = workers[0]
        .client
        .publish_request(support::text_request(
            group,
            original_scope,
            prns_groups_core::retry::RequestToken::new(NOW, [4; 20])
                .unwrap()
                .into_bytes(),
            "Restarted outbox".into(),
            clock(),
        ))
        .await
        .unwrap();
    for worker in workers.drain(..) {
        worker.stop();
    }
    workers = fixtures
        .iter()
        .enumerate()
        .map(|(i, fixture)| Worker::open(paths[i].clone(), fixture.clone(), identities[i]))
        .collect();
    controls.clear();
    let mut restarted = Vec::new();
    for (i, worker) in workers.iter().enumerate() {
        let (sender, jobs) = mpsc::channel(16);
        *destinations[i].lock().unwrap() = sender;
        let (control, stop) = watch::channel(RunnerControl::Running);
        controls.push(control);
        restarted.push(tokio::spawn(run(
            worker.client.clone(),
            networks[i].clone(),
            jobs,
            options,
            Arc::new(TestClock),
            stop,
        )));
    }
    wait_receipt(&workers[0].client, group, 2).await;
    let reopened = workers[0]
        .client
        .messages(group, None, 100, clock())
        .await
        .unwrap();
    assert_eq!(reopened.messages.len(), 2);
    assert!(reopened
        .messages
        .iter()
        .any(|m| m.id == queued && m.received_count == 1));
    assert!(reopened
        .messages
        .iter()
        .any(|m| m.id == first && m.received_count == 1));
    let received = workers[1]
        .client
        .messages(group, None, 100, clock())
        .await
        .unwrap();
    assert_eq!(received.messages.len(), 2);
    assert!(received.messages.iter().any(|m| m.id == queued));
    for control in &controls {
        control.send(RunnerControl::Stop).unwrap();
    }
    for runner in restarted {
        runner.await.unwrap().unwrap();
    }
    for forwarder in forwarders {
        forwarder.abort();
    }
    nodes.stop().await;
    for worker in workers {
        worker.stop();
    }
}
