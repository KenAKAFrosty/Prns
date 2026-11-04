//! Real independent Reticulum nodes above a controlled byte carrier. Neither
//! tests nor bridge invoke the receiving group service with publication bytes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use prns_groups_mls::Adapter;
use prns_groups_reticulum::{
    push, reconcile, serve, serve_mut, Network, PushReport, RequestJob, SyncBudget, SyncCursor,
    SyncOptions, SyncReport,
};
use prns_groups_service::{
    ChangeCursor, ClockInterval, ClockReading, DeliverySummary, Descriptor, GroupService,
    MaintenanceOutcome, NamespaceId, PublicationValidity, RecordId, Seconds, Timestamp,
};
use prns_groups_store::Coordinator;
use prns_groups_store_redb::RedbStoreOwner;
use tokio::sync::{mpsc, oneshot};

use support::{
    clock, control, coordinator_limits, endpoint, incarnation, mls_limits, next, publication,
    service_limits, store_limits, Trio, CHAT, GROUP, STORAGE, TELEMETRY,
};

enum Command {
    LoseNextReply(Arc<support::wire::WireState>),
    IgnoreCapabilities,
    Push {
        peer: usize,
        record: RecordId,
        budget: SyncBudget,
        reply: oneshot::Sender<Result<PushReport, String>>,
    },
    Publish {
        nonce: u8,
        namespace: NamespaceId,
        body: Vec<u8>,
        deadline: u64,
        reply: oneshot::Sender<RecordId>,
    },
    Sync {
        peer: usize,
        budget: SyncBudget,
        cursor: Option<SyncCursor>,
        clock: ClockReading,
        reply: oneshot::Sender<Result<SyncReport, String>>,
    },
    Body {
        record: RecordId,
        reply: oneshot::Sender<Option<Vec<u8>>>,
    },
    Progress {
        record: RecordId,
        reply: oneshot::Sender<DeliverySummary>,
    },
    ChangeHead(oneshot::Sender<ChangeCursor>),
    Maintain {
        clock: ClockReading,
        reply: oneshot::Sender<MaintenanceOutcome>,
    },
    Stop,
}

