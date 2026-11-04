//! Exercise the public runner against independent real Reticulum endpoints.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod support;
use support::PublicationClient;

use prns_groups::{fixture, run, RunnerControl, RunnerOptions};
use prns_groups_core::sync::SyncPolicy;
use prns_groups_reticulum::{Network, SyncBudget};
use prns_groups_service::Timestamp;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use support::{clock, TestClock, Worker, NOW};
use tokio::sync::{mpsc, watch};

struct Evidence(Mutex<Vec<String>>);
static EVIDENCE: Evidence = Evidence(Mutex::new(Vec::new()));
impl log::Log for Evidence {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == "prns.groups.schedule"
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let text = record.args().to_string();
            eprintln!("{text}");
            let mut events = self.0.lock().unwrap();
            assert!(events.len() < 1026);
            events.push(text);
        }
    }
    fn flush(&self) {}
}

fn field<'a>(event: &'a str, key: &str) -> Option<&'a str> {
    event.split_whitespace().find_map(|item| {
        item.split_once('=')
            .filter(|(name, _)| *name == key)
            .map(|(_, value)| value)
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_durable_work_accelerates_a_failed_public_runner_without_changing_record_identity() {
    log::set_logger(&EVIDENCE).unwrap();
    log::set_max_level(log::LevelFilter::Info);
    prns_groups::enable_metadata_diagnostics();
    let mut nodes = support::wire::Nodes::open().await;
    let identities: Vec<_> = nodes.owners[..2]
        .iter()
        .map(|owner| owner.client().identity_hash().unwrap())
        .collect();
    let fixtures = fixture::generate("Bounded retry", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let workers: Vec<_> = fixtures
        .iter()
        .enumerate()
        .map(|(index, fixture)| {
            Worker::open(
                directory.path().join(format!("peer-{index}.redb")),
                fixture.clone(),
                identities[index],
            )
        })
        .collect();
    let initial = workers[0].client.network_config().await.unwrap();
    let group = initial.group;
    let original_scope = prns_groups::OriginalScope::Control {
        control: initial.control,
        incarnation: initial.incarnation,
    };
    let policy = SyncPolicy {
        min_exchange_millis: 100,
        max_exchange_millis: 1_000,
        unknown_rtt_millis: 10,
        fast_poll_millis: 100,
        constrained_poll_millis: 100,
        max_fast_idle_millis: 30_000,
        max_idle_millis: 30_000,
        ..SyncPolicy::default()
    };
    let options = RunnerOptions {
        poll_interval: Duration::from_secs(300),
        retry_initial: Duration::from_millis(100),
        retry_max: Duration::from_secs(300),
        budget: SyncBudget {
            max_requests: 4,
            max_envelopes: 1,
            max_bytes: 16_384,
        },
    };
    let mut networks = Vec::new();
    for (index, worker) in workers.iter().enumerate() {
        let config = worker.client.network_config().await.unwrap();
        networks.push(Arc::new(
            Network::new(
                nodes.owners[index].client(),
                nodes.owners[index].client().destination_hashes().unwrap()[0],
                config.peers,
                prns_groups_reticulum::Limits {
                    exchange_timeout: Duration::from_millis(100),
                    ..support::wire::limits()
                },
            )
            .unwrap()
            .with_sync_policy(policy)
            .unwrap(),
        ));
    }
    nodes.state.set_online(1, false);
    // Keep the author's inbound service unavailable. Receiver polling cannot
    // fetch the record or reconcile its receipt; the accelerated outbound push
    // and its same-Link receipt must do the work.
    let mut reverse_jobs = nodes.jobs[0].take().unwrap();
    let discard_reverse = tokio::spawn(async move { while reverse_jobs.recv().await.is_some() {} });
    let (_author_ingress, author_jobs) = mpsc::channel(16);
    let (author_stop, author_control) = watch::channel(RunnerControl::Running);
    let author = tokio::spawn(run(
        workers[0].client.clone(),
        networks[0].clone(),
        author_jobs,
        options,
        Arc::new(TestClock),
        author_control,
    ));
    // Only the author runner exists, so this recorded failure cannot belong to
    // the receiver. Wait for an actual long failed wait, not an assumed sleep.
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let backed_off = EVIDENCE.0.lock().unwrap().iter().any(|event| {
                field(event, "category") == Some("completed")
                    && field(event, "activity") == Some("failed")
                    && field(event, "due_in_ms")
                        .and_then(|value| value.parse::<u64>().ok())
                        .is_some_and(|value| value >= 12_000)
            });
            if backed_off {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("author must enter a measured failed wait");
    let event_start = EVIDENCE.0.lock().unwrap().len();
    let request = support::text_request(
        group,
        original_scope,
        prns_groups_core::retry::RequestToken::new(NOW, [91; 20])
            .unwrap()
            .into_bytes(),
        "Fresh work wakes the backed-off sender".into(),
        clock(),
    );
    let record = workers[0]
        .client
        .publish_request(request.clone())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if EVIDENCE.0.lock().unwrap()[event_start..]
                .iter()
                .any(|event| {
                    field(event, "category") == Some("fresh_work")
                        && field(event, "accelerated") == Some("true")
                        && field(event, "failures")
                            .and_then(|value| value.parse::<u32>().ok())
                            .is_some_and(|value| value >= 7)
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("durable new work must shorten the failed wait, preserving its exponent");
    nodes.state.set_online(1, true);
    let recovery = Instant::now();
    let (receiver_stop, receiver_control) = watch::channel(RunnerControl::Running);
    let receiver = tokio::spawn(run(
        workers[1].client.clone(),
        networks[1].clone(),
        nodes.jobs[1].take().unwrap(),
        options,
        Arc::new(TestClock),
        receiver_control,
    ));
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let page = workers[0]
                .client
                .messages(group, None, 10, clock())
                .await
                .unwrap();
            if page
                .messages
                .iter()
                .any(|message| message.id == record && message.received_count == 1)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("receipt must arrive before the previously measured failed deadline");
    eprintln!(
        "fresh-send recovery receipt elapsed_ms={}",
        recovery.elapsed().as_millis()
    );
    assert_eq!(
        workers[0].client.publish_request(request).await.unwrap(),
        record
    );
    let received = workers[1]
        .client
        .messages(group, None, 10, clock())
        .await
        .unwrap();
    assert_eq!(received.messages.len(), 1);
    assert_eq!(received.messages[0].id, record);
    author_stop.send(RunnerControl::Stop).unwrap();
    receiver_stop.send(RunnerControl::Stop).unwrap();
    author.await.unwrap().unwrap();
    receiver.await.unwrap().unwrap();
    discard_reverse.abort();
    let (watermark, healthy) = prns_groups::metadata_schedule_snapshot();
    assert!(healthy);
    {
        let events = EVIDENCE.0.lock().unwrap();
        assert_eq!(watermark as usize, events.len());
        for (index, event) in events.iter().enumerate() {
            assert_eq!(
                field(event, "sequence").unwrap().parse::<usize>().unwrap(),
                index + 1
            );
        }
    }
    nodes.stop().await;
    for worker in workers {
        worker.stop();
    }
}
