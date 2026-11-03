//! Generic and text consumers share the same durable owner and carrier.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod support;
#[path = "../examples/support/telemetry_projection.rs"]
mod telemetry_projection;
use prns_groups::{fixture, Client, Error, Job, StoreSession, Submit};
use prns_groups_core::{NamespaceId, SchemaId};
use prns_groups_service::{ChangeKind, ClockReading, Timestamp};
use prns_host::IdentityHash;
use std::sync::{Arc, Mutex};
use support::{clock, explicit_targets, publication, PublicationClient, Worker, NOW};
use telemetry_projection::{Projection, Reading, Refresh};

fn request(
    group: prns_groups_core::GroupId,
    scope: prns_groups::OriginalScope,
    targets: Vec<prns_groups_core::Endpoint>,
) -> support::Prepared {
    use prns_groups::{ExecutionContext, Intent, PublicationIntent, Request, RequestId, Targets};
    (
        Request {
            id: RequestId(
                prns_groups_core::retry::RequestToken::new(NOW, [19; 20])
                    .unwrap()
                    .into_bytes(),
            ),
            original_scope: Some(scope),
            intent: Intent::Publish(PublicationIntent {
                group,
                namespace: NamespaceId(0x74656c65),
                schema: SchemaId(1),
                targets: Targets::Explicit(targets),
                // Application-defined sensor ID + temperature in centi-degrees.
                body: [3u32.to_be_bytes(), 2150i32.to_be_bytes()].concat(),
            }),
        },
        ExecutionContext { clock: clock() },
    )
}
struct Captured(Mutex<Option<Job>>, prns_groups::InputBudget);
impl Submit for Captured {
    fn input_budget(&self) -> &prns_groups::InputBudget {
        &self.1
    }
    fn submit(&self, job: Job) -> Result<(), Error> {
        let mut pending = self.0.lock().unwrap();
        if pending.is_some() {
            return Err(Error::Busy);
        }
        *pending = Some(job);
        Ok(())
    }
}

fn expired_clock() -> ClockReading {
    let later = Timestamp(NOW + 15 * 86400);
    ClockReading::Trusted(prns_groups_core::ClockInterval::new(later, later).unwrap())
}