struct EndpointActor {
    links: Arc<Mutex<Vec<(u8, prns_host::LinkId)>>>,
    commands: mpsc::Sender<Command>,
    task: tokio::task::JoinHandle<mpsc::Receiver<RequestJob>>,
}
impl EndpointActor {
    fn start(
        fixture: Arc<Trio>,
        index: usize,
        network: Arc<Network>,
        jobs: mpsc::Receiver<RequestJob>,
    ) -> Self {
        Self::start_mode(fixture, index, network, jobs, false)
    }
    fn start_mode(
        fixture: Arc<Trio>,
        index: usize,
        network: Arc<Network>,
        mut jobs: mpsc::Receiver<RequestJob>,
        supports_push: bool,
    ) -> Self {
        let links = Arc::new(Mutex::new(Vec::new()));
        let received_links = Arc::clone(&links);
        let network = Arc::new((*network).clone().with_push_enabled(supports_push));
        let (commands, mut receiver) = mpsc::channel(8);
        let task = tokio::task::spawn_local(async move {
            let database =
                redb::Database::open(fixture.directory.path().join(format!("peer-{index}.redb")))
                    .unwrap();
            let mut owner = RedbStoreOwner::new(&database);
            let backend = owner
                .open(STORAGE, incarnation(index), store_limits())
                .unwrap();
            let adapter = Adapter::new(
                Coordinator::new(backend, coordinator_limits()).unwrap(),
                fixture.identity(index),
                fixture.policy.clone(),
                mls_limits(),
            )
            .unwrap();
            let mut service = GroupService::new(
                adapter,
                Descriptor {
                    group: GROUP,
                    control: control(),
                    readers: &fixture.readers,
                },
                service_limits(),
            )
            .unwrap();
            let mut lose_next_reply: Option<Arc<support::wire::WireState>> = None;
            let mut ignore_capabilities = false;
            loop {
                tokio::select! {
                    job = jobs.recv() => {
                        let Some(job) = job else { break };
                        received_links.lock().unwrap().push((job.data()[4], job.link()));
                        if ignore_capabilities && job.data()[4] == 4 { continue; }
                        if job.data()[4] == 5 {
                            if let Some(wire) = lose_next_reply.take() { wire.set_online(index, false); }
                        }
                        let result = if supports_push {
                            serve_mut(&network, &mut service, &job, SyncOptions {
                                incarnation: incarnation(index), clock: clock(), budget: budget(32),
                            }).await
                        } else { serve(&network, &service, &job).await };
                        match result {
                            Ok(()) | Err(prns_groups_reticulum::Error::Service(
                                prns_groups_service::Error::Fault(
                                    prns_groups_service::Fault::RestartInventory,
                                ),
                            )) => {},
                            Err(error) => panic!("unexpected serving failure: {error:?}"),
                        }
                    },
                    command = receiver.recv() => match command {
                        Some(Command::LoseNextReply(wire)) => lose_next_reply = Some(wire),
                        Some(Command::IgnoreCapabilities) => ignore_capabilities = true,
                        Some(Command::Push { peer, record, budget, reply }) => {
                            let snapshot = service.snapshot().unwrap().unwrap();
                            let info = service.inventory(snapshot, prns_groups_service::InventoryCursor(0),
                                prns_groups_service::InventoryKind::Record, endpoint(peer), 128).unwrap()
                                .items.into_iter().find(|info| info.record == record).unwrap();
                            let result = push(&network, &mut service, endpoint(peer), GROUP, info,
                                SyncOptions { incarnation: incarnation(index), clock: clock(), budget }).await;
                            reply.send(result.map_err(|error| format!("{error:?}"))).unwrap();
                        },
                        Some(Command::Publish { nonce, namespace, body, deadline, reply }) => {
                            let mut request = publication(nonce, namespace, &fixture.readers[1..], &body);
                            request.validity = PublicationValidity::new(Timestamp(support::NOW),
                                Timestamp(deadline), Seconds(3600)).unwrap();
                            let published = service.publish(next(index, service.coordinator()),
                                &request, clock()).unwrap();
                            reply.send(published.record).unwrap();
                        },
                        Some(Command::Sync { peer, budget, cursor, clock, reply }) => {
                            let result = reconcile(&network, &mut service, endpoint(peer), GROUP,
                                SyncOptions { incarnation: incarnation(index), clock, budget }, cursor).await;
                            reply.send(result.map_err(|error| format!("{error:?}"))).unwrap();
                        },
                        Some(Command::Body { record, reply }) => {
                            reply.send(service.record(record).unwrap().map(|record| record.body.to_vec())).unwrap();
                        },
                        Some(Command::Progress { record, reply }) => {
                            reply.send(service.progress(record, clock()).unwrap()).unwrap();
                        },
                        Some(Command::ChangeHead(reply)) => {
                            reply.send(service.changes(ChangeCursor(0), 128).unwrap().head).unwrap();
                        },
                        Some(Command::Maintain { clock, reply }) => {
                            let plan = service.maintenance_plan(clock, 32, 128).unwrap();
                            let outcome = service.maintain(next(index, service.coordinator()), &plan).unwrap();
                            reply.send(outcome).unwrap();
                        },
                        Some(Command::Stop) | None => break,
                    }
                }
            }
            jobs
        });
        Self {
            commands,
            task,
            links,
        }
    }
    async fn push(
        &self,
        peer: usize,
        record: RecordId,
        budget: SyncBudget,
    ) -> Result<PushReport, String> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Push {
                peer,
                record,
                budget,
                reply,
            })
            .await
            .unwrap();
        result.await.unwrap()
    }
    async fn publish(&self, nonce: u8, namespace: NamespaceId, body: &[u8]) -> RecordId {
        self.publish_until(nonce, namespace, body, support::NOW + 600)
            .await
    }
    async fn publish_until(
        &self,
        nonce: u8,
        namespace: NamespaceId,
        body: &[u8],
        deadline: u64,
    ) -> RecordId {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Publish {
                nonce,
                namespace,
                body: body.to_vec(),
                deadline,
                reply,
            })
            .await
            .unwrap();
        result.await.unwrap()
    }
    async fn sync(
        &self,
        peer: usize,
        budget: SyncBudget,
        cursor: Option<SyncCursor>,
    ) -> SyncReport {
        self.sync_result(peer, budget, cursor).await.unwrap()
    }
    async fn sync_result(
        &self,
        peer: usize,
        budget: SyncBudget,
        cursor: Option<SyncCursor>,
    ) -> Result<SyncReport, String> {
        self.sync_at(peer, budget, cursor, clock()).await
    }
    async fn sync_at(
        &self,
        peer: usize,
        budget: SyncBudget,
        cursor: Option<SyncCursor>,
        clock: ClockReading,
    ) -> Result<SyncReport, String> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Sync {
                peer,
                budget,
                cursor,
                clock,
                reply,
            })
            .await
            .unwrap();
        result.await.unwrap()
    }
    async fn body(&self, record: RecordId) -> Option<Vec<u8>> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Body { record, reply })
            .await
            .unwrap();
        result.await.unwrap()
    }
    async fn progress(&self, record: RecordId) -> DeliverySummary {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Progress { record, reply })
            .await
            .unwrap();
        result.await.unwrap()
    }
    async fn change_head(&self) -> ChangeCursor {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::ChangeHead(reply))
            .await
            .unwrap();
        result.await.unwrap()
    }
    async fn maintain(&self, clock: ClockReading) -> MaintenanceOutcome {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Maintain { clock, reply })
            .await
            .unwrap();
        result.await.unwrap()
    }
    async fn stop(self) -> mpsc::Receiver<RequestJob> {
        self.commands.send(Command::Stop).await.unwrap();
        self.task.await.unwrap()
    }
}

