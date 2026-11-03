//! Real routed Links and Resources through a slow middle segment, with no bypass.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "support/slow_wire.rs"]
mod slow_wire;
mod support;
use support::PublicationClient;

use prns_groups::{fixture, run_multi, ApiError, Client, Clock, SystemClock};
use prns_groups_service::{GroupId, Timestamp};
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use support::Worker;
use tokio::sync::watch;

struct GroupDiagnostics;
static DIAGNOSTIC_START: OnceLock<Instant> = OnceLock::new();
static GROUP_DIAGNOSTICS: GroupDiagnostics = GroupDiagnostics;
impl log::Log for GroupDiagnostics {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == "prns.groups.diagnostics"
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "group diagnostic t={:.3} {}",
                DIAGNOSTIC_START
                    .get_or_init(Instant::now)
                    .elapsed()
                    .as_secs_f64(),
                record.args(),
            );
        }
    }

    fn flush(&self) {}
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
fn send(
    group: GroupId,
    original_scope: prns_groups::OriginalScope,
    token: prns_groups::RequestId,
    text: &str,
) -> support::Prepared {
    support::text_request(
        group,
        original_scope,
        token.0,
        text.into(),
        SystemClock.now(),
    )
}
async fn has_message(client: &Client, group: GroupId, text: &str, receipts: Option<u64>) -> bool {
    match client.messages(group, None, 100, SystemClock.now()).await {
        Ok(page) => page.messages.iter().any(|message| {
            message.text == text && receipts.is_none_or(|count| message.received_count == count)
        }),
        Err(ApiError::Busy) => false,
        Err(error) => panic!("history failed: {error:?}"),
    }
}
async fn wait_message(client: &Client, group: GroupId, text: &str, receipts: Option<u64>) {
    tokio::time::timeout(Duration::from_secs(180), async {
        while !has_message(client, group, text, receipts).await {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("routed history and receipt converge");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_routed_sync_keeps_local_work_live_and_stops_without_losing_the_outbox() {
    let mut nodes = slow_wire::Nodes::open(Duration::from_millis(250)).await;
    let identities: Vec<_> = nodes.owners[..slow_wire::MEMBERS]
        .iter()
        .map(|owner| owner.client().identity_hash().unwrap())
        .collect();
    let fixtures = fixture::generate("Slow routed group", &identities, Timestamp(now())).unwrap();
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
    // Maximal text forces real Resource fragmentation. There is no shortcut that
    // can inject the application payload directly into another endpoint's store.
    let text = "s".repeat(prns_group_text::MAX_TEXT_BYTES);
    let token = prns_groups::create_request_id().unwrap();
    let record = workers[0]
        .client
        .publish_request(send(group, original_scope, token, &text))
        .await
        .unwrap();
    let mut stops = Vec::new();
    let mut tasks = Vec::new();
    let started = Instant::now();
    for (index, worker) in workers.iter().enumerate() {
        let (stop, receiver) = watch::channel(false);
        stops.push(stop);
        tasks.push(tokio::spawn(run_multi(
            worker.client.clone(),
            nodes.owners[index].client(),
            prns_groups_reticulum::destination_for(identities[index]).unwrap(),
            prns_groups_reticulum::enrollment::destination_for(identities[index]).unwrap(),
            nodes.jobs[index].take().unwrap(),
            nodes.enrollment_jobs[index].take().unwrap(),
            slow_wire::limits(),
            slow_wire::enrollment_limits(),
            receiver,
        )));
    }
    // Fast-side work can finish while the independent slow carrier is occupied.
    tokio::time::timeout(
        Duration::from_secs(10),
        wait_message(&workers[2].client, group, &text, None),
    )
    .await
    .expect("local member is not starved by the paced middle hop");
    eprintln!("local member received after {:?}", started.elapsed());
    wait_message(&workers[1].client, group, &text, None).await;
    eprintln!("routed member received after {:?}", started.elapsed());
    let config = workers[0].client.network_config().await.unwrap();
    let remote = *config
        .peers
        .iter()
        .find(|peer| peer.identity == identities[1])
        .unwrap();
    let network = prns_groups_reticulum::Network::new(
        nodes.owners[0].client(),
        prns_groups_reticulum::destination_for(identities[0]).unwrap(),
        config.peers,
        slow_wire::limits(),
    )
    .unwrap();
    let path = network.path_capabilities(&remote).await.unwrap();
    assert_eq!(
        path.hops,
        Some(3),
        "two actual transport nodes are on the route"
    );
    assert_eq!(path.local_bitrate_bps, Some(1_000_000));
    assert_eq!(
        network.sync_policy().tuning(path).max_envelopes,
        1,
        "the fast first hop must not classify the routed path as fast"
    );
    assert!(
        started.elapsed() > Duration::from_secs(10),
        "test must exercise the former wall-time limit"
    );
    assert!(nodes.state.resources.load(Ordering::SeqCst) > 0);
    assert!(nodes
        .state
        .frames
        .iter()
        .all(|count| count.load(Ordering::SeqCst) > 0));
    wait_message(&workers[0].client, group, &text, Some(2)).await;
    eprintln!(
        "both device receipts reconciled after {:?}",
        started.elapsed()
    );

    // Cutting the only middle carrier must strand a new record at the source,
    // while local serving remains available. Stopping cancels network waits but
    // cannot undo the already admitted durable send.
    nodes.state.middle_online.store(false, Ordering::SeqCst);
    let offline_token = prns_groups::create_request_id().unwrap();
    let offline = "queued behind the disconnected middle segment";
    let offline_record = workers[0]
        .client
        .publish_request(send(group, original_scope, offline_token, offline))
        .await
        .unwrap();
    wait_message(&workers[2].client, group, offline, None).await;
    assert!(!has_message(&workers[1].client, group, offline, None).await);
    let stopping = Instant::now();
    for stop in stops {
        stop.send(true).unwrap();
    }
    for task in tasks {
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("stop interrupts a route wait")
            .unwrap()
            .unwrap();
    }
    assert!(stopping.elapsed() < Duration::from_secs(2));
    for worker in workers {
        worker.stop();
    }
    let reopened = Worker::open(
        directory.path().join("peer-0.redb"),
        fixtures[0].clone(),
        identities[0],
    );
    assert_eq!(
        reopened
            .client
            .publish_request(send(group, original_scope, token, &text))
            .await
            .unwrap(),
        record
    );
    assert_eq!(
        reopened
            .client
            .publish_request(send(group, original_scope, offline_token, offline))
            .await
            .unwrap(),
        offline_record
    );
    assert!(has_message(&reopened.client, group, offline, None).await);
    reopened.stop();
    nodes.stop().await;
}

async fn exact_message(
    client: &Client,
    group: GroupId,
    text: &str,
) -> Result<Option<prns_groups::Message>, ApiError> {
    let page = client.messages(group, None, 100, SystemClock.now()).await?;
    let mut matching = page
        .messages
        .into_iter()
        .filter(|message| message.text == text);
    let message = matching.next();
    assert!(
        matching.next().is_none(),
        "a retained record must project exactly once"
    );
    Ok(message)
}

async fn wait_two_way_receipts(workers: &[Worker], group: GroupId, texts: [&str; 2]) {
    let started = Instant::now();
    let mut next_report = Instant::now();
    loop {
        let mut complete = true;
        let mut progress = Vec::new();
        for (author, text) in texts.iter().enumerate() {
            for (reader, worker) in workers.iter().enumerate() {
                match exact_message(&worker.client, group, text).await {
                    Ok(Some(message)) => {
                        assert_eq!(message.outgoing, reader == author);
                        assert_eq!(message.recipient_count, 1);
                        if reader == author && message.received_count != 1 {
                            complete = false;
                        }
                        progress.push((author, reader, Some(message.received_count)));
                    }
                    Ok(None) | Err(ApiError::Busy) => {
                        complete = false;
                        progress.push((author, reader, None));
                    }
                    Err(error) => panic!("history failed: {error:?}"),
                }
            }
        }
        if complete {
            eprintln!(
                "byte-paced two-way receipt convergence after {:?}",
                started.elapsed()
            );
            return;
        }
        if Instant::now() >= next_report {
            eprintln!(
                "byte-paced progress after {:?}: {progress:?}",
                started.elapsed()
            );
            next_report = Instant::now() + Duration::from_secs(15);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn report_slow_routes(nodes: &slow_wire::Nodes, identities: &[prns_host::IdentityHash]) {
    eprintln!(
        "byte-paced carrier: frames={} bytes={} resources={} dropped={} peak_waiting={} jobs={:?}",
        nodes.state.frames[1].load(Ordering::SeqCst),
        nodes.state.middle_bytes.load(Ordering::SeqCst),
        nodes.state.resources.load(Ordering::SeqCst),
        nodes.state.middle_dropped.load(Ordering::SeqCst),
        nodes.state.middle_peak_queue.load(Ordering::SeqCst),
        nodes
            .state
            .jobs
            .iter()
            .map(|count| count.load(Ordering::SeqCst))
            .collect::<Vec<_>>(),
    );
    for index in [0, 1, 3, 4] {
        let services = nodes.owners[index].client().service_client().unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            services.protocols().engine_inspection_snapshot(),
        )
        .await;
        if let Ok(Some(snapshot)) = result {
            for identity in identities {
                let destination = personal_rns::wire::DestinationHash::new(
                    prns_groups_reticulum::destination_for(*identity)
                        .unwrap()
                        .into_bytes(),
                );
                let route = snapshot
                    .routes
                    .iter()
                    .find(|route| route.destination == destination);
                eprintln!(
                    "byte-paced node={index} destination={destination:?} links={} route={route:?}",
                    snapshot.link_count
                );
            }
        }
    }
}

/// This uses real native hosts (each with its own runtime and clock), so pausing
/// only the test's Tokio clock would distort the carrier/core deadline relation.
/// Two bounded five-minute phases keep production polling and backoff intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn byte_paced_half_duplex_sync_recovers_both_directions_receipts_and_reopen() {
    const PHASE_BOUND: Duration = Duration::from_secs(300);
    DIAGNOSTIC_START.get_or_init(Instant::now);
    log::set_logger(&GROUP_DIAGNOSTICS).unwrap();
    log::set_max_level(log::LevelFilter::Info);
    let mut nodes = slow_wire::Nodes::byte_paced().await;
    let identities: Vec<_> = nodes.owners[..2]
        .iter()
        .map(|owner| owner.client().identity_hash().unwrap())
        .collect();
    let fixtures =
        fixture::generate("Byte-paced routed group", &identities, Timestamp(now())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let workers: Vec<_> = fixtures
        .iter()
        .enumerate()
        .map(|(index, fixture)| {
            Worker::open(
                directory.path().join(format!("paced-{index}.redb")),
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
    let original_scopes = [
        original_scope,
        workers[1].client.group_scope(group).await.unwrap(),
    ];
    let texts = [
        "one slow path, first direction",
        "one slow path, reverse direction",
    ];
    let tokens = [
        prns_groups::create_request_id().unwrap(),
        prns_groups::create_request_id().unwrap(),
    ];
    let mut record_ids = Vec::new();
    for index in 0..2 {
        record_ids.push(
            workers[index]
                .client
                .publish_request(send(
                    group,
                    original_scopes[index],
                    tokens[index],
                    texts[index],
                ))
                .await
                .unwrap(),
        );
    }
    let mut stops = Vec::new();
    let mut tasks = Vec::new();
    for (index, worker) in workers.iter().enumerate() {
        let (stop, receiver) = watch::channel(false);
        stops.push(stop);
        tasks.push(tokio::spawn(run_multi(
            worker.client.clone(),
            nodes.owners[index].client(),
            prns_groups_reticulum::destination_for(identities[index]).unwrap(),
            prns_groups_reticulum::enrollment::destination_for(identities[index]).unwrap(),
            nodes.jobs[index].take().unwrap(),
            nodes.enrollment_jobs[index].take().unwrap(),
            slow_wire::limits(),
            slow_wire::enrollment_limits(),
            receiver,
        )));
    }
    // Only run_multi's ordinary startup announces occur. There is no manual
    // announce, direct payload injection, accelerated polling or resubmission.
    let initial =
        tokio::time::timeout(PHASE_BOUND, wait_two_way_receipts(&workers, group, texts)).await;
    let offline = "saved while the only middle carrier is off";
    let offline_token = prns_groups::create_request_id().unwrap();
    let mut offline_record = None;
    let mut caught_up = false;
    if initial.is_ok() {
        assert!(nodes.state.resources.load(Ordering::SeqCst) > 0);
        nodes.state.middle_online.store(false, Ordering::SeqCst);
        let cut_started = Instant::now();
        let dropped_before = nodes.state.middle_dropped.load(Ordering::SeqCst);
        offline_record = Some(
            workers[0]
                .client
                .publish_request(send(group, original_scope, offline_token, offline))
                .await
                .unwrap(),
        );
        let catch_up = async {
            // Observe an actual carrier attempt during the cut, without claiming
            // that its frame necessarily carried this particular record.
            while cut_started.elapsed() < Duration::from_secs(60)
                || nodes.state.middle_dropped.load(Ordering::SeqCst) == dropped_before
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(exact_message(&workers[1].client, group, offline)
                .await
                .unwrap()
                .is_none());
            let saved = exact_message(&workers[0].client, group, offline)
                .await
                .unwrap()
                .unwrap();
            assert_eq!((saved.received_count, saved.recipient_count), (0, 1));
            eprintln!(
                "byte-paced carrier resumes after {:?} off; dropped_frames={}",
                cut_started.elapsed(),
                nodes.state.middle_dropped.load(Ordering::SeqCst) - dropped_before,
            );
            nodes.state.middle_online.store(true, Ordering::SeqCst);
            loop {
                let local = exact_message(&workers[0].client, group, offline)
                    .await
                    .unwrap();
                let remote = exact_message(&workers[1].client, group, offline)
                    .await
                    .unwrap();
                if local.is_some_and(|message| message.received_count == 1) && remote.is_some() {
                    eprintln!(
                        "byte-paced offline record and receipt converge after {:?}",
                        cut_started.elapsed(),
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        };
        caught_up = tokio::time::timeout(PHASE_BOUND, catch_up).await.is_ok();
    }
    report_slow_routes(&nodes, &identities).await;
    for stop in stops {
        stop.send(true).unwrap();
    }
    for task in tasks {
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("stop cancels network waits")
            .unwrap()
            .unwrap();
    }
    for worker in workers {
        worker.stop();
    }
    nodes.stop().await;

    // Reopen both durable stores, retaining receipt projections and immutable
    // operation IDs. Retry the same IDs only after transport has stopped.
    for index in 0..2 {
        let reopened = Worker::open(
            directory.path().join(format!("paced-{index}.redb")),
            fixtures[index].clone(),
            identities[index],
        );
        assert_eq!(
            reopened
                .client
                .publish_request(send(
                    group,
                    original_scopes[index],
                    tokens[index],
                    texts[index]
                ))
                .await
                .unwrap(),
            record_ids[index]
        );
        if initial.is_ok() {
            for text in texts {
                let message = exact_message(&reopened.client, group, text)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!((message.received_count, message.recipient_count), (1, 1));
            }
        }
        if index == 0 {
            if let Some(record) = offline_record {
                assert_eq!(
                    reopened
                        .client
                        .publish_request(send(group, original_scope, offline_token, offline))
                        .await
                        .unwrap(),
                    record
                );
            }
        }
        if caught_up {
            let message = exact_message(&reopened.client, group, offline)
                .await
                .unwrap()
                .unwrap();
            assert_eq!((message.received_count, message.recipient_count), (1, 1));
        }
        reopened.stop();
    }
    assert!(initial.is_ok(), "byte-paced content and signed receipts must converge in both directions without an artificial announce");
    assert!(
        caught_up,
        "the same saved record must catch up and reconcile after the only carrier returns"
    );
}