#[tokio::test]
async fn caught_up_projection_rebuilds_after_lost_maintenance_result_and_reopen() {
    let identities = [IdentityHash::new([31; 16]), IdentityHash::new([32; 16])];
    let fixtures = fixture::generate("Projection recovery", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("projection.redb");
    let worker = Worker::open(path.clone(), fixtures[0].clone(), identities[0]);
    let cfg = worker.client.network_config().await.unwrap();
    let group = cfg.group;
    let incarnation = cfg.incarnation;
    let request = request(
        group,
        prns_groups::OriginalScope::Control {
            control: cfg.control,
            incarnation: cfg.incarnation,
        },
        cfg.peers.iter().map(|peer| peer.endpoint).collect(),
    );
    let id = worker.client.publish_request(request).await.unwrap();
    let retired_boundary = prns_groups::RecordCursor::from(
        worker
            .client
            .records(group, None, 1, clock())
            .await
            .unwrap()
            .records[0]
            .position,
    );
    let mut projection = Projection::new(256).unwrap();
    assert_eq!(
        projection
            .refresh(&worker.client, group, clock())
            .await
            .unwrap(),
        Refresh::Current
    );
    assert_eq!(projection.readings.len(), 1);
    let caught_up = projection.cursor.unwrap();
    assert_eq!(
        caught_up,
        worker.client.changes(group, None, 10).await.unwrap().head
    );
    worker.stop();

    // The admitted maintenance owns its transaction even when the waiting
    // consumer disappears. Reopen the actual database before observing results.
    let database = redb::Database::create(&path).unwrap();
    let cfg = fixture::load_and_install(&database, &fixtures[0], identities[0]).unwrap();
    let mut session = StoreSession::open(&database, cfg).unwrap();
    let lane = Arc::new(Captured(Mutex::new(None), Default::default()));
    let client = Client::new(lane.clone());
    let mut waiter = Box::pin(client.maintain(expired_clock()));
    assert!(futures_util::poll!(waiter.as_mut()).is_pending());
    drop(waiter);
    lane.0.lock().unwrap().take().unwrap().execute(&mut session);
    drop(session);
    drop(database);

    let worker = Worker::open(path, fixtures[0].clone(), identities[0]);
    let changes = worker
        .client
        .changes(group, Some(caught_up), 10)
        .await
        .unwrap();
    assert!(changes.changes.is_empty());
    assert!(
        changes.history_pruned,
        "a caught-up consumer must learn of deletion"
    );
    assert!(
        worker
            .client
            .records(group, Some(retired_boundary), 10, expired_clock())
            .await
            .unwrap()
            .records
            .is_empty(),
        "expiry preserves the scoped insertion boundary"
    );
    assert!(changes.head.position > caught_up.position);
    assert_eq!(changes.next, changes.head);
    assert!(worker
        .client
        .record(group, incarnation, id, expired_clock())
        .await
        .unwrap()
        .is_none());
    assert!(worker
        .client
        .delivery_progress(group, incarnation, id, expired_clock())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        projection
            .refresh(&worker.client, group, expired_clock())
            .await
            .unwrap(),
        Refresh::Current
    );
    assert!(
        projection.readings.is_empty(),
        "rebuild replaces the old projection"
    );
    assert_eq!(projection.cursor, Some(changes.head));
    let rebuilt = projection.clone();
    assert_eq!(
        projection
            .refresh(&worker.client, group, expired_clock())
            .await
            .unwrap(),
        Refresh::Current
    );
    assert_eq!(projection, rebuilt);
    worker.stop();
}

#[tokio::test]
async fn bounded_projection_keeps_rows_and_cursor_on_incomplete_refresh_or_wrong_store() {
    let identities = [IdentityHash::new([33; 16]), IdentityHash::new([34; 16])];
    let fixtures = fixture::generate("Projection bounds", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let worker = Worker::open(
        directory.path().join("local.redb"),
        fixtures[0].clone(),
        identities[0],
    );
    let other = Worker::open(
        directory.path().join("peer.redb"),
        fixtures[1].clone(),
        identities[1],
    );
    let cfg = worker.client.network_config().await.unwrap();
    let group = cfg.group;
    let mut request = request(
        group,
        prns_groups::OriginalScope::Control {
            control: cfg.control,
            incarnation: cfg.incarnation,
        },
        cfg.peers.iter().map(|peer| peer.endpoint).collect(),
    );
    worker
        .client
        .publish_request(request.clone())
        .await
        .unwrap();
    let mut projection = Projection::new(1).unwrap();
    assert_eq!(
        projection
            .refresh(&worker.client, group, clock())
            .await
            .unwrap(),
        Refresh::Current
    );
    let saved = projection.clone();
    // An actual other endpoint's cursor scope, not only a fabricated token.
    assert!(matches!(
        projection.refresh(&other.client, group, clock()).await,
        Err(Error::InvalidInput)
    ));
    assert_eq!(projection, saved);

    request.0.id.0 = prns_groups_core::retry::RequestToken::new(NOW, [23; 20])
        .unwrap()
        .into_bytes();
    publication(&mut request).body = Reading {
        sensor: 4,
        centi_celsius: 2275,
    }
    .encode()
    .to_vec();
    worker.client.publish_request(request).await.unwrap();
    assert_eq!(
        projection
            .refresh(&worker.client, group, clock())
            .await
            .unwrap(),
        Refresh::Incomplete
    );
    assert_eq!(
        projection, saved,
        "a partial rebuild must not advance the durable cursor"
    );
    let mut larger = Projection::new(2).unwrap();
    assert_eq!(
        larger
            .refresh(&worker.client, group, clock())
            .await
            .unwrap(),
        Refresh::Current
    );
    assert_eq!(larger.readings.len(), 2);
    worker.stop();
    other.stop();
}

#[tokio::test]
async fn projection_replays_insertions_and_restarts_after_retirement_during_paging() {
    // Interleave writes at a deterministic query boundary using the real native
    // owner and store. The third query follows the first record page and is the
    // replay from the pre-page anchor; neither race relies on timing or sleeps.
    for retire in [false, true] {
        let identities = [IdentityHash::new([35; 16]), IdentityHash::new([36; 16])];
        let fixtures =
            fixture::generate("Projection paging race", &identities, Timestamp(NOW)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let database = redb::Database::create(directory.path().join("race.redb")).unwrap();
        let cfg = fixture::load_and_install(&database, &fixtures[0], identities[0]).unwrap();
        let group = cfg.group;
        let mut request = request(
            group,
            prns_groups::OriginalScope::Control {
                control: cfg.control,
                incarnation: cfg.incarnation,
            },
            cfg.peers.iter().map(|peer| peer.endpoint).collect(),
        );
        let mut session = StoreSession::open(&database, cfg).unwrap();
        let lane = Arc::new(Captured(Mutex::new(None), Default::default()));
        let client = Client::new(lane.clone());
        let first = client.admit_request(request.clone()).unwrap();
        lane.0.lock().unwrap().take().unwrap().execute(&mut session);
        let first = first.await.unwrap();
        let mut projection = Projection::new(256).unwrap();
        let mut refresh = Box::pin(projection.refresh(&client, group, clock()));
        let mut queries = 0;
        let result = loop {
            if let std::task::Poll::Ready(result) = futures_util::poll!(refresh.as_mut()) {
                break result.unwrap();
            }
            queries += 1;
            assert!(queries < 20, "projection refresh must be bounded");
            let query = lane.0.lock().unwrap().take().unwrap();
            if queries == 3 {
                let writer = Arc::new(Captured(Mutex::new(None), Default::default()));
                let writer_client = Client::new(writer.clone());
                if retire {
                    let mut maintenance = Box::pin(writer_client.maintain(expired_clock()));
                    assert!(futures_util::poll!(maintenance.as_mut()).is_pending());
                    writer
                        .0
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .execute(&mut session);
                    maintenance.await.unwrap();
                } else {
                    request.0.id.0 = prns_groups_core::retry::RequestToken::new(NOW, [24; 20])
                        .unwrap()
                        .into_bytes();
                    publication(&mut request).body = Reading {
                        sensor: 4,
                        centi_celsius: 2275,
                    }
                    .encode()
                    .to_vec();
                    let second = writer_client.admit_request(request.clone()).unwrap();
                    writer
                        .0
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .execute(&mut session);
                    second.await.unwrap();
                }
            }
            query.execute(&mut session);
        };
        drop(refresh);
        assert_eq!(result, Refresh::Current);
        if retire {
            assert!(
                projection.readings.is_empty(),
                "invalidation must discard the stale page"
            );
            assert!(queries >= 6, "retirement must restart the paged rebuild");
        } else {
            assert_eq!(
                projection.readings.len(),
                2,
                "pre-page anchor must recover concurrent insertion"
            );
            assert!(projection.readings.contains_key(&first));
        }
    }
}

#[tokio::test]
async fn projection_reports_incomplete_when_retirement_repeatedly_overtakes_rebuild() {
    let identities = [IdentityHash::new([37; 16]), IdentityHash::new([38; 16])];
    let fixtures = fixture::generate("Projection churn", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let database = redb::Database::create(directory.path().join("churn.redb")).unwrap();
    let cfg = fixture::load_and_install(&database, &fixtures[0], identities[0]).unwrap();
    let group = cfg.group;
    let mut request = request(
        group,
        prns_groups::OriginalScope::Control {
            control: cfg.control,
            incarnation: cfg.incarnation,
        },
        cfg.peers.iter().map(|peer| peer.endpoint).collect(),
    );
    let mut session = StoreSession::open(&database, cfg).unwrap();
    let lane = Arc::new(Captured(Mutex::new(None), Default::default()));
    let client = Client::new(lane.clone());
    let first = client.admit_request(request.clone()).unwrap();
    lane.0.lock().unwrap().take().unwrap().execute(&mut session);
    first.await.unwrap();
    let mut projection = Projection::new(256).unwrap();
    let saved = projection.clone();
    let mut refresh = Box::pin(projection.refresh(&client, group, clock()));
    let mut queries = 0;
    let result = loop {
        if let std::task::Poll::Ready(result) = futures_util::poll!(refresh.as_mut()) {
            break result.unwrap();
        }
        queries += 1;
        assert!(
            queries <= 9,
            "three rebuild attempts must exhaust the retry budget"
        );
        let query = lane.0.lock().unwrap().take().unwrap();
        if queries % 3 == 0 {
            let writer = Arc::new(Captured(Mutex::new(None), Default::default()));
            let writer_client = Client::new(writer.clone());
            let cycle = queries / 3;
            let now = NOW + (cycle - 1) as u64 * 15 * 86400;
            if queries > 3 {
                request.0.id.0 =
                    prns_groups_core::retry::RequestToken::new(now, [queries as u8; 20])
                        .unwrap()
                        .into_bytes();
                request.1.clock = ClockReading::Trusted(
                    prns_groups_core::ClockInterval::new(Timestamp(now), Timestamp(now)).unwrap(),
                );
                let publication = writer_client.admit_request(request.clone()).unwrap();
                writer
                    .0
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .execute(&mut session);
                publication.await.unwrap();
            }
            let expired = Timestamp(now + 15 * 86400);
            let expired = ClockReading::Trusted(
                prns_groups_core::ClockInterval::new(expired, expired).unwrap(),
            );
            let mut maintenance = Box::pin(writer_client.maintain(expired));
            assert!(futures_util::poll!(maintenance.as_mut()).is_pending());
            writer
                .0
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .execute(&mut session);
            maintenance.await.unwrap();
        }
        query.execute(&mut session);
    };
    drop(refresh);
    assert_eq!(result, Refresh::Incomplete);
    assert_eq!(queries, 9);
    assert_eq!(
        projection, saved,
        "a repeatedly invalidated rebuild must not publish partial state"
    );
}

#[tokio::test]
async fn lost_generic_reply_reopens_exact_record_and_scoped_change_cursor() {
    let identities = [IdentityHash::new([11; 16]), IdentityHash::new([22; 16])];
    let fixtures = fixture::generate("Generic recovery", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("generic.redb");
    let database = redb::Database::create(&path).unwrap();
    let cfg = fixture::load_and_install(&database, &fixtures[0], identities[0]).unwrap();
    let group = cfg.group;
    let incarnation = cfg.incarnation;
    let request = request(
        group,
        prns_groups::OriginalScope::Control {
            control: cfg.control,
            incarnation: cfg.incarnation,
        },
        cfg.peers.iter().map(|p| p.endpoint).collect(),
    );
    let mut session = StoreSession::open(&database, cfg).unwrap();
    let lane = Arc::new(Captured(Mutex::new(None), Default::default()));
    let client = Client::new(lane.clone());
    let mut oversized = request.clone();
    publication(&mut oversized).body =
        vec![0; usize::try_from(prns_groups::ClientLimits::DEFAULT.max_job_bytes).unwrap()];
    assert!(matches!(
        client.admit_request(oversized),
        Err(prns_groups_core::ApiError::InvalidInput)
    ));
    let mut duplicate = request.clone();
    let targets = explicit_targets(&mut duplicate);
    targets.push(targets[0]);
    assert!(matches!(
        client.admit_request(duplicate),
        Err(prns_groups_core::ApiError::InvalidInput)
    ));
    let mut unsorted = request.clone();
    let mut another = explicit_targets(&mut unsorted)[0];
    another.id.0[0] ^= 1;
    explicit_targets(&mut unsorted).push(another);
    explicit_targets(&mut unsorted).sort();
    explicit_targets(&mut unsorted).reverse();
    assert!(matches!(
        client.admit_request(unsorted),
        Err(prns_groups_core::ApiError::InvalidInput)
    ));
    assert!(
        lane.0.lock().unwrap().is_none(),
        "invalid inputs must not occupy the lane"
    );
    let waiter = client.admit_request(request.clone()).unwrap();
    drop(waiter);
    lane.0.lock().unwrap().take().unwrap().execute(&mut session);
    drop(session);
    drop(database);
    let worker = Worker::open(path, fixtures[0].clone(), identities[0]);
    let page = worker
        .client
        .records(group, None, 10, clock())
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);
    let id = page.records[0].record.metadata.record;
    assert_eq!(
        page.records[0].record.body.as_slice(),
        support::publication_ref(&request).body
    );
    assert_eq!(page.records[0].progress.summary.pending, 1);
    assert!(worker
        .client
        .messages(group, None, 10, clock())
        .await
        .unwrap()
        .messages
        .is_empty());
    let mut replay = request.clone();
    replay.1.clock = ClockReading::Unknown;
    assert_eq!(worker.client.publish_request(replay).await.unwrap(), id);
    // The same shared control does not select another local membership store.
    // Reject this before either recovery or fresh encryption can use that store.
    let mut foreign_membership = request.clone();
    let Some(prns_groups::OriginalScope::Control { control, .. }) = request.0.original_scope else {
        panic!("joined scope")
    };
    let mut foreign = *incarnation.as_bytes();
    foreign[0] ^= 1;
    foreign_membership.0.original_scope = Some(prns_groups::OriginalScope::Control {
        control,
        incarnation: prns_groups_core::StoreIncarnation::new(foreign).unwrap(),
    });
    assert!(matches!(
        worker.client.publish_request(foreign_membership).await,
        Err(prns_groups_core::ApiError::Rejected {
            reason: prns_groups_core::RejectionReason::StaleMembership
        })
    ));
    for field in 0..5 {
        let mut changed = request.clone();
        match field {
            0 => publication(&mut changed).body.push(1),
            1 => publication(&mut changed).namespace.0 += 1,
            2 => publication(&mut changed).schema.0 += 1,
            3 => explicit_targets(&mut changed)[0].id.0[0] ^= 1,
            _ => {
                let Some(prns_groups::OriginalScope::Control {
                    mut control,
                    incarnation,
                }) = changed.0.original_scope
                else {
                    panic!("control")
                };
                control.era.0 .0[0] ^= 1;
                changed.0.original_scope = Some(prns_groups::OriginalScope::Control {
                    control,
                    incarnation,
                });
            }
        }
        assert!(matches!(
            worker.client.publish_request(changed).await,
            Err(prns_groups_core::ApiError::Rejected {
                reason: prns_groups_core::RejectionReason::RequestConflict
            })
        ));
    }
    let changes = worker.client.changes(group, None, 10).await.unwrap();
    assert_eq!(
        changes
            .changes
            .iter()
            .filter(|c| matches!(c.kind, ChangeKind::Published(_)))
            .count(),
        1
    );
    assert!(!changes.history_pruned);
    assert!(worker
        .client
        .changes(group, Some(changes.next), 10)
        .await
        .unwrap()
        .changes
        .is_empty());
    let mut wrong = changes.next;
    wrong.group.0[0] ^= 1;
    assert!(matches!(
        worker.client.changes(group, Some(wrong), 10).await,
        Err(prns_groups_core::ApiError::InvalidInput)
    ));
    let mut wrong = changes.next;
    wrong.incarnation = prns_groups_store::StoreIncarnation::new([99; 24]).unwrap();
    assert!(matches!(
        worker.client.changes(group, Some(wrong), 10).await,
        Err(prns_groups_core::ApiError::InvalidInput)
    ));
    assert!(
        worker
            .client
            .delivery_progress(group, incarnation, id, clock())
            .await
            .unwrap()
            .unwrap()
            .summary
            .pending
            == 1
    );
    assert_eq!(
        worker
            .client
            .record(group, incarnation, id, clock())
            .await
            .unwrap()
            .unwrap()
            .record
            .metadata
            .record,
        id
    );
    let later = prns_groups_core::Timestamp(NOW + 15 * 86400);
    let later = ClockReading::Trusted(prns_groups_core::ClockInterval::new(later, later).unwrap());
    for _ in 0..4 {
        worker.client.maintain(later).await.unwrap();
    }
    assert!(worker
        .client
        .record(group, incarnation, id, later)
        .await
        .unwrap()
        .is_none());
    assert!(
        worker
            .client
            .changes(group, None, 10)
            .await
            .unwrap()
            .history_pruned
    );
    worker.stop();
}

#[cfg(feature = "network")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_telemetry_reopens_then_syncs_with_text_and_one_application_change() {
    use prns_groups::{run, RunnerControl, RunnerOptions};
    use prns_groups_reticulum::{Network, SyncBudget};
    use std::time::Duration;
    let mut nodes = support::wire::Nodes::open().await;
    let identities: Vec<_> = nodes.owners[..2]
        .iter()
        .map(|n| n.client().identity_hash().unwrap())
        .collect();
    let fixtures = fixture::generate("Telemetry and text", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let open = |i: usize| {
        Worker::open(
            directory.path().join(format!("{i}.redb")),
            fixtures[i].clone(),
            identities[i],
        )
    };
    let first = open(0);
    let cfg = first.client.network_config().await.unwrap();
    let group = cfg.group;
    let request = request(
        group,
        prns_groups::OriginalScope::Control {
            control: cfg.control,
            incarnation: cfg.incarnation,
        },
        cfg.peers.iter().map(|p| p.endpoint).collect(),
    );
    let id = first.client.publish_request(request.clone()).await.unwrap();
    let saved = first.client.changes(group, None, 10).await.unwrap().next;
    first.stop(); // Exact output is pending; no network runner has started.
    let workers = [open(0), open(1)];
    let incarnations = [
        workers[0]
            .client
            .network_config()
            .await
            .unwrap()
            .incarnation,
        workers[1]
            .client
            .network_config()
            .await
            .unwrap()
            .incarnation,
    ];
    let text = workers[1]
        .client
        .publish_request(support::text_request(
            group,
            workers[1].client.group_scope(group).await.unwrap(),
            prns_groups_core::retry::RequestToken::new(NOW, [20; 20])
                .unwrap()
                .into_bytes(),
            "same group, other profile".into(),
            clock(),
        ))
        .await
        .unwrap();
    let mut runners = Vec::new();
    let mut controls = Vec::new();
    for (i, worker) in workers.iter().enumerate() {
        let cfg = worker.client.network_config().await.unwrap();
        let network = Arc::new(
            Network::new(
                nodes.owners[i].client(),
                nodes.owners[i].client().destination_hashes().unwrap()[0],
                cfg.peers,
                support::wire::limits(),
            )
            .unwrap(),
        );
        let (control, stop) = tokio::sync::watch::channel(RunnerControl::Running);
        controls.push(control);
        runners.push(tokio::spawn(run(
            worker.client.clone(),
            network,
            nodes.jobs[i].take().unwrap(),
            RunnerOptions {
                poll_interval: Duration::from_millis(100),
                retry_initial: Duration::from_millis(100),
                retry_max: Duration::from_millis(500),
                budget: SyncBudget {
                    max_requests: 4,
                    max_envelopes: 1,
                    max_bytes: 16384,
                },
            },
            Arc::new(support::TestClock),
            stop,
        )));
    }
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let a = workers[0]
                .client
                .delivery_progress(group, incarnations[0], id, clock())
                .await
                .unwrap();
            let b = workers[1]
                .client
                .delivery_progress(group, incarnations[1], text, clock())
                .await
                .unwrap();
            if a.is_some_and(|p| p.summary.all_received())
                && b.is_some_and(|p| p.summary.all_received())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        workers[0].client.publish_request(request).await.unwrap(),
        id
    );
    for worker in &workers {
        let all = worker
            .client
            .records(group, None, 10, clock())
            .await
            .unwrap();
        assert_eq!(all.records.len(), 2);
        let one = worker
            .client
            .records(group, None, 1, clock())
            .await
            .unwrap();
        let two = worker
            .client
            .records(group, one.next_before, 1, clock())
            .await
            .unwrap();
        assert_ne!(
            one.records[0].record.metadata.record,
            two.records[0].record.metadata.record
        );
        assert_eq!(
            worker
                .client
                .messages(group, None, 10, clock())
                .await
                .unwrap()
                .messages
                .len(),
            1
        );
        let feed = worker.client.changes(group, None, 100).await.unwrap();
        assert_eq!(
            feed.changes
                .iter()
                .filter(
                    |c| matches!(c.kind,ChangeKind::Published(r)|ChangeKind::Received(r) if r==id)
                )
                .count(),
            1
        );
    }
    assert!(
        !workers[0]
            .client
            .changes(group, Some(saved), 100)
            .await
            .unwrap()
            .history_pruned
    );
    for control in controls {
        control.send(RunnerControl::Stop).unwrap();
    }
    for runner in runners {
        runner.await.unwrap().unwrap();
    }
    for worker in workers {
        worker.stop();
    }
    nodes.stop().await;
}

#[tokio::test]
async fn full_change_page_reports_trailing_retirement_on_the_next_page() {
    let identities = [IdentityHash::new([31; 16]), IdentityHash::new([32; 16])];
    let fixtures = fixture::generate("Feed retention", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let worker = Worker::open(
        directory.path().join("feed.redb"),
        fixtures[0].clone(),
        identities[0],
    );
    let cfg = worker.client.network_config().await.unwrap();
    let at = |seconds| {
        let time = Timestamp(seconds);
        ClockReading::Trusted(prns_groups_core::ClockInterval::new(time, time).unwrap())
    };
    let mut later = request(
        cfg.group,
        prns_groups::OriginalScope::Control {
            control: cfg.control,
            incarnation: cfg.incarnation,
        },
        cfg.peers.iter().map(|p| p.endpoint).collect(),
    );
    later.1.clock = at(NOW + 3600);
    later.0.id.0 = prns_groups::RequestToken::new(NOW + 3600, [51; 20])
        .unwrap()
        .into_bytes();
    let retained = worker.client.publish_request(later.clone()).await.unwrap();
    let old = request(
        cfg.group,
        prns_groups::OriginalScope::Control {
            control: cfg.control,
            incarnation: cfg.incarnation,
        },
        explicit_targets(&mut later).clone(),
    );
    let expired = worker.client.publish_request(old).await.unwrap();
    let floor = at(NOW + 14 * 86400 + 1);
    for _ in 0..4 {
        worker.client.maintain(floor).await.unwrap();
    }
    assert!(worker
        .client
        .record(cfg.group, cfg.incarnation, expired, floor)
        .await
        .unwrap()
        .is_none());
    assert!(worker
        .client
        .record(cfg.group, cfg.incarnation, retained, floor)
        .await
        .unwrap()
        .is_some());
    let first = worker.client.changes(cfg.group, None, 1).await.unwrap();
    assert_eq!(first.changes.len(), 1);
    assert!(!first.history_pruned);
    assert!(first.next.position < first.head.position);
    let tail = worker
        .client
        .changes(cfg.group, Some(first.next), 1)
        .await
        .unwrap();
    assert!(tail.changes.is_empty());
    assert!(tail.history_pruned);
    assert_eq!(tail.next, tail.head);
    worker.stop();
}

#[tokio::test]
async fn exact_message_and_delivery_targets_are_bounded_offline_reads() {
    use prns_groups::{DeliveryTarget, DeliveryTargetPage, Intent, PageLimits, PublicationIntent};
    use prns_groups_core::{ApiError, ReceiptStatus, RecordId, RemovalStatus};
    for archive in std::iter::once(false).chain(cfg!(feature = "archive").then_some(true)) {
        let identities = [
            IdentityHash::new([71; 16]),
            IdentityHash::new([72; 16]),
            IdentityHash::new([73; 16]),
        ];
        let fixtures = fixture::generate("Exact details", &identities, Timestamp(NOW)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("details.redb");
        #[cfg(feature = "archive")]
        if archive {
            let database = redb::Database::create(&path).unwrap();
            fixture::load_and_install(&database, &fixtures[0], identities[0]).unwrap();
            let write = database.begin_write().unwrap();
            prns_groups::archive::initialize_in(
                &write,
                identities[0],
                prns_groups::archive::Limits {
                    max_records: 4,
                    max_bytes: 1_048_576,
                },
            )
            .unwrap();
            write.commit().unwrap();
        }
        let worker = Worker::open(path.clone(), fixtures[0].clone(), identities[0]);
        let cfg = worker.client.network_config().await.unwrap();
        let mut targets: Vec<_> = cfg.peers.iter().map(|peer| peer.endpoint).collect();
        targets.sort_unstable();
        let mut original = request(
            cfg.group,
            prns_groups::OriginalScope::Control {
                control: cfg.control,
                incarnation: cfg.incarnation,
            },
            targets,
        );
        let opaque = worker
            .client
            .publish_request(original.clone())
            .await
            .unwrap();
        original.0.id =
            prns_groups::RequestId::from(prns_groups::RequestToken::new(NOW, [51; 20]).unwrap());
        original.0.intent =
            Intent::Publish(PublicationIntent::text(cfg.group, "Exact retained text").unwrap());
        let text = worker.client.publish_request(original).await.unwrap();
        let before = worker
            .client
            .changes(cfg.group, None, 100)
            .await
            .unwrap()
            .head;
        assert_eq!(
            worker
                .client
                .message(cfg.group, cfg.incarnation, opaque, clock())
                .await
                .unwrap(),
            None
        );
        let message = worker
            .client
            .message(cfg.group, cfg.incarnation, text, clock())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            worker
                .client
                .messages(cfg.group, None, 10, clock())
                .await
                .unwrap()
                .messages,
            vec![message.clone()]
        );
        let limits = PageLimits {
            max_rows: 5,
            max_bytes: (std::mem::size_of::<DeliveryTargetPage>()
                + std::mem::size_of::<DeliveryTarget>()) as u64,
        };
        let first = worker
            .client
            .delivery_targets(cfg.group, cfg.incarnation, text, None, limits)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.targets.len(), 1);
        assert_eq!(first.next, Some(first.targets[0].endpoint));
        let second = worker
            .client
            .delivery_targets(cfg.group, cfg.incarnation, text, first.next, limits)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.targets.len(), 1);
        assert_eq!(second.next, None);
        assert!(first.targets[0].endpoint < second.targets[0].endpoint);
        for target in first.targets.iter().chain(&second.targets) {
            let peer = cfg
                .peers
                .iter()
                .find(|peer| peer.endpoint == target.endpoint)
                .unwrap();
            assert_eq!(
                target,
                &DeliveryTarget {
                    endpoint: peer.endpoint,
                    identity: Some(peer.identity),
                    receipt: ReceiptStatus::Awaiting,
                    removal: RemovalStatus::NotKnownRemoved
                }
            );
        }
        assert_eq!(
            worker
                .client
                .delivery_targets(
                    cfg.group,
                    cfg.incarnation,
                    text,
                    Some(second.targets[0].endpoint),
                    limits
                )
                .await
                .unwrap(),
            Some(DeliveryTargetPage {
                targets: vec![],
                next: None
            })
        );
        assert_eq!(
            worker
                .client
                .delivery_targets(cfg.group, cfg.incarnation, RecordId([99; 32]), None, limits)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            worker
                .client
                .message(cfg.group, cfg.incarnation, RecordId([99; 32]), clock())
                .await
                .unwrap(),
            None
        );
        let wrong = prns_groups_store::StoreIncarnation::new([99; 24]).unwrap();
        assert!(matches!(
            worker
                .client
                .delivery_targets(cfg.group, wrong, text, None, limits)
                .await,
            Err(ApiError::InvalidInput)
        ));
        assert!(matches!(
            worker.client.message(cfg.group, wrong, text, clock()).await,
            Err(ApiError::InvalidInput)
        ));
        assert!(matches!(
            worker
                .client
                .delivery_targets(
                    cfg.group,
                    cfg.incarnation,
                    text,
                    None,
                    PageLimits {
                        max_rows: 1,
                        max_bytes: limits.max_bytes - 1
                    }
                )
                .await,
            Err(ApiError::InvalidInput)
        ));
        assert_eq!(
            worker
                .client
                .changes(cfg.group, None, 100)
                .await
                .unwrap()
                .head,
            before,
            "detail reads cannot commit new events"
        );
        for _ in 0..4 {
            worker.client.maintain(expired_clock()).await.unwrap();
        }
        worker.stop();
        let worker = Worker::open(path, fixtures[0].clone(), identities[0]);
        let retained = worker
            .client
            .message(cfg.group, cfg.incarnation, text, expired_clock())
            .await
            .unwrap();
        let targets = worker
            .client
            .delivery_targets(cfg.group, cfg.incarnation, text, None, limits)
            .await
            .unwrap();
        if archive {
            assert_eq!(retained, Some(message));
            assert_eq!(targets, Some(first));
        } else {
            assert_eq!(retained, None);
            assert_eq!(targets, None);
        }
        worker.stop();
    }
}