fn budget(max_envelopes: usize) -> SyncBudget {
    SyncBudget {
        max_requests: 64,
        max_envelopes,
        max_bytes: 1_048_576,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn constrained_soft_target_yields_after_a_larger_legal_envelope_without_starvation() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            // Exercise constrained work planning on the ordinary fast test
            // carrier: transfer correctness is real, but this test does not
            // claim radio timing evidence.
            let peers = (0..3)
                .map(|index| *nodes.networks[1].peer(endpoint(index)).unwrap())
                .collect();
            let constrained = Network::new(
                nodes.owners[1].client(),
                nodes.owners[1].client().destination_hashes().unwrap()[0],
                peers,
                support::wire::limits(),
            )
            .unwrap()
            .with_sync_policy(prns_groups_core::sync::SyncPolicy {
                fast_bitrate_bps: u64::MAX,
                ..Default::default()
            })
            .unwrap();
            nodes.networks[1] = Arc::new(constrained);
            let alice = EndpointActor::start(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
            );
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
            );
            let body = vec![17; 4096];
            let large = alice.publish(71, TELEMETRY, &body).await;
            let following = alice.publish(72, CHAT, b"after telemetry").await;
            let first = bob.sync(0, budget(32), None).await;
            assert_eq!(first.accepted_records, 1);
            assert_eq!(first.fetched, 1);
            assert!(
                first.bytes > 4096,
                "a legal envelope may exceed the soft target"
            );
            assert_eq!(first.path.local_bitrate_bps, Some(1_000_000));
            assert_eq!(first.path.hops, Some(1));
            assert!(first.path.rtt_millis.is_some());
            assert!(first.cursor.is_some());
            assert_eq!(bob.body(large).await, Some(body));
            assert!(bob.body(following).await.is_none());

            let mut cursor = first.cursor;
            let mut fetched = first.fetched;
            for _ in 0..4 {
                let pass = bob.sync(0, budget(32), cursor).await;
                assert!(pass.fetched <= 1);
                fetched += pass.fetched;
                cursor = pass.cursor;
                if !pass.has_more() {
                    break;
                }
            }
            assert!(cursor.unwrap().is_complete());
            assert_eq!(fetched, 2);
            assert_eq!(
                bob.body(following).await.as_deref(),
                Some(b"after telemetry".as_slice())
            );
            let head = bob.change_head().await;
            let repeated = bob.sync(0, budget(32), None).await;
            assert_eq!(repeated.fetched, 0);
            assert_eq!(bob.change_head().await, head);
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn expired_dependency_receipt_does_not_block_later_records_or_receipt_cursor() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            let alice = EndpointActor::start(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
            );
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
            );
            let carol = EndpointActor::start(
                Arc::clone(&fixture),
                2,
                Arc::clone(&nodes.networks[2]),
                nodes.jobs[2].take().unwrap(),
            );
            nodes.state.set_online(2, false);
            let expired = alice
                .publish(61, CHAT, b"expired while Carol was offline")
                .await;
            let current = alice
                .publish_until(62, CHAT, b"still deliverable", support::NOW + 1200)
                .await;
            let received = bob.sync(0, budget(32), None).await;
            assert_eq!(received.accepted_records, 2);
            nodes.state.set_online(0, false);
            nodes.state.set_online(2, true);
            let late = ClockReading::Trusted(
                ClockInterval::new(Timestamp(support::NOW + 601), Timestamp(support::NOW + 601))
                    .unwrap(),
            );
            let first = carol.sync_at(1, budget(1), None, late).await.unwrap();
            assert_eq!(first.rejected, 1);
            assert_eq!(first.accepted_records, 0);
            let checkpoint = first.cursor.unwrap().to_bytes();
            let jobs = carol.stop().await;
            let carol = EndpointActor::start(
                Arc::clone(&fixture),
                2,
                Arc::clone(&nodes.networks[2]),
                jobs,
            );
            let resumed = carol
                .sync_at(
                    1,
                    budget(32),
                    Some(SyncCursor::from_bytes(&checkpoint).unwrap()),
                    late,
                )
                .await
                .unwrap();
            assert_eq!(resumed.accepted_records, 1);
            assert_eq!(resumed.accepted_receipts, 1);
            assert_eq!(
                resumed.rejected, 1,
                "expired dependency receipt is a durable rejection"
            );
            assert!(
                !resumed.has_more(),
                "reconciliation crosses the terminal dependency"
            );
            assert!(carol.body(expired).await.is_none());
            assert_eq!(
                carol.body(current).await.as_deref(),
                Some(b"still deliverable".as_slice())
            );
            assert!(carol.progress(current).await.all_received());
            let head = carol.change_head().await;
            let repeated = carol.sync_at(1, budget(32), None, late).await.unwrap();
            assert!(!repeated.has_more());
            assert_eq!(carol.change_head().await, head);
            // Nothing in the expired rejection can manufacture Carol's receipt.
            assert_eq!(bob.progress(expired).await.received, 1);
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            let _ = carol.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn independent_nodes_catch_up_through_another_member_and_reconcile_receipts() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            let alice = EndpointActor::start(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
            );
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
            );
            let carol = EndpointActor::start(
                Arc::clone(&fixture),
                2,
                Arc::clone(&nodes.networks[2]),
                nodes.jobs[2].take().unwrap(),
            );
            nodes.state.set_online(2, false);
            let chat_body = b"A group message survives its author's disconnection.";
            let telemetry_body: Vec<_> = (0..4096)
                .map(|offset| (offset * 17 + offset / 13) as u8)
                .collect();
            let chat = alice.publish(11, CHAT, chat_body).await;
            let telemetry = alice.publish(12, TELEMETRY, &telemetry_body).await;
            assert_eq!(alice.progress(chat).await.received, 0);
            assert!(carol.body(chat).await.is_none());

            let invalid_budget = bob
                .sync_result(
                    0,
                    SyncBudget {
                        max_requests: 1,
                        ..budget(32)
                    },
                    None,
                )
                .await;
            assert!(invalid_budget.unwrap_err().contains("InvalidBudget"));
            let discovery_only = bob
                .sync(
                    0,
                    SyncBudget {
                        max_requests: 2,
                        ..budget(32)
                    },
                    None,
                )
                .await;
            assert_eq!(discovery_only.requests, 2);
            assert_eq!(discovery_only.fetched, 0);
            let byte_limited = bob
                .sync_result(
                    0,
                    SyncBudget {
                        max_bytes: 1,
                        ..budget(32)
                    },
                    discovery_only.cursor,
                )
                .await;
            assert!(byte_limited.unwrap_err().contains("EnvelopeBudget"));
            assert!(
                bob.body(chat).await.is_none(),
                "a byte budget cannot partially commit a record"
            );
            let first = bob.sync(0, budget(1), discovery_only.cursor).await;
            assert_eq!(first.accepted_records, 1);
            assert_eq!(first.fetched, 1);
            let cursor = first
                .cursor
                .expect("one-envelope budget leaves a durable inventory continuation");
            // Drop every database owner/service value, then physically reopen the
            // independently persisted endpoint before using the same continuation.
            let bob_jobs = bob.stop().await;
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                bob_jobs,
            );
            let checkpoint = fixture.directory.path().join("bob-sync-checkpoint");
            std::fs::write(&checkpoint, cursor.to_bytes()).unwrap();
            let restored = SyncCursor::from_bytes(&std::fs::read(checkpoint).unwrap()).unwrap();
            let resumed = bob.sync(0, budget(32), Some(restored)).await;
            assert_eq!(resumed.accepted_records, 1);
            assert!(!resumed.has_more());
            assert_eq!(bob.body(chat).await.as_deref(), Some(chat_body.as_slice()));
            assert_eq!(
                bob.body(telemetry).await.as_deref(),
                Some(telemetry_body.as_slice())
            );
            assert!(
                nodes.state.resources.load(Ordering::SeqCst) > 0,
                "large MLS ciphertext traverses Reticulum Resources"
            );
            assert!(
                nodes.state.resource_proofs.load(Ordering::SeqCst) > 0,
                "Reticulum proves the received Resource"
            );
            assert_eq!(
                alice.progress(chat).await.received,
                0,
                "serving and proving transport data is not a device receipt"
            );

            let bob_head = bob.change_head().await;
            let duplicate = bob.sync(0, budget(32), None).await;
            assert_eq!(
                duplicate.fetched, 0,
                "inventory suppresses exact ciphertext replay before MLS"
            );
            assert_eq!(duplicate.accepted_records, 0);
            assert_eq!(bob.change_head().await, bob_head);

            nodes.state.set_online(0, false);
            nodes.state.set_online(2, true);
            let alice_frames = nodes.state.frames[0].load(Ordering::SeqCst);
            let caught_up = carol.sync(1, budget(32), None).await;
            assert_eq!(caught_up.accepted_records, 2);
            assert_eq!(caught_up.accepted_receipts, 2);
            assert_eq!(
                carol.body(chat).await.as_deref(),
                Some(chat_body.as_slice())
            );
            assert_eq!(
                carol.body(telemetry).await.as_deref(),
                Some(telemetry_body.as_slice())
            );
            assert_eq!(
                nodes.state.frames[0].load(Ordering::SeqCst),
                alice_frames,
                "offline catch-up obtains retained author ciphertext from Bob alone"
            );
            let bob_receipts = bob.sync(2, budget(32), None).await;
            assert_eq!(bob_receipts.accepted_receipts, 2);
            nodes.state.set_online(0, true);
            let alice_receipts = alice.sync(1, budget(32), None).await;
            assert_eq!(alice_receipts.accepted_receipts, 4);
            for actor in [&alice, &bob, &carol] {
                for record in [chat, telemetry] {
                    let delivery = actor.progress(record).await;
                    assert_eq!(delivery.original_targets, 2);
                    assert_eq!(delivery.received, 2);
                    assert_eq!(delivery.pending, 0);
                }
            }

            // Authenticated carrier input is still untrusted protocol input. A
            // typed rejection does not mutate durable application history.
            let before_malformed = alice.change_head().await;
            let connection = nodes.networks[1]
                .connect(nodes.networks[1].peer(endpoint(0)).unwrap())
                .await
                .unwrap();
            let rejection = connection
                .request(b"malformed group request")
                .await
                .unwrap();
            assert_eq!(rejection.as_slice(), b"PGT\x01\x00\x03");
            assert_eq!(alice.change_head().await, before_malformed);
            drop(connection);
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if nodes.owners[1]
                        .client()
                        .snapshot()
                        .await
                        .unwrap()
                        .active_link_count
                        == 0
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("dropping the carrier connection closes its Link while keeping the host alive");

            // D can discover and authenticate A's public SINGLE destination, but it
            // is not a provisioned group endpoint and reaches no inventory handler.
            let admitted_before = nodes.state.jobs[0].load(Ordering::SeqCst);
            let stranger = &nodes.networks[3];
            let connection = stranger
                .connect(stranger.peer(endpoint(0)).unwrap())
                .await
                .unwrap();
            assert!(connection
                .request(b"unauthorized inventory probe")
                .await
                .is_err());
            connection.close().await.unwrap();
            assert_eq!(nodes.state.jobs[0].load(Ordering::SeqCst), admitted_before);

            for actor in [alice, bob, carol] {
                drop(actor.stop().await);
            }
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn completed_inventory_checkpoint_survives_reopen_and_fetches_only_new_suffix() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            let alice = EndpointActor::start(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
            );
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
            );
            let first_record = alice.publish(81, CHAT, b"first retained record").await;
            let second_record = alice.publish(82, CHAT, b"second retained record").await;
            let first = bob.sync(0, budget(32), None).await;
            assert_eq!(first.accepted_records, 2);
            assert!(!first.has_more());
            let completed = first
                .cursor
                .expect("finished inventory retains its high-water mark");
            assert!(completed.is_complete());
            let before = bob.change_head().await;
            for _ in 0..2 {
                let idle = bob.sync(0, budget(32), Some(completed)).await;
                assert_eq!(
                    idle.requests, 1,
                    "unchanged peers need only a snapshot request"
                );
                assert_eq!(
                    idle.already_known, 0,
                    "old ciphertext metadata is not rescanned"
                );
                assert_eq!(idle.fetched, 0);
                assert_eq!(idle.cursor, Some(completed));
                assert!(!idle.has_more());
            }
            assert_eq!(bob.change_head().await, before);

            // Persist the completed mark separately after its durable receives,
            // then destroy every local store owner before opening it again.
            let checkpoint = fixture
                .directory
                .path()
                .join("completed-inventory-checkpoint");
            std::fs::write(&checkpoint, completed.to_bytes()).unwrap();
            let jobs = bob.stop().await;
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                jobs,
            );
            let restored = SyncCursor::from_bytes(&std::fs::read(&checkpoint).unwrap()).unwrap();
            let third_record = alice.publish(83, CHAT, b"new suffix after reopen").await;
            let suffix = bob.sync(0, budget(32), Some(restored)).await;
            assert_eq!(suffix.accepted_records, 1);
            assert_eq!(suffix.fetched, 1);
            assert_eq!(suffix.already_known, 0);
            assert!(!suffix.has_more());
            for record in [first_record, second_record, third_record] {
                assert!(bob.body(record).await.is_some());
            }
            let idle = bob.sync(0, budget(32), suffix.cursor).await;
            assert_eq!(idle.requests, 1);
            assert_eq!(idle.already_known, 0);
            assert_eq!(idle.fetched, 0);
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn receipt_phase_replays_after_lost_checkpoint_and_late_relay_receipts_form_new_suffix() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            let alice = EndpointActor::start(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
            );
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
            );
            let carol = EndpointActor::start(
                Arc::clone(&fixture),
                2,
                Arc::clone(&nodes.networks[2]),
                nodes.jobs[2].take().unwrap(),
            );
            let records = [
                alice.publish(91, CHAT, b"first receipt dependency").await,
                alice.publish(92, CHAT, b"second receipt dependency").await,
            ];
            let bob_records = bob.sync(0, budget(32), None).await;
            assert_eq!(bob_records.accepted_records, 2);
            assert!(!bob_records.has_more());
            let author_checkpoint = alice.sync(1, budget(32), None).await;
            assert_eq!(author_checkpoint.accepted_receipts, 2);
            assert!(!author_checkpoint.has_more());
            for record in records {
                assert_eq!(alice.progress(record).await.received, 1);
            }

            // Two records are committed before the first relayed receipt. The
            // envelope budget leaves the second receipt for a later pass.
            let partial = carol.sync(1, budget(3), None).await;
            assert_eq!(partial.accepted_records, 2);
            assert_eq!(partial.accepted_receipts, 1);
            assert!(partial.has_more());
            let checkpoint = fixture.directory.path().join("receipt-phase-checkpoint");
            std::fs::write(&checkpoint, partial.cursor.unwrap().to_bytes()).unwrap();
            let committed_head = {
                let committed_without_checkpoint = carol.sync(1, budget(1), partial.cursor).await;
                assert_eq!(committed_without_checkpoint.accepted_records, 0);
                assert_eq!(committed_without_checkpoint.accepted_receipts, 1);
                assert!(!committed_without_checkpoint.has_more());
                carol.change_head().await
                // The new result is deliberately lost, exactly as when the
                // process exits after receive but before cursor persistence.
            };
            let jobs = carol.stop().await;
            let carol = EndpointActor::start(
                Arc::clone(&fixture),
                2,
                Arc::clone(&nodes.networks[2]),
                jobs,
            );
            let restored = SyncCursor::from_bytes(&std::fs::read(&checkpoint).unwrap()).unwrap();
            let replay = carol.sync(1, budget(32), Some(restored)).await;
            assert_eq!(replay.accepted_records, 0);
            assert_eq!(replay.accepted_receipts, 0);
            assert_eq!(replay.fetched, 0);
            assert_eq!(replay.already_known, 1);
            assert!(!replay.has_more());
            assert_eq!(carol.change_head().await, committed_head);
            for record in records {
                assert!(carol.progress(record).await.all_received());
            }

            // Bob learns Carol's durable receipts after Alice finished scanning
            // Bob. Alice must revisit only that appended receipt suffix.
            let relayed = bob.sync(2, budget(32), None).await;
            assert_eq!(relayed.accepted_receipts, 2);
            assert!(!relayed.has_more());
            let late_receipts = alice.sync(1, budget(32), author_checkpoint.cursor).await;
            assert_eq!(late_receipts.accepted_records, 0);
            assert_eq!(late_receipts.accepted_receipts, 2);
            assert_eq!(late_receipts.fetched, 2);
            assert_eq!(late_receipts.already_known, 0);
            assert!(!late_receipts.has_more());
            for actor in [&alice, &bob, &carol] {
                for record in records {
                    assert!(actor.progress(record).await.all_received());
                }
            }
            let idle = alice.sync(1, budget(32), late_receipts.cursor).await;
            assert_eq!(idle.requests, 1);
            assert_eq!(idle.already_known, 0);
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            let _ = carol.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn retirement_generation_resets_completed_checkpoint_and_rejects_stale_partial_scan() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            let alice = EndpointActor::start(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
            );
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
            );
            let carol = EndpointActor::start(
                Arc::clone(&fixture),
                2,
                Arc::clone(&nodes.networks[2]),
                nodes.jobs[2].take().unwrap(),
            );
            let expired = alice
                .publish(101, CHAT, b"expires before maintenance")
                .await;
            let surviving = alice
                .publish_until(
                    102,
                    CHAT,
                    b"survives the generation change",
                    support::NOW + 1200,
                )
                .await;
            let completed = bob.sync(0, budget(32), None).await;
            assert!(!completed.has_more());
            let partial = carol.sync(0, budget(1), None).await;
            assert_eq!(partial.accepted_records, 1);
            assert!(partial.has_more());
            let late = ClockReading::Trusted(
                ClockInterval::new(Timestamp(support::NOW + 601), Timestamp(support::NOW + 601))
                    .unwrap(),
            );
            let retired = alice.maintain(late).await;
            assert_eq!(retired.records_removed, 1);
            assert!(retired.envelopes_removed > 0);
            assert!(alice.body(expired).await.is_none());
            let new_record = alice
                .publish_until(
                    103,
                    CHAT,
                    b"appended after maintenance",
                    support::NOW + 1200,
                )
                .await;

            let stale = carol.sync_at(0, budget(32), partial.cursor, late).await;
            assert!(stale.unwrap_err().contains("RestartInventory"));
            let reset = carol.sync_at(0, budget(32), None, late).await.unwrap();
            assert_eq!(reset.accepted_records, 2);
            assert!(!reset.has_more());
            let refreshed = bob
                .sync_at(0, budget(32), completed.cursor, late)
                .await
                .unwrap();
            assert_eq!(refreshed.accepted_records, 1);
            assert_eq!(
                refreshed.already_known, 1,
                "generation change rescans the retained prefix"
            );
            assert!(!refreshed.has_more());
            for actor in [&bob, &carol] {
                assert!(actor.body(surviving).await.is_some());
                assert!(actor.body(new_record).await.is_some());
            }
            let idle = bob
                .sync_at(0, budget(32), refreshed.cursor, late)
                .await
                .unwrap();
            assert_eq!(idle.requests, 1);
            assert_eq!(idle.already_known, 0);
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            let _ = carol.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn regressed_inventory_head_invalidates_partial_receipt_checkpoint_without_losing_receipts() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            let alice = EndpointActor::start(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
            );
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
            );
            let carol = EndpointActor::start(
                Arc::clone(&fixture),
                2,
                Arc::clone(&nodes.networks[2]),
                nodes.jobs[2].take().unwrap(),
            );
            let records = [
                alice.publish(111, CHAT, b"first rollback dependency").await,
                alice
                    .publish(112, CHAT, b"second rollback dependency")
                    .await,
            ];
            let bob_records = bob.sync(0, budget(32), None).await;
            assert_eq!(bob_records.accepted_records, 2);
            assert!(!bob_records.has_more());
            let partial = carol.sync(1, budget(3), None).await;
            assert_eq!(partial.accepted_records, 2);
            assert_eq!(partial.accepted_receipts, 1);
            assert!(partial.has_more());
            let before = carol.change_head().await;

            // Model a saved partial snapshot obtained before its source was
            // restored to an earlier head within the same inventory generation.
            // Alter only the versioned checkpoint's frozen head; the recipient's
            // committed records, receipts, scan position and base stay intact.
            let mut saved = partial.cursor.unwrap().to_bytes();
            let old_head = u64::from_be_bytes(saved[148..156].try_into().unwrap());
            saved[148..156].copy_from_slice(&(old_head + 1).to_be_bytes());
            let checkpoint = fixture
                .directory
                .path()
                .join("partial-before-source-rollback");
            std::fs::write(&checkpoint, saved).unwrap();
            let jobs = carol.stop().await;
            let carol = EndpointActor::start(
                Arc::clone(&fixture),
                2,
                Arc::clone(&nodes.networks[2]),
                jobs,
            );
            let restored = SyncCursor::from_bytes(&std::fs::read(&checkpoint).unwrap()).unwrap();
            assert!(!restored.is_complete());
            let stale = carol.sync_result(1, budget(32), Some(restored)).await;
            assert_eq!(stale.unwrap_err(), "Remote(RestartInventory)");
            assert_eq!(carol.change_head().await, before);
            for record in records {
                assert!(carol.body(record).await.is_some());
            }

            // Discarding only the obsolete cursor recovers the missing receipt;
            // already committed records and the first receipt remain idempotent.
            let reset = carol.sync(1, budget(32), None).await;
            assert_eq!(reset.accepted_records, 0);
            assert_eq!(reset.accepted_receipts, 1);
            assert_eq!(reset.fetched, 1);
            assert_eq!(reset.already_known, 3);
            assert!(!reset.has_more());
            for record in records {
                assert!(carol.progress(record).await.all_received());
            }
            let recovered_head = carol.change_head().await;
            let idle = carol.sync(1, budget(32), reset.cursor).await;
            assert_eq!(idle.requests, 1);
            assert_eq!(idle.fetched, 0);
            assert_eq!(idle.already_known, 0);
            assert_eq!(carol.change_head().await, recovered_head);
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            let _ = carol.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn push_returns_durable_receipt_on_same_link_and_exact_retries_survive_reopen() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            // A short bounded outer timeout makes deliberate response loss finish
            // promptly; no transfer failure is interpreted as acceptance.
            nodes.networks[0] = Arc::new(
                (*nodes.networks[0])
                    .clone()
                    .with_sync_policy(prns_groups_core::sync::SyncPolicy {
                        min_exchange_millis: 1_000,
                        max_exchange_millis: 5_000,
                        ..Default::default()
                    })
                    .unwrap(),
            );
            let alice = EndpointActor::start_mode(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
                true,
            );
            let bob = EndpointActor::start_mode(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
                true,
            );
            let first = alice.publish(91, CHAT, b"prompt push").await;
            let admitted = nodes.state.jobs[1].load(Ordering::SeqCst);
            assert!(alice
                .push(
                    1,
                    first,
                    SyncBudget {
                        max_bytes: 1,
                        ..budget(1)
                    }
                )
                .await
                .unwrap_err()
                .contains("Oversized"));
            assert_eq!(
                nodes.state.jobs[1].load(Ordering::SeqCst),
                admitted,
                "hard byte rejection happens before network I/O"
            );
            let report = alice.push(1, first, budget(1)).await.unwrap();
            assert!(report.supported);
            assert_eq!(report.requests, 2);
            assert_eq!(report.submitted, 1);
            assert_eq!(report.accepted_receipts, 1);
            assert_eq!(alice.progress(first).await.received, 1);
            assert_eq!(
                bob.body(first).await.as_deref(),
                Some(b"prompt push".as_slice())
            );
            {
                let links = bob.links.lock().unwrap();
                assert_eq!(links.len(), 2);
                assert_eq!((links[0].0, links[1].0), (4, 5));
                assert_eq!(
                    links[0].1, links[1].1,
                    "capability and Submit use exactly one Link"
                );
            }
            assert!(
                alice.links.lock().unwrap().is_empty(),
                "receipt was a response, not a reciprocal Link"
            );
            let head = bob.change_head().await;
            let repeated = alice.push(1, first, budget(1)).await.unwrap();
            assert_eq!(repeated.accepted_receipts, 1);
            assert_eq!(
                bob.change_head().await,
                head,
                "duplicate ciphertext creates no new record/receipt"
            );

            let lost = alice.publish(92, CHAT, b"response lost after commit").await;
            bob.commands
                .send(Command::LoseNextReply(Arc::clone(&nodes.state)))
                .await
                .unwrap();
            assert!(alice.push(1, lost, budget(1)).await.is_err());
            assert_eq!(alice.progress(lost).await.received, 0);
            assert_eq!(
                bob.body(lost).await.as_deref(),
                Some(b"response lost after commit".as_slice())
            );
            let head = bob.change_head().await;
            let jobs = bob.stop().await;
            nodes.state.set_online(1, true);
            let bob = EndpointActor::start_mode(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                jobs,
                true,
            );
            let recovered = alice.push(1, lost, budget(1)).await.unwrap();
            assert_eq!(recovered.accepted_receipts, 1);
            assert_eq!(alice.progress(lost).await.received, 1);
            assert_eq!(
                bob.change_head().await,
                head,
                "reopen uses the exact durable receipt"
            );
            assert_eq!(nodes.state.unsolicited_announces.load(Ordering::SeqCst), 0);
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pull_returns_receipt_on_fetch_link_within_four_request_budget() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            let alice = EndpointActor::start_mode(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
                true,
            );
            let bob = EndpointActor::start_mode(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
                true,
            );
            let record = alice.publish(93, CHAT, b"recovery pull receipt").await;
            let report = bob
                .sync(
                    0,
                    SyncBudget {
                        max_requests: 4,
                        ..budget(1)
                    },
                    None,
                )
                .await;
            assert_eq!(report.push_supported, Some(true));
            assert_eq!(report.requests, 4);
            assert_eq!(report.accepted_records, 1);
            assert_eq!(
                alice.progress(record).await.received,
                1,
                "no reciprocal polling was scheduled"
            );
            assert!(bob.links.lock().unwrap().is_empty());
            {
                let links = alice.links.lock().unwrap();
                assert_eq!(
                    links.iter().map(|(tag, _)| *tag).collect::<Vec<_>>(),
                    [4, 2, 3, 5]
                );
                assert!(links.iter().all(|(_, link)| *link == links[0].1));
            }
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_peer_declines_push_without_receipt_and_remains_pull_compatible() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            let alice = EndpointActor::start_mode(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
                true,
            );
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
            );
            let record = alice.publish(94, CHAT, b"old peer recovery").await;
            let report = alice.push(1, record, budget(1)).await.unwrap();
            assert!(!report.supported);
            assert_eq!(report.requests, 1);
            assert_eq!(report.submitted, 0);
            assert_eq!(alice.progress(record).await.received, 0);
            assert!(bob.body(record).await.is_none());
            let fetched = bob.sync(0, budget(1), None).await;
            assert_eq!(fetched.accepted_records, 1);
            assert_eq!(
                alice.progress(record).await.received,
                0,
                "legacy pull generates no immediate response receipt"
            );
            let repaired = alice.sync(1, budget(8), None).await;
            assert_eq!(repaired.push_supported, Some(false));
            assert_eq!(repaired.accepted_receipts, 1);
            assert_eq!(alice.progress(record).await.received, 1);
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn submit_rejects_wrong_group_bad_hash_truncation_and_unauthorized_identity() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            let alice = EndpointActor::start_mode(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
                true,
            );
            let before = alice.change_head().await;
            let connection = nodes.networks[1]
                .connect(nodes.networks[1].peer(endpoint(0)).unwrap())
                .await
                .unwrap();
            let mut frame = b"PGT\x01\x05".to_vec();
            frame.extend_from_slice(&GROUP.0);
            frame.extend_from_slice(&[0; 32]); // Deliberately not the ciphertext hash.
            frame.extend_from_slice(&3_u32.to_be_bytes());
            frame.extend_from_slice(&0_u32.to_be_bytes());
            frame.extend_from_slice(b"bad");
            assert_eq!(
                connection.request(&frame).await.unwrap(),
                b"PGT\x01\x00\x03"
            );
            let mut wrong_group = frame.clone();
            wrong_group[5] ^= 1;
            assert_eq!(
                connection.request(&wrong_group).await.unwrap(),
                b"PGT\x01\x00\x02"
            );
            assert_eq!(
                connection.request(&frame[..frame.len() - 1]).await.unwrap(),
                b"PGT\x01\x00\x03"
            );
            let mut oversized = vec![0; support::wire::limits().max_frame_bytes + 1];
            oversized[..frame.len()].copy_from_slice(&frame);
            assert!(matches!(
                connection.request(&oversized).await,
                Err(prns_groups_reticulum::NetworkError::FrameTooLarge)
            ));
            assert_eq!(alice.change_head().await, before);
            connection.close().await.unwrap();
            let admitted = nodes.state.jobs[0].load(Ordering::SeqCst);
            let stranger = nodes.networks[3]
                .connect(nodes.networks[3].peer(endpoint(0)).unwrap())
                .await
                .unwrap();
            assert!(stranger.request(&frame).await.is_err());
            assert_eq!(nodes.state.jobs[0].load(Ordering::SeqCst), admitted);
            stranger.close().await.unwrap();
            assert_eq!(alice.change_head().await, before);
            let _ = alice.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn silent_legacy_group_dispatcher_uses_core_timeout_then_pull_fallback() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = Arc::new(Trio::enrolled());
            let mut nodes = support::wire::Nodes::open().await;
            nodes.networks[0] = Arc::new(
                (*nodes.networks[0])
                    .clone()
                    .with_sync_policy(prns_groups_core::sync::SyncPolicy {
                        min_exchange_millis: 60_000,
                        max_exchange_millis: 60_000,
                        ..Default::default()
                    })
                    .unwrap(),
            );
            let alice = EndpointActor::start_mode(
                Arc::clone(&fixture),
                0,
                Arc::clone(&nodes.networks[0]),
                nodes.jobs[0].take().unwrap(),
                true,
            );
            let bob = EndpointActor::start(
                Arc::clone(&fixture),
                1,
                Arc::clone(&nodes.networks[1]),
                nodes.jobs[1].take().unwrap(),
            );
            bob.commands
                .send(Command::IgnoreCapabilities)
                .await
                .unwrap();
            let record = alice.publish(95, CHAT, b"silent legacy dispatcher").await;
            let started = std::time::Instant::now();
            let report = alice.push(1, record, budget(1)).await.unwrap();
            assert!(
                started.elapsed() < std::time::Duration::from_secs(50),
                "core expiry wins before the 60-second outer timer"
            );
            assert!(!report.supported);
            assert_eq!(report.submitted, 0);
            assert_eq!(alice.progress(record).await.received, 0);
            assert_eq!(bob.sync(0, budget(1), None).await.accepted_records, 1);
            let recovered = alice.sync(1, budget(8), None).await;
            assert_eq!(recovered.push_supported, Some(false));
            assert_eq!(recovered.accepted_receipts, 1);
            assert_eq!(alice.progress(record).await.received, 1);
            let _ = alice.stop().await;
            let _ = bob.stop().await;
            nodes.stop().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn misrouted_submit_never_enters_mutable_storage() {
    use prns_groups_reticulum::{Error, SyncStorage};
    use prns_groups_service::{
        Endpoint, EnvelopeId, EnvelopeInfo, GroupId, Incoming, InventoryCursor, InventoryKind,
        InventoryPage, InventorySnapshot,
    };
    struct CountStorage {
        received: usize,
    }
    impl SyncStorage for CountStorage {
        type Error = ();
        fn group(&self) -> GroupId {
            GROUP
        }
        fn local_endpoint(&self) -> Endpoint {
            endpoint(0)
        }
        fn is_reader(&self, _: Endpoint) -> bool {
            true
        }
        async fn snapshot(&mut self) -> Result<Option<InventorySnapshot>, Error<()>> {
            panic!("misrouted")
        }
        async fn inventory(
            &mut self,
            _: InventorySnapshot,
            _: InventoryCursor,
            _: InventoryKind,
            _: Endpoint,
            _: usize,
        ) -> Result<InventoryPage, Error<()>> {
            panic!("misrouted")
        }
        async fn envelope(&mut self, _: EnvelopeId) -> Result<Option<Vec<u8>>, Error<()>> {
            panic!("misrouted")
        }
        async fn known(
            &mut self,
            _: EnvelopeInfo,
            _: prns_groups_store::StoreIncarnation,
        ) -> Result<bool, Error<()>> {
            panic!("misrouted")
        }
        async fn receive(&mut self, _: &[u8], _: SyncOptions) -> Result<Incoming, Error<()>> {
            self.received += 1;
            Err(Error::InvalidPeer)
        }
        fn envelope_id(&self, bytes: &[u8]) -> EnvelopeId {
            prns_groups_service::envelope_id(GROUP, bytes)
        }
    }
    let nodes = support::wire::Nodes::open().await;
    let destination = nodes.owners[0].client().destination_hashes().unwrap()[0];
    let identity = nodes.owners[1].client().identity_hash().unwrap();
    let mut frame = b"PGT\x01\x05".to_vec();
    frame.extend_from_slice(&GROUP.0);
    frame.extend_from_slice(&prns_groups_service::envelope_id(GROUP, b"opaque").0);
    frame.extend_from_slice(&6_u32.to_be_bytes());
    frame.extend_from_slice(&0_u32.to_be_bytes());
    frame.extend_from_slice(b"opaque");
    let mut packed = vec![0xc4, frame.len() as u8];
    packed.extend_from_slice(&frame);
    for wrong_path in [false, true] {
        let addressed = if wrong_path {
            destination
        } else {
            prns_host::DestinationHash::new([99; 16])
        };
        let (ingress, mut jobs) = if wrong_path {
            let (ingress, jobs) = prns_groups_reticulum::enrollment::Ingress::new(
                addressed,
                support::wire::enrollment_limits(),
            )
            .unwrap();
            (None, (Some(ingress), jobs))
        } else {
            let (ingress, jobs) =
                prns_groups_reticulum::Ingress::new(addressed, support::wire::limits()).unwrap();
            (Some(ingress), (None, jobs))
        };
        let path = if wrong_path {
            prns_groups_reticulum::enrollment::REQUEST_PATH
        } else {
            prns_groups_reticulum::REQUEST_PATH
        };
        let event =
            personal_rns::runtime::PrnsEvent::Message(personal_rns::runtime::Message::Request {
                destination: personal_rns::wire::DestinationHash::new(addressed.into_bytes()),
                link_id: personal_rns::routing::links::LinkId::new([11; 16]),
                request_id: personal_rns::routing::links::request::RequestId([12; 16]),
                requester: Some(personal_rns::identity::IdentityHash::new(
                    identity.into_bytes(),
                )),
                path_hash: personal_rns::routing::request_handlers::RequestPathHash::of(path),
                requested_at: personal_rns::engine::InstantMillis(0),
                rtt: personal_rns::units::RttMillis::new(1),
                data: &packed,
            });
        if let Some(ingress) = ingress {
            ingress.on_event(event);
        } else {
            jobs.0.as_ref().unwrap().on_event_ref(&event);
        }
        let job = jobs.1.recv().await.unwrap();
        let mut storage = CountStorage { received: 0 };
        assert!(matches!(
            prns_groups_reticulum::serve_storage_with_options(
                &nodes.networks[0],
                &mut storage,
                &job,
                SyncOptions {
                    incarnation: incarnation(0),
                    clock: clock(),
                    budget: budget(1)
                }
            )
            .await,
            Err(Error::Network(
                prns_groups_reticulum::NetworkError::UnknownPeer
            ))
        ));
        assert_eq!(storage.received, 0);
    }
    nodes.stop().await;
}
