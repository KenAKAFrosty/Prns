use super::*;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;

use personal_rns::identity::{IdentityHash, PrivateIdentityMaterial, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::routing::announce::derive_single_destination_hash;
use personal_rns::routing::delivery::{Delivery, LinkDelivery};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{Message, PrnsEvent};
use personal_rns::units::InstantMillis;
use personal_rns::wire::DestinationHash;
use prns_lxmf_wire::{compose_basic_direct_lxmf, encode_current_lxmf_announce, BasicLxmfSigner};
use tokio::sync::Notify;

use crate::direct::{
    CallbackOutcome, DirectDeliveryReceipt, DirectNetworkFuture, MAX_ANNOUNCE_APP_DATA_BYTES,
};

const LOCAL_SECRET: [u8; IDENTITY_SECRET_KEY_LEN] = [0x31; IDENTITY_SECRET_KEY_LEN];
const PEER_SECRET: [u8; IDENTITY_SECRET_KEY_LEN] = [0x52; IDENTITY_SECRET_KEY_LEN];
const TEST_INTERFACE: InterfaceId = InterfaceId::new([4, 1, 2, 3, 4, 5, 6, 7]);

struct Signer;

impl BasicLxmfSigner for Signer {
    fn sign_lxmf(&self, input: &[u8]) -> [u8; 64] {
        let mut signature = [0_u8; 64];
        for (index, byte) in input.iter().enumerate() {
            signature[index % signature.len()] ^= *byte;
        }
        signature
    }
}

struct DatabaseSubmitter {
    database: Arc<Database>,
}

impl MailboxSubmitter for DatabaseSubmitter {
    fn submit(&self, request: MailboxRequest) -> MailboxFuture<'_> {
        Box::pin(async move { execute_mailbox_request(&self.database, request) })
    }
}

struct TransientCompletionSubmitter {
    database: Arc<Database>,
    remaining_failures: AtomicUsize,
    completion_submissions: AtomicUsize,
}

impl MailboxSubmitter for TransientCompletionSubmitter {
    fn submit(&self, request: MailboxRequest) -> MailboxFuture<'_> {
        Box::pin(async move {
            if matches!(&request, MailboxRequest::CompleteAttempt { .. }) {
                self.completion_submissions.fetch_add(1, Ordering::AcqRel);
                if {
                    let mut previous = self.remaining_failures.load(Ordering::Acquire);
                    loop {
                        let Some(next) = previous.checked_sub(1) else {
                            break Err(previous);
                        };
                        match self.remaining_failures.compare_exchange_weak(
                            previous,
                            next,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ) {
                            Ok(previous) => break Ok(previous),
                            Err(current) => previous = current,
                        }
                    }
                }
                .is_ok()
                {
                    return Err(MailboxFailure::Busy);
                }
            }
            execute_mailbox_request(&self.database, request)
        })
    }
}

#[derive(Clone, Copy)]
enum RejectedWrite {
    CompleteAttempt,
    InsertInbound,
}

struct PersistentWriteFailureSubmitter {
    database: Arc<Database>,
    rejected: RejectedWrite,
    failure: MailboxFailure,
}

impl MailboxSubmitter for PersistentWriteFailureSubmitter {
    fn submit(&self, request: MailboxRequest) -> MailboxFuture<'_> {
        Box::pin(async move {
            let rejected = matches!(
                (&self.rejected, &request),
                (
                    RejectedWrite::CompleteAttempt,
                    MailboxRequest::CompleteAttempt { .. }
                ) | (
                    RejectedWrite::InsertInbound,
                    MailboxRequest::InsertInbound(_)
                )
            );
            if rejected {
                return Err(self.failure.clone());
            }
            execute_mailbox_request(&self.database, request)
        })
    }
}

struct GatedWriteFailureSubmitter {
    database: Arc<Database>,
    rejected: RejectedWrite,
    failure: MailboxFailure,
    entered: Notify,
    release: Notify,
}

impl MailboxSubmitter for GatedWriteFailureSubmitter {
    fn submit(&self, request: MailboxRequest) -> MailboxFuture<'_> {
        Box::pin(async move {
            let rejected = matches!(
                (&self.rejected, &request),
                (
                    RejectedWrite::CompleteAttempt,
                    MailboxRequest::CompleteAttempt { .. }
                ) | (
                    RejectedWrite::InsertInbound,
                    MailboxRequest::InsertInbound(_)
                )
            );
            if rejected {
                self.entered.notify_one();
                self.release.notified().await;
                return Err(self.failure.clone());
            }
            execute_mailbox_request(&self.database, request)
        })
    }
}

struct AlwaysBusySubmitter;

impl MailboxSubmitter for AlwaysBusySubmitter {
    fn submit(&self, _request: MailboxRequest) -> MailboxFuture<'_> {
        Box::pin(async { Err(MailboxFailure::Busy) })
    }
}

struct FixedClock(u64);

impl MailboxClock for FixedClock {
    fn now_unix_millis(&self) -> u64 {
        self.0
    }
}

struct DurableFakeNetwork {
    has_route: AtomicBool,
    path_requests: AtomicUsize,
    path_requested: Notify,
    results: StdMutex<VecDeque<Result<DirectDeliveryReceipt, DirectSendFailure>>>,
    public_keys: StdMutex<BTreeMap<[u8; 16], [u8; 64]>>,
    used_destinations: StdMutex<Vec<[u8; 16]>>,
    block_retention: AtomicBool,
    retention_entered: Notify,
    retention_released: Notify,
    sent_wires: StdMutex<Vec<Vec<u8>>>,
    block_send: AtomicBool,
    send_entered: Notify,
    send_released: Notify,
    send_future_dropped: AtomicBool,
}

struct DropSignal<'a>(&'a AtomicBool);

impl Drop for DropSignal<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Default for DurableFakeNetwork {
    fn default() -> Self {
        Self {
            has_route: AtomicBool::new(true),
            path_requests: AtomicUsize::new(0),
            path_requested: Notify::new(),
            results: StdMutex::new(VecDeque::from([Ok(DirectDeliveryReceipt {
                rtt_millis: 23,
            })])),
            public_keys: StdMutex::new(BTreeMap::new()),
            used_destinations: StdMutex::new(Vec::new()),
            block_retention: AtomicBool::new(false),
            retention_entered: Notify::new(),
            retention_released: Notify::new(),
            sent_wires: StdMutex::new(Vec::new()),
            block_send: AtomicBool::new(false),
            send_entered: Notify::new(),
            send_released: Notify::new(),
            send_future_dropped: AtomicBool::new(false),
        }
    }
}

impl DurableFakeNetwork {
    fn release_send(&self) {
        self.block_send.store(false, Ordering::Release);
        self.send_released.notify_waiters();
    }

    fn add_public_key(&self, destination: [u8; 16], material: &PrivateIdentityMaterial) {
        self.public_keys
            .lock()
            .unwrap()
            .insert(destination, *material.public().as_bytes());
    }
}

impl DirectNetwork for DurableFakeNetwork {
    fn has_route(&self, _destination: [u8; 16]) -> DirectNetworkFuture<'_, bool> {
        Box::pin(async move { self.has_route.load(Ordering::Acquire) })
    }

    fn request_path(
        &self,
        _destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>> {
        Box::pin(async move {
            self.path_requests.fetch_add(1, Ordering::AcqRel);
            self.path_requested.notify_one();
            Ok(())
        })
    }

    fn establish_link(
        &self,
        _destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<[u8; 16], DirectSendFailure>> {
        Box::pin(async move { Ok([0xa5; 16]) })
    }

    fn send_link_packet(
        &self,
        _link: [u8; 16],
        complete_wire: Vec<u8>,
    ) -> DirectNetworkFuture<'_, Result<DirectDeliveryReceipt, DirectSendFailure>> {
        Box::pin(async move {
            let _drop_signal = DropSignal(&self.send_future_dropped);
            self.sent_wires.lock().unwrap().push(complete_wire);
            self.send_entered.notify_one();
            while self.block_send.load(Ordering::Acquire) {
                self.send_released.notified().await;
            }
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(DirectDeliveryReceipt { rtt_millis: 23 }))
        })
    }

    fn destination_public_key(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Option<[u8; 64]>> {
        Box::pin(async move { self.public_keys.lock().unwrap().get(&destination).copied() })
    }

    fn announce(
        &self,
        _destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectAnnounceFailure>> {
        Box::pin(async move { Ok(()) })
    }

    fn mark_destination_used(&self, destination: [u8; 16]) -> DirectNetworkFuture<'_, ()> {
        Box::pin(async move {
            self.used_destinations.lock().unwrap().push(destination);
            self.retention_entered.notify_one();
            while self.block_retention.load(Ordering::Acquire) {
                self.retention_released.notified().await;
            }
        })
    }
}

fn open_database() -> (tempfile::TempDir, Database) {
    let root = tempfile::tempdir().unwrap();
    let database = Database::create(root.path().join("application.redb")).unwrap();
    let write = database.begin_write().unwrap();
    initialize_mailbox_tables(&write).unwrap();
    write.commit().unwrap();
    (root, database)
}

fn shared_database() -> (tempfile::TempDir, Arc<Database>, Arc<DatabaseSubmitter>) {
    let (root, database) = open_database();
    let database = Arc::new(database);
    let submitter = Arc::new(DatabaseSubmitter {
        database: database.clone(),
    });
    (root, database, submitter)
}

fn peer_facts() -> (PrivateIdentityMaterial, [u8; 16]) {
    let material = PrivateIdentityMaterial::from_bytes(PEER_SECRET);
    let destination = derive_single_destination_hash(
        &material.identity_hash(),
        prns_lxmf_wire::LXMF_APP_NAME,
        prns_lxmf_wire::LXMF_DELIVERY_ASPECTS,
    )
    .unwrap();
    (material, *destination.as_bytes())
}

fn authenticated_observation<'a>(
    destination: [u8; 16],
    identity: IdentityHash,
    app_data: &'a [u8],
) -> prns_host_runtime::AuthenticatedAnnounce<'a> {
    prns_host_runtime::AuthenticatedAnnounce {
        destination: prns_host::DestinationHash::new(destination),
        announced_identity: prns_host::IdentityHash::new(*identity.as_bytes()),
        source_interface: prns_host::InterfaceId::new(*TEST_INTERFACE.as_bytes()),
        hops: 2,
        arrived_at_millis: 100,
        app_data,
        is_path_response: false,
    }
}

fn current_announce() -> Vec<u8> {
    let mut output = [0_u8; MAX_ANNOUNCE_APP_DATA_BYTES];
    let len = encode_current_lxmf_announce(b"peer", &mut output).unwrap();
    output[..len].to_vec()
}

fn link_event(wire: &[u8]) -> PrnsEvent<'_> {
    PrnsEvent::Message(Message::Delivered(Delivery::Link(LinkDelivery {
        link_id: LinkId::new([0xc7; 16]),
        local_destination: Some(DestinationHash::new(
            wire[..16].try_into().expect("fixture carries destination"),
        )),
        plaintext: wire,
        arrived_at: InstantMillis(200),
        source_interface: TEST_INTERFACE,
    })))
}

fn all_messages() -> MailboxListRequest {
    MailboxListRequest {
        peer: None,
        direction: None,
        before: None,
        limit: 100,
    }
}

async fn wait_for_durable_snapshot(
    service: &DurableDirectLxmfService,
    predicate: impl Fn(&DurableLxmfSnapshot) -> bool,
) -> DurableLxmfSnapshot {
    for _ in 0..100 {
        let snapshot = service.snapshot(all_messages()).await.unwrap();
        if predicate(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("durable snapshot predicate did not settle");
}

async fn start_durable(
    network: Arc<DurableFakeNetwork>,
    submitter: Arc<DatabaseSubmitter>,
) -> DurableDirectLxmfService {
    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, callbacks) = DurableDirectLxmfService::prepare(identity);
    let service = pending
        .start(network, submitter, Arc::new(FixedClock(9_999)))
        .await
        .unwrap();
    assert_eq!(
        service.callbacks().local_destination,
        callbacks.local_destination
    );
    service
}

async fn learn_durable_peer(service: &DurableDirectLxmfService) -> [u8; 16] {
    let (peer, destination) = peer_facts();
    let announce = current_announce();
    assert_eq!(
        service
            .callbacks()
            .on_authenticated_announce(&authenticated_observation(
                destination,
                peer.identity_hash(),
                &announce,
            )),
        CallbackOutcome::Enqueued
    );
    wait_for_durable_snapshot(service, |snapshot| !snapshot.peers.is_empty()).await;
    destination
}

fn outbound(timestamp: u64) -> NewOutboundMessage {
    outbound_between(timestamp, [0x11; 16], [0x22; 16])
}

fn outbound_between(timestamp: u64, source: [u8; 16], destination: [u8; 16]) -> NewOutboundMessage {
    let title = b"title".to_vec();
    let content = b"content".to_vec();
    let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let prepared = compose_basic_direct_lxmf(
        destination,
        source,
        timestamp,
        &title,
        &content,
        None,
        &Signer,
        &mut output,
    )
    .unwrap();
    NewOutboundMessage {
        message_id: prepared.message_id(),
        source,
        destination,
        timestamp_unix_ms: timestamp,
        title,
        content,
        exact_wire: output[..usize::from(prepared.wire_len())].to_vec(),
    }
}

fn inbound(message: NewOutboundMessage, verification: LxmfVerification) -> NewInboundMessage {
    NewInboundMessage {
        message_id: message.message_id,
        source: message.source,
        destination: message.destination,
        timestamp_unix_ms: message.timestamp_unix_ms,
        title: message.title,
        content: message.content,
        exact_wire: message.exact_wire,
        verification,
    }
}

fn listed(database: &Database, request: MailboxListRequest) -> (Vec<DurableLxmfMessage>, u64) {
    let MailboxReply::Listed { messages, revision } =
        execute_mailbox_request(database, MailboxRequest::List(request)).unwrap()
    else {
        panic!("unexpected list reply");
    };
    (messages, revision)
}

fn conversations(
    database: &Database,
    before: Option<u64>,
    limit: usize,
) -> (Vec<DurableLxmfMessage>, u64) {
    let MailboxReply::ConversationsListed { messages, revision } = execute_mailbox_request(
        database,
        MailboxRequest::ListConversations(MailboxConversationListRequest { before, limit }),
    )
    .unwrap() else {
        panic!("unexpected conversation reply");
    };
    (messages, revision)
}

fn inserted_outbound(database: &Database, timestamp: u64) -> DurableLxmfMessage {
    match execute_mailbox_request(
        database,
        MailboxRequest::InsertOutbound(outbound(timestamp)),
    )
    .unwrap()
    {
        MailboxReply::OutboundInserted { message, .. } => message,
        other => panic!("unexpected reply: {other:?}"),
    }
}

#[test]
fn accepted_wire_reopens_as_queued_and_validates_exactly() {
    let (root, database) = open_database();
    let message = inserted_outbound(&database, 1_700_000_000_123);
    assert_eq!(message.local_record_id, 1);
    assert_eq!(message.generation(), Some(1));
    assert_eq!(
        message.delivery_state,
        DurableLxmfDeliveryState::Queued { failed_attempts: 0 }
    );
    let exact_wire = message.exact_wire.clone();
    drop(database);

    let reopened = Database::open(root.path().join("application.redb")).unwrap();
    validate_mailbox_database(&reopened).unwrap();
    let listed = execute_mailbox_request(
        &reopened,
        MailboxRequest::List(MailboxListRequest {
            peer: None,
            direction: None,
            before: None,
            limit: 10,
        }),
    )
    .unwrap();
    let MailboxReply::Listed { messages, revision } = listed else {
        panic!("unexpected list reply");
    };
    assert_eq!(revision, 1);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].exact_wire, exact_wire);
}

#[test]
fn list_cursor_filters_and_limits_are_exact() {
    let (_root, database) = open_database();
    for index in 0..6 {
        let message = outbound_between(1_700_000_001_000 + index, [0x11; 16], [0x22; 16]);
        let request = if index % 2 == 0 {
            MailboxRequest::InsertOutbound(message)
        } else {
            MailboxRequest::InsertInbound(inbound(message, LxmfVerification::Verified))
        };
        execute_mailbox_request(&database, request).unwrap();
    }
    let ids = |request| {
        listed(&database, request)
            .0
            .into_iter()
            .map(|message| message.local_record_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(MailboxListRequest {
            before: Some(6),
            limit: 2,
            ..all_messages()
        }),
        vec![5, 4]
    );
    assert_eq!(
        ids(MailboxListRequest {
            before: Some(6),
            direction: Some(MailboxDirectionFilter::Inbox),
            peer: Some([0x22; 16]),
            limit: 2,
        }),
        vec![4, 2]
    );
    assert_eq!(
        ids(MailboxListRequest {
            direction: Some(MailboxDirectionFilter::Outbox),
            peer: Some([0x11; 16]),
            limit: 2,
            ..all_messages()
        }),
        vec![5, 3]
    );
    for request in [
        MailboxListRequest {
            limit: 0,
            ..all_messages()
        },
        MailboxListRequest {
            before: Some(0),
            ..all_messages()
        },
        MailboxListRequest {
            before: Some(1),
            ..all_messages()
        },
        MailboxListRequest {
            peer: Some([0x33; 16]),
            ..all_messages()
        },
    ] {
        assert!(ids(request).is_empty());
    }
    let MailboxReply::QueuedPage {
        messages,
        revision,
        next,
    } = execute_mailbox_request(
        &database,
        MailboxRequest::Replay(MailboxReplayRequest {
            after: None,
            limit: 100,
        }),
    )
    .unwrap()
    else {
        panic!("unexpected list reply");
    };
    assert_eq!(revision, 6);
    assert_eq!(next, None);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.local_record_id)
            .collect::<Vec<_>>(),
        vec![1, 3, 5]
    );
}

#[test]
fn bounded_list_does_not_decode_records_outside_its_page() {
    let (_root, database) = open_database();
    inserted_outbound(&database, 1_700_000_001_000);
    inserted_outbound(&database, 1_700_000_001_001);
    let write = database.begin_write().unwrap();
    write
        .open_table(LXMF_MESSAGES)
        .unwrap()
        .insert(1, b"malformed".as_slice())
        .unwrap();
    write.commit().unwrap();
    assert!(listed(
        &database,
        MailboxListRequest {
            limit: 0,
            ..all_messages()
        }
    )
    .0
    .is_empty());
    assert_eq!(
        listed(
            &database,
            MailboxListRequest {
                limit: 1,
                ..all_messages()
            }
        )
        .0[0]
            .local_record_id,
        2
    );
    assert!(listed(
        &database,
        MailboxListRequest {
            before: Some(1),
            ..all_messages()
        }
    )
    .0
    .is_empty());
    assert!(execute_mailbox_request(
        &database,
        MailboxRequest::List(MailboxListRequest {
            before: Some(2),
            limit: 1,
            ..all_messages()
        })
    )
    .is_err());
}

#[test]
fn conversation_pages_include_old_peers_without_repeating_newer_conversations() {
    let (_root, database) = open_database();
    let local = [0x11; 16];
    let older_peer = [0x33; 16];
    let active_peer = [0x22; 16];
    let newest_peer = [0x44; 16];
    let insert = |timestamp, source, destination, incoming| {
        let message = outbound_between(timestamp, source, destination);
        let request = if incoming {
            MailboxRequest::InsertInbound(inbound(message, LxmfVerification::Verified))
        } else {
            MailboxRequest::InsertOutbound(message)
        };
        execute_mailbox_request(&database, request).unwrap();
    };
    insert(1_700_000_001_000, local, newest_peer, false);
    insert(1_700_000_001_001, local, older_peer, false);
    for index in 2..108 {
        insert(1_700_000_001_000 + index, local, active_peer, false);
    }
    // The latest active peer row is inbound; its peer is source, not local.
    insert(1_700_000_001_108, active_peer, local, true);
    insert(1_700_000_001_109, local, newest_peer, false);
    let (first, revision) = conversations(&database, None, 1);
    assert_eq!(revision, 110);
    assert_eq!(first[0].local_record_id, 110);
    let (second, _) = conversations(&database, Some(110), 1);
    assert_eq!(second[0].local_record_id, 109);
    let (third, _) = conversations(&database, Some(109), 1);
    assert_eq!(third[0].local_record_id, 2);
    assert!(conversations(&database, Some(2), 1).0.is_empty());
    assert!(conversations(&database, None, 0).0.is_empty());
    assert!(conversations(&database, Some(0), 10).0.is_empty());
    assert_eq!(
        conversations(&database, None, 10)
            .0
            .iter()
            .map(|message| message.local_record_id)
            .collect::<Vec<_>>(),
        vec![110, 109, 2]
    );
    // A new row moves the old peer ahead of the existing cursor; it must not
    // reappear below it. A fresh first-page query finds the new ordering.
    insert(1_700_000_001_110, older_peer, local, true);
    assert!(conversations(&database, Some(109), 10).0.is_empty());
    assert_eq!(conversations(&database, None, 1).0[0].local_record_id, 111);
    validate_mailbox_database(&database).unwrap();
}

#[test]
fn retry_preserves_wire_and_message_id_and_rejects_old_generation() {
    let (_root, database) = open_database();
    let original = inserted_outbound(&database, 1_700_000_000_456);
    let first_key = AttemptKey {
        local_record_id: original.local_record_id,
        generation: original.generation().unwrap(),
    };
    let failed = execute_mailbox_request(
        &database,
        MailboxRequest::CompleteAttempt {
            key: first_key,
            completion: AttemptCompletion::Failed {
                failure: DirectSendFailure::DeliveryTimedOut,
            },
        },
    )
    .unwrap();
    assert!(matches!(
        failed,
        MailboxReply::Completion(CompletionTransition::Applied {
            message: DurableLxmfMessage {
                delivery_state: DurableLxmfDeliveryState::Failed {
                    failed_attempts: 1,
                    last_failure: DirectSendFailure::DeliveryTimedOut,
                },
                ..
            },
            ..
        })
    ));

    let retried = execute_mailbox_request(
        &database,
        MailboxRequest::Retry {
            local_record_id: original.local_record_id,
        },
    )
    .unwrap();
    let MailboxReply::Retry(RetryTransition::Accepted { message, .. }) = retried else {
        panic!("retry was not accepted");
    };
    assert_eq!(message.generation(), Some(2));
    assert_eq!(message.message_id, original.message_id);
    assert_eq!(message.exact_wire, original.exact_wire);
    assert_eq!(
        message.delivery_state,
        DurableLxmfDeliveryState::Queued { failed_attempts: 1 }
    );

    let late = execute_mailbox_request(
        &database,
        MailboxRequest::CompleteAttempt {
            key: first_key,
            completion: AttemptCompletion::Delivered {
                delivered_at_millis: 99,
                rtt_millis: Some(7),
            },
        },
    )
    .unwrap();
    assert!(matches!(
        late,
        MailboxReply::Completion(CompletionTransition::Stale {
            current: Some(DurableLxmfDeliveryState::Queued { failed_attempts: 1 })
        })
    ));
}

#[test]
fn proof_and_cancel_are_linearized_by_the_first_transaction() {
    let (_root, database) = open_database();
    let delivered = inserted_outbound(&database, 1_700_000_001_000);
    let key = AttemptKey {
        local_record_id: delivered.local_record_id,
        generation: delivered.generation().unwrap(),
    };
    execute_mailbox_request(
        &database,
        MailboxRequest::CompleteAttempt {
            key,
            completion: AttemptCompletion::Delivered {
                delivered_at_millis: 10,
                rtt_millis: Some(3),
            },
        },
    )
    .unwrap();
    assert_eq!(
        execute_mailbox_request(
            &database,
            MailboxRequest::Cancel {
                local_record_id: delivered.local_record_id,
                cancelled_at_millis: 11,
            },
        )
        .unwrap(),
        MailboxReply::Cancel(CancelTransition::AlreadyDelivered)
    );

    let cancelled = inserted_outbound(&database, 1_700_000_002_000);
    let cancelled_key = AttemptKey {
        local_record_id: cancelled.local_record_id,
        generation: cancelled.generation().unwrap(),
    };
    assert!(matches!(
        execute_mailbox_request(
            &database,
            MailboxRequest::Cancel {
                local_record_id: cancelled.local_record_id,
                cancelled_at_millis: 12,
            },
        )
        .unwrap(),
        MailboxReply::Cancel(CancelTransition::Cancelled { .. })
    ));
    assert!(matches!(
        execute_mailbox_request(
            &database,
            MailboxRequest::CompleteAttempt {
                key: cancelled_key,
                completion: AttemptCompletion::Delivered {
                    delivered_at_millis: 13,
                    rtt_millis: None,
                },
            },
        )
        .unwrap(),
        MailboxReply::Completion(CompletionTransition::Stale {
            current: Some(DurableLxmfDeliveryState::Cancelled {
                cancelled_at_millis: 12
            })
        })
    ));
}

#[test]
fn inbound_verification_upgrade_is_monotonic_transactional_and_survives_reopen() {
    for initial_verification in [
        LxmfVerification::SourceUnknown,
        LxmfVerification::InvalidSignature,
    ] {
        let (root, database) = open_database();
        let verified = inbound(outbound(1_700_000_003_001), LxmfVerification::Verified);
        let mut unverified = verified.clone();
        unverified.verification = initial_verification;
        if initial_verification == LxmfVerification::InvalidSignature {
            unverified.exact_wire[32] ^= 1;
        }
        execute_mailbox_request(&database, MailboxRequest::InsertInbound(unverified.clone()))
            .unwrap();
        let upgraded =
            execute_mailbox_request(&database, MailboxRequest::InsertInbound(verified.clone()))
                .unwrap();
        assert!(matches!(
            upgraded,
            MailboxReply::Inbound(InboundInsertOutcome::VerificationUpgraded {
                message: DurableLxmfMessage {
                    local_record_id: 1,
                    verification: LxmfVerification::Verified,
                    ..
                },
                revision: 2,
            })
        ));
        for duplicate in [verified.clone(), unverified] {
            assert_eq!(
                execute_mailbox_request(&database, MailboxRequest::InsertInbound(duplicate))
                    .unwrap(),
                MailboxReply::Inbound(InboundInsertOutcome::Duplicate { local_record_id: 1 })
            );
        }
        validate_mailbox_database(&database).unwrap();
        drop(database);
        let reopened = Database::open(root.path().join("application.redb")).unwrap();
        validate_mailbox_database(&reopened).unwrap();
        let (messages, revision) = listed(&reopened, all_messages());
        assert_eq!(revision, 2);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].local_record_id, 1);
        assert_eq!(messages[0].exact_wire, verified.exact_wire);
        assert_eq!(messages[0].verification, LxmfVerification::Verified);
        assert_eq!(
            inserted_outbound(&reopened, 1_700_000_003_002).local_record_id,
            2
        );
        validate_mailbox_database(&reopened).unwrap();
    }
}

#[test]
fn upgrade_rejects_mismatched_incoming_fields_without_mutating_the_record() {
    let (_root, database) = open_database();
    let original = inbound(outbound(1_700_000_003_001), LxmfVerification::SourceUnknown);
    execute_mailbox_request(&database, MailboxRequest::InsertInbound(original.clone())).unwrap();
    let mut candidates = Vec::new();
    for field in 0..6 {
        let mut candidate = original.clone();
        candidate.verification = LxmfVerification::Verified;
        match field {
            0 => candidate.source[0] ^= 1,
            1 => candidate.destination[0] ^= 1,
            2 => candidate.timestamp_unix_ms += 1,
            3 => candidate.title.push(0),
            4 => candidate.content.push(0),
            _ => candidate.exact_wire = outbound(1_700_000_003_002).exact_wire,
        }
        candidates.push(candidate);
    }
    for candidate in candidates {
        assert!(
            execute_mailbox_request(&database, MailboxRequest::InsertInbound(candidate)).is_err()
        );
        let (messages, revision) = listed(&database, all_messages());
        assert_eq!(revision, 1);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].verification, LxmfVerification::SourceUnknown);
        assert_eq!(messages[0].exact_wire, original.exact_wire);
    }
    validate_mailbox_database(&database).unwrap();
}

#[test]
fn nonverified_duplicates_do_not_change_verification_or_wire() {
    let (_root, database) = open_database();
    let original = inbound(
        outbound(1_700_000_003_001),
        LxmfVerification::InvalidSignature,
    );
    execute_mailbox_request(&database, MailboxRequest::InsertInbound(original.clone())).unwrap();
    let mut unknown = original.clone();
    unknown.verification = LxmfVerification::SourceUnknown;
    unknown.exact_wire[32] ^= 1;
    assert_eq!(
        execute_mailbox_request(&database, MailboxRequest::InsertInbound(unknown)).unwrap(),
        MailboxReply::Inbound(InboundInsertOutcome::Duplicate { local_record_id: 1 })
    );
    let (messages, revision) = listed(&database, all_messages());
    assert_eq!(revision, 1);
    assert_eq!(messages[0].verification, LxmfVerification::InvalidSignature);
    assert_eq!(messages[0].exact_wire, original.exact_wire);
}

#[test]
fn duplicate_inbound_is_noop_and_does_not_advance_revision() {
    let (_root, database) = open_database();
    let outbound = outbound(1_700_000_003_000);
    let inbound = NewInboundMessage {
        message_id: outbound.message_id,
        source: outbound.source,
        destination: outbound.destination,
        timestamp_unix_ms: outbound.timestamp_unix_ms,
        title: outbound.title,
        content: outbound.content,
        exact_wire: outbound.exact_wire,
        verification: LxmfVerification::SourceUnknown,
    };
    let first =
        execute_mailbox_request(&database, MailboxRequest::InsertInbound(inbound.clone())).unwrap();
    assert!(matches!(
        first,
        MailboxReply::Inbound(InboundInsertOutcome::Inserted { revision: 1, .. })
    ));
    let duplicate =
        execute_mailbox_request(&database, MailboxRequest::InsertInbound(inbound)).unwrap();
    assert_eq!(
        duplicate,
        MailboxReply::Inbound(InboundInsertOutcome::Duplicate { local_record_id: 1 })
    );
    let listed = execute_mailbox_request(
        &database,
        MailboxRequest::List(MailboxListRequest {
            peer: None,
            direction: None,
            before: None,
            limit: 10,
        }),
    )
    .unwrap();
    assert!(matches!(
        listed,
        MailboxReply::Listed {
            revision: 1,
            ref messages
        } if messages.len() == 1
    ));
    validate_mailbox_database(&database).unwrap();
}

#[test]
fn validation_rejects_missing_index_and_exact_wire_drift() {
    let (_root, database) = open_database();
    let outbound = outbound(1_700_000_004_000);
    let inbound = NewInboundMessage {
        message_id: outbound.message_id,
        source: outbound.source,
        destination: outbound.destination,
        timestamp_unix_ms: outbound.timestamp_unix_ms,
        title: outbound.title,
        content: outbound.content,
        exact_wire: outbound.exact_wire,
        verification: LxmfVerification::Verified,
    };
    execute_mailbox_request(&database, MailboxRequest::InsertInbound(inbound)).unwrap();
    let write = database.begin_write().unwrap();
    {
        let mut index = write.open_table(LXMF_INBOUND_IDS).unwrap();
        index.remove([0x22; 32].as_slice()).unwrap();
        let first = index
            .iter()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .0
            .value()
            .to_vec();
        index.remove(first.as_slice()).unwrap();
    }
    write.commit().unwrap();
    assert!(matches!(
        validate_mailbox_database(&database),
        Err(MailboxFailure::ResetRequired(_))
    ));

    let (_root, database) = open_database();
    let message = inserted_outbound(&database, 1_700_000_005_000);
    let write = database.begin_write().unwrap();
    {
        let mut table = write.open_table(LXMF_MESSAGES).unwrap();
        let encoded = table
            .get(message.local_record_id)
            .unwrap()
            .unwrap()
            .value()
            .to_vec();
        let mut record: StoredRecord = serde_json::from_slice(&encoded).unwrap();
        record.content.push(0xff);
        let encoded = serde_json::to_vec(&record).unwrap();
        table
            .insert(message.local_record_id, encoded.as_slice())
            .unwrap();
    }
    write.commit().unwrap();
    assert!(matches!(
        validate_mailbox_database(&database),
        Err(MailboxFailure::ResetRequired(_))
    ));
}

#[test]
fn validation_requires_the_nullable_receipt_field() {
    let (_root, database) = open_database();
    let message = inserted_outbound(&database, 1_700_000_005_500);
    let key = AttemptKey {
        local_record_id: message.local_record_id,
        generation: message.generation().unwrap(),
    };
    execute_mailbox_request(
        &database,
        MailboxRequest::CompleteAttempt {
            key,
            completion: AttemptCompletion::Delivered {
                delivered_at_millis: 10,
                rtt_millis: None,
            },
        },
    )
    .unwrap();

    let write = database.begin_write().unwrap();
    {
        let mut table = write.open_table(LXMF_MESSAGES).unwrap();
        let encoded = table
            .get(message.local_record_id)
            .unwrap()
            .unwrap()
            .value()
            .to_vec();
        let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        value["recordKind"]["state"]
            .as_object_mut()
            .unwrap()
            .remove("rttMillis");
        let encoded = serde_json::to_vec(&value).unwrap();
        table
            .insert(message.local_record_id, encoded.as_slice())
            .unwrap();
    }
    write.commit().unwrap();

    assert!(matches!(
        validate_mailbox_database(&database),
        Err(MailboxFailure::ResetRequired(_))
    ));
}

#[test]
fn validation_rejects_impossible_counter_id_and_generation_history() {
    let (_root, database) = open_database();
    inserted_outbound(&database, 1_700_000_005_600);
    let write = database.begin_write().unwrap();
    {
        let mut meta = write.open_table(LXMF_META).unwrap();
        meta.insert(LXMF_REVISION_KEY, 0).unwrap();
    }
    write.commit().unwrap();
    assert!(matches!(
        validate_mailbox_database(&database),
        Err(MailboxFailure::ResetRequired(_))
    ));

    let (_root, database) = open_database();
    inserted_outbound(&database, 1_700_000_005_700);
    inserted_outbound(&database, 1_700_000_005_800);
    let write = database.begin_write().unwrap();
    {
        let mut messages = write.open_table(LXMF_MESSAGES).unwrap();
        messages.remove(1).unwrap();
    }
    write.commit().unwrap();
    assert!(matches!(
        validate_mailbox_database(&database),
        Err(MailboxFailure::ResetRequired(_))
    ));

    let (_root, database) = open_database();
    let message = inserted_outbound(&database, 1_700_000_005_900);
    let write = database.begin_write().unwrap();
    {
        let mut messages = write.open_table(LXMF_MESSAGES).unwrap();
        let encoded = messages
            .get(message.local_record_id)
            .unwrap()
            .unwrap()
            .value()
            .to_vec();
        let mut record: StoredRecord = serde_json::from_slice(&encoded).unwrap();
        let StoredRecordKind::Outbound { generation, .. } = &mut record.record_kind else {
            panic!("inserted outbound record decoded as inbound");
        };
        *generation = 9;
        let encoded = serde_json::to_vec(&record).unwrap();
        messages
            .insert(message.local_record_id, encoded.as_slice())
            .unwrap();
    }
    write.commit().unwrap();
    assert!(matches!(
        validate_mailbox_database(&database),
        Err(MailboxFailure::ResetRequired(_))
    ));
}

#[tokio::test]
async fn restarted_recipient_resolves_even_with_cached_route_before_durable_admission() {
    let (_root, _database, submitter) = shared_database();
    let first = start_durable(Arc::new(DurableFakeNetwork::default()), submitter.clone()).await;
    let destination = learn_durable_peer(&first).await;
    first.stop().await.unwrap();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .peers
        .is_empty());
    let sending = service.clone();
    let expected = *peer_facts().0.identity_hash().as_bytes();
    let send = tokio::spawn(async move {
        sending
            .send_direct_text_to_identity(
                destination,
                Some(expected),
                1_700_000_000_000,
                b"",
                b"fresh after restart",
            )
            .await
    });
    network.path_requested.notified().await;
    assert_eq!(network.path_requests.load(Ordering::Acquire), 1);
    assert!(
        !send.is_finished(),
        "route settlement is not accepted LXMF metadata"
    );
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .messages
        .is_empty());
    learn_durable_peer(&service).await;
    assert_eq!(
        send.await.unwrap(),
        DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
    );
    wait_for_durable_snapshot(&service, |s| {
        matches!(
            s.messages.first().map(|m| m.delivery_state),
            Some(DurableLxmfDeliveryState::Delivered { .. })
        )
    })
    .await;
    assert_eq!(
        network.path_requests.load(Ordering::Acquire),
        1,
        "known route did not suppress metadata lookup or cause a duplicate lookup"
    );
    assert_eq!(network.sent_wires.lock().unwrap().len(), 1);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn discovery_clear_keeps_saved_recipient_sendable_and_checks_pinned_identity() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    let destination = learn_durable_peer(&service).await;
    service.clear_discovered_peers().await;
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .peers
        .is_empty());
    let observed = *peer_facts().0.identity_hash().as_bytes();
    assert_eq!(
        service
            .send_direct_text_to_identity(
                destination,
                Some([0xff; 16]),
                1_700_000_000_000,
                b"",
                b"conflict"
            )
            .await,
        DurableSendDirectTextOutcome::PeerIdentityConflict {
            expected: [0xff; 16],
            observed
        }
    );
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .messages
        .is_empty());
    assert!(matches!(
        service
            .send_direct_text_to_identity(
                destination,
                Some(observed),
                1_700_000_000_000,
                b"",
                b"after clear"
            )
            .await,
        DurableSendDirectTextOutcome::Accepted { .. }
    ));
    assert_eq!(network.path_requests.load(Ordering::Acquire), 0);
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .peers
        .is_empty());
    service.stop().await.unwrap();
}

#[tokio::test]
async fn authenticated_nameless_announce_is_discovered_and_resolves_the_recipient() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    let callbacks = service.callbacks();
    let (peer, destination) = peer_facts();
    let local = PrivateIdentityMaterial::from_bytes(LOCAL_SECRET);
    assert_eq!(
        callbacks.on_authenticated_announce(&authenticated_observation(
            [0xff; 16],
            peer.identity_hash(),
            &[],
        )),
        CallbackOutcome::InvalidDestinationAssociation
    );
    assert_eq!(
        callbacks.on_authenticated_announce(&authenticated_observation(
            destination,
            local.identity_hash(),
            &[],
        )),
        CallbackOutcome::InvalidDestinationAssociation
    );
    assert_eq!(
        callbacks.on_authenticated_announce(&authenticated_observation(
            service.local_destination(),
            local.identity_hash(),
            &[],
        )),
        CallbackOutcome::Ignored
    );
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .peers
        .is_empty());
    assert_eq!(
        callbacks.on_authenticated_announce(&authenticated_observation(
            destination,
            peer.identity_hash(),
            &[],
        )),
        CallbackOutcome::Enqueued
    );
    let snapshot = wait_for_durable_snapshot(&service, |snapshot| snapshot.peers.len() == 1).await;
    assert_eq!(snapshot.peers[0].destination, destination);
    assert_eq!(
        snapshot.peers[0].announced_identity,
        *peer.identity_hash().as_bytes()
    );
    assert_eq!(snapshot.peers[0].display_name, None);
    assert_eq!(snapshot.peers[0].required_stamp_cost, None);
    assert_eq!(
        snapshot.peers[0].source_interface,
        *TEST_INTERFACE.as_bytes()
    );
    assert!(matches!(
        service
            .send_direct_text_to_identity(
                destination,
                Some(*peer.identity_hash().as_bytes()),
                1_700_000_000_000,
                b"",
                b"nameless recipient",
            )
            .await,
        DurableSendDirectTextOutcome::Accepted { .. }
    ));
    assert_eq!(network.path_requests.load(Ordering::Acquire), 0);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn newly_resolved_stamp_requirement_is_checked_before_any_message_is_accepted() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    let (peer, destination) = peer_facts();
    let sending = service.clone();
    let send = tokio::spawn(async move {
        sending
            .send_direct_text(destination, 1_700_000_000_000, b"", b"no stamp")
            .await
    });
    network.path_requested.notified().await;
    let stamp = [0x93, 0xc4, 0x04, b's', b't', b'a', b'm', 0x05, 0x90];
    assert_eq!(
        service
            .callbacks()
            .on_authenticated_announce(&authenticated_observation(
                destination,
                peer.identity_hash(),
                &stamp
            )),
        CallbackOutcome::Enqueued
    );
    assert_eq!(
        send.await.unwrap(),
        DurableSendDirectTextOutcome::UnsupportedRemoteStampRequirement {
            required_stamp_cost: 5
        }
    );
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .messages
        .is_empty());
    assert!(network.sent_wires.lock().unwrap().is_empty());
    service.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn missing_recipient_lookup_times_out_without_creating_a_queued_message() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    let sending = service.clone();
    let send = tokio::spawn(async move {
        sending
            .send_direct_text(peer_facts().1, 1_700_000_000_000, b"", b"timeout")
            .await
    });
    network.path_requested.notified().await;
    tokio::time::advance(crate::RECIPIENT_RESOLUTION_TIMEOUT).await;
    assert_eq!(
        send.await.unwrap(),
        DurableSendDirectTextOutcome::RecipientResolutionTimedOut
    );
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .messages
        .is_empty());
    assert!(network.sent_wires.lock().unwrap().is_empty());
    service.stop().await.unwrap();
}

#[tokio::test]
async fn stamp_policy_is_rechecked_after_waiting_for_durable_admission() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    let destination = learn_durable_peer(&service).await;
    let gate = service.lifecycle.transition.lock().await;
    let sending = service.clone();
    let send = tokio::spawn(async move {
        sending
            .send_direct_text(destination, 1_700_000_000_000, b"", b"new stamp policy")
            .await
    });
    tokio::task::yield_now().await;
    let mut refresh = service.subscribe();
    let stamp = [0x93, 0xc4, 0x04, b's', b't', b'a', b'm', 0x05, 0x90];
    let (peer, _) = peer_facts();
    assert_eq!(
        service
            .callbacks()
            .on_authenticated_announce(&authenticated_observation(
                destination,
                peer.identity_hash(),
                &stamp
            )),
        CallbackOutcome::Enqueued
    );
    refresh.changed().await.unwrap();
    drop(gate);
    assert_eq!(
        send.await.unwrap(),
        DurableSendDirectTextOutcome::UnsupportedRemoteStampRequirement {
            required_stamp_cost: 5
        }
    );
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .messages
        .is_empty());
    assert!(network.sent_wires.lock().unwrap().is_empty());
    service.stop().await.unwrap();
}

#[tokio::test]
async fn stopping_recipient_lookup_never_admits_a_message() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    let sending = service.clone();
    let send = tokio::spawn(async move {
        sending
            .send_direct_text(peer_facts().1, 1_700_000_000_000, b"", b"stop")
            .await
    });
    network.path_requested.notified().await;
    service.stop().await.unwrap();
    assert_eq!(
        send.await.unwrap(),
        DurableSendDirectTextOutcome::LocalNodeStopped
    );
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .messages
        .is_empty());
    assert!(network.sent_wires.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn recipient_expiring_while_waiting_for_admission_is_not_resurrected() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    let destination = learn_durable_peer(&service).await;
    let gate = service.lifecycle.transition.lock().await;
    let sending = service.clone();
    let send = tokio::spawn(async move {
        sending
            .send_direct_text(destination, 1_700_000_000_000, b"", b"expired policy")
            .await
    });
    tokio::task::yield_now().await;
    assert!(!send.is_finished());
    assert_eq!(network.path_requests.load(Ordering::Acquire), 0);
    tokio::time::advance(crate::DISCOVERED_PEER_MAX_AGE).await;
    drop(gate);
    assert_eq!(
        send.await.unwrap(),
        DurableSendDirectTextOutcome::PeerIdentityUnavailable
    );
    let snapshot = service.snapshot(all_messages()).await.unwrap();
    assert!(snapshot.peers.is_empty());
    assert!(snapshot.messages.is_empty());
    assert!(network.sent_wires.lock().unwrap().is_empty());
    service.stop().await.unwrap();
}

#[tokio::test]
async fn abandoned_recipient_lookup_cannot_send_when_a_later_announce_arrives() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    let sending = service.clone();
    let send = tokio::spawn(async move {
        sending
            .send_direct_text(peer_facts().1, 1_700_000_000_000, b"", b"abandoned")
            .await
    });
    network.path_requested.notified().await;
    send.abort();
    assert!(send.await.unwrap_err().is_cancelled());
    learn_durable_peer(&service).await;
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .messages
        .is_empty());
    assert!(network.sent_wires.lock().unwrap().is_empty());
    service.stop().await.unwrap();
}

#[tokio::test]
async fn unresolved_recipient_waiters_are_bounded() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    let mut waiting = Vec::new();
    for _ in 0..8 {
        let sending = service.clone();
        waiting.push(tokio::spawn(async move {
            sending
                .send_direct_text(peer_facts().1, 1_700_000_000_000, b"", b"bounded")
                .await
        }));
        network.path_requested.notified().await;
    }
    assert_eq!(
        service
            .send_direct_text(peer_facts().1, 1_700_000_000_000, b"", b"over capacity")
            .await,
        DurableSendDirectTextOutcome::Busy
    );
    assert_eq!(network.path_requests.load(Ordering::Acquire), 8);
    service.stop().await.unwrap();
    for send in waiting {
        assert_eq!(
            send.await.unwrap(),
            DurableSendDirectTextOutcome::LocalNodeStopped
        );
    }
    assert!(service
        .snapshot(all_messages())
        .await
        .unwrap()
        .messages
        .is_empty());
}

#[tokio::test]
async fn durable_send_accepts_before_proof_and_projects_receipt_settlement() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    network.block_send.store(true, Ordering::Release);
    let service = start_durable(network.clone(), submitter).await;
    let destination = learn_durable_peer(&service).await;
    let before = *service.subscribe().borrow();

    assert_eq!(
        service
            .send_direct_text(
                destination,
                1_700_000_006_000,
                b"durable",
                b"accepted before proof",
            )
            .await,
        DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
    );
    network.send_entered.notified().await;
    let sending = service.snapshot(all_messages()).await.unwrap();
    assert!(matches!(
        sending.messages[0].delivery_state,
        DurableLxmfDeliveryState::Sending { failed_attempts: 0 }
    ));
    assert_eq!(sending.durable_revision, 1);
    assert_eq!(sending.projection_revision, before + 2);
    assert_eq!(
        service.retry_lxmf_message(1).await,
        RetryLxmfMessageOutcome::NotFailed {
            current: DurableLxmfDeliveryState::Sending { failed_attempts: 0 }
        }
    );

    network.release_send();
    let delivered = wait_for_durable_snapshot(&service, |snapshot| {
        matches!(
            snapshot.messages[0].delivery_state,
            DurableLxmfDeliveryState::Delivered {
                delivered_at_millis: 9_999,
                rtt_millis: Some(23),
            }
        )
    })
    .await;
    assert_eq!(delivered.durable_revision, 2);
    assert_eq!(delivered.projection_revision, before + 4);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn transient_settlement_admission_failure_does_not_strand_a_queued_row() {
    let (_root, database) = open_database();
    let database = Arc::new(database);
    let submitter = Arc::new(TransientCompletionSubmitter {
        database,
        remaining_failures: AtomicUsize::new(1),
        completion_submissions: AtomicUsize::new(0),
    });
    let network = Arc::new(DurableFakeNetwork::default());
    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, _callbacks) = DurableDirectLxmfService::prepare(identity);
    let service = pending
        .start(
            network.clone(),
            submitter.clone(),
            Arc::new(FixedClock(9_999)),
        )
        .await
        .unwrap();
    let destination = learn_durable_peer(&service).await;

    assert_eq!(
        service
            .send_direct_text(
                destination,
                1_700_000_006_500,
                b"settle",
                b"retry commit only",
            )
            .await,
        DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
    );
    let delivered = wait_for_durable_snapshot(&service, |snapshot| {
        matches!(
            snapshot.messages[0].delivery_state,
            DurableLxmfDeliveryState::Delivered { .. }
        )
    })
    .await;

    assert_eq!(delivered.durable_revision, 2);
    assert_eq!(delivered.mailbox_health, MailboxProjectionHealth::Ready);
    assert!(submitter.completion_submissions.load(Ordering::Acquire) >= 2);
    assert_eq!(network.sent_wires.lock().unwrap().len(), 1);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn successful_reads_do_not_hide_a_persistent_completion_write_failure() {
    let (_root, database) = open_database();
    let submitter = Arc::new(PersistentWriteFailureSubmitter {
        database: Arc::new(database),
        rejected: RejectedWrite::CompleteAttempt,
        failure: MailboxFailure::Unavailable("disk full".to_owned()),
    });
    let network = Arc::new(DurableFakeNetwork::default());
    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, _callbacks) = DurableDirectLxmfService::prepare(identity);
    let service = pending
        .start(network, submitter, Arc::new(FixedClock(9_999)))
        .await
        .unwrap();
    let destination = learn_durable_peer(&service).await;

    assert_eq!(
        service
            .send_direct_text(
                destination,
                1_700_000_006_700,
                b"settle",
                b"persistent failure",
            )
            .await,
        DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
    );
    let degraded = wait_for_durable_snapshot(&service, |snapshot| {
        matches!(
            snapshot.mailbox_health,
            MailboxProjectionHealth::Unavailable(ref detail) if detail == "disk full"
        )
    })
    .await;
    assert!(matches!(
        degraded.messages[0].delivery_state,
        DurableLxmfDeliveryState::Sending { .. }
    ));

    let read_again = service.snapshot(all_messages()).await.unwrap();
    assert_eq!(
        read_again.mailbox_health,
        MailboxProjectionHealth::Unavailable("disk full".to_owned())
    );
    service.stop().await.unwrap();
}

#[tokio::test]
async fn successful_reads_do_not_hide_a_rejected_inbound_write() {
    let (_root, database) = open_database();
    let submitter = Arc::new(PersistentWriteFailureSubmitter {
        database: Arc::new(database),
        rejected: RejectedWrite::InsertInbound,
        failure: MailboxFailure::Unavailable("inbound store unavailable".to_owned()),
    });
    let network = Arc::new(DurableFakeNetwork::default());
    let (peer, peer_destination) = peer_facts();
    network.add_public_key(peer_destination, &peer);
    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, _callbacks) = DurableDirectLxmfService::prepare(identity);
    let service = pending
        .start(network, submitter, Arc::new(FixedClock(9_999)))
        .await
        .unwrap();
    let signer = LocalLxmfIdentity::from_secret_bytes(&PEER_SECRET).unwrap();
    let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let prepared = compose_basic_direct_lxmf(
        service.local_destination(),
        peer_destination,
        1_700_000_006_800,
        b"inbound",
        b"rejected",
        None,
        &signer,
        &mut output,
    )
    .unwrap();
    let wire = &output[..usize::from(prepared.wire_len())];

    assert_eq!(
        service.callbacks().on_prns_event(&link_event(wire)),
        CallbackOutcome::Enqueued
    );
    let degraded = wait_for_durable_snapshot(&service, |snapshot| {
        matches!(
            snapshot.mailbox_health,
            MailboxProjectionHealth::Unavailable(ref detail)
                if detail == "inbound store unavailable"
        )
    })
    .await;
    assert!(degraded.messages.is_empty());

    let read_again = service.snapshot(all_messages()).await.unwrap();
    assert_eq!(
        read_again.mailbox_health,
        MailboxProjectionHealth::Unavailable("inbound store unavailable".to_owned())
    );
    service.stop().await.unwrap();
}

#[tokio::test]
async fn stop_retains_an_admitted_completion_failure_past_its_bounded_wait() {
    let (_root, database) = open_database();
    let submitter = Arc::new(GatedWriteFailureSubmitter {
        database: Arc::new(database),
        rejected: RejectedWrite::CompleteAttempt,
        failure: MailboxFailure::ResetRequired("completion record is corrupt".to_owned()),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let network = Arc::new(DurableFakeNetwork::default());
    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, _callbacks) = DurableDirectLxmfService::prepare(identity);
    let service = pending
        .start(network, submitter.clone(), Arc::new(FixedClock(9_999)))
        .await
        .unwrap();
    let destination = learn_durable_peer(&service).await;

    assert_eq!(
        service
            .send_direct_text(
                destination,
                1_700_000_006_900,
                b"settle",
                b"retained through stop",
            )
            .await,
        DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
    );
    submitter.entered.notified().await;
    assert_eq!(
        service.stop().await,
        Err(DirectServiceStopError::JoinTimedOut)
    );

    submitter.release.notify_one();
    service.stop().await.unwrap();
    let stopped = service.snapshot(all_messages()).await.unwrap();
    assert_eq!(
        stopped.mailbox_health,
        MailboxProjectionHealth::ResetRequired("completion record is corrupt".to_owned())
    );
    assert!(matches!(
        stopped.messages[0].delivery_state,
        DurableLxmfDeliveryState::Queued { .. }
    ));
}

#[tokio::test]
async fn stop_retains_an_admitted_inbound_failure_past_its_bounded_wait() {
    let (_root, database) = open_database();
    let submitter = Arc::new(GatedWriteFailureSubmitter {
        database: Arc::new(database),
        rejected: RejectedWrite::InsertInbound,
        failure: MailboxFailure::ResetRequired("inbound record is corrupt".to_owned()),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let network = Arc::new(DurableFakeNetwork::default());
    let (peer, peer_destination) = peer_facts();
    network.add_public_key(peer_destination, &peer);
    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, _callbacks) = DurableDirectLxmfService::prepare(identity);
    let service = pending
        .start(network, submitter.clone(), Arc::new(FixedClock(9_999)))
        .await
        .unwrap();
    let signer = LocalLxmfIdentity::from_secret_bytes(&PEER_SECRET).unwrap();
    let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let prepared = compose_basic_direct_lxmf(
        service.local_destination(),
        peer_destination,
        1_700_000_007_000,
        b"inbound",
        b"retained through stop",
        None,
        &signer,
        &mut output,
    )
    .unwrap();
    let wire = &output[..usize::from(prepared.wire_len())];

    assert_eq!(
        service.callbacks().on_prns_event(&link_event(wire)),
        CallbackOutcome::Enqueued
    );
    submitter.entered.notified().await;
    assert_eq!(
        service.stop().await,
        Err(DirectServiceStopError::JoinTimedOut)
    );

    submitter.release.notify_one();
    service.stop().await.unwrap();
    let stopped = service.snapshot(all_messages()).await.unwrap();
    assert_eq!(
        stopped.mailbox_health,
        MailboxProjectionHealth::ResetRequired("inbound record is corrupt".to_owned())
    );
    assert!(stopped.messages.is_empty());
}

#[tokio::test]
async fn cancelled_stop_retains_worker_and_drains_every_accepted_callback() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let (peer, peer_destination) = peer_facts();
    network.add_public_key(peer_destination, &peer);
    network.block_retention.store(true, Ordering::Release);
    let service = start_durable(network.clone(), submitter).await;
    let signer = LocalLxmfIdentity::from_secret_bytes(&PEER_SECRET).unwrap();
    let callbacks = service.callbacks();
    for timestamp in [1_700_000_007_001, 1_700_000_007_002] {
        let mut output = [0; MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = compose_basic_direct_lxmf(
            service.local_destination(),
            peer_destination,
            timestamp,
            b"inbound",
            b"accepted before shutdown",
            None,
            &signer,
            &mut output,
        )
        .unwrap();
        assert_eq!(
            callbacks.on_prns_event(&link_event(&output[..usize::from(prepared.wire_len())])),
            CallbackOutcome::Enqueued
        );
    }
    // The first callback is accepted but has not reached mailbox admission;
    // the second remains queued behind it when shutdown starts.
    network.retention_entered.notified().await;
    let stopping = service.clone();
    let stop = tokio::spawn(async move { stopping.stop().await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!stop.is_finished());
    assert!(service.lifecycle.worker().is_some());
    assert!(!service.lifecycle.stop_complete.load(Ordering::Acquire));
    stop.abort();
    assert!(stop.await.unwrap_err().is_cancelled());
    assert!(
        service.lifecycle.worker().is_some(),
        "cancellation cannot detach the worker"
    );
    network.block_retention.store(false, Ordering::Release);
    network.retention_released.notify_one();
    service.stop().await.unwrap();
    let snapshot = service.snapshot(all_messages()).await.unwrap();
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.durable_revision, 2);
    assert!(snapshot
        .messages
        .iter()
        .all(|message| message.direction == LxmfDirection::Inbound));
    service.stop().await.unwrap();
}

#[tokio::test]
async fn failed_callback_worker_cannot_claim_successful_drain_on_retry() {
    let (_root, _database, submitter) = shared_database();
    let service = start_durable(Arc::new(DurableFakeNetwork::default()), submitter).await;
    service.lifecycle.worker().as_ref().unwrap().abort();
    assert_eq!(
        service.stop().await,
        Err(DirectServiceStopError::WorkerFailed)
    );
    assert_eq!(
        service.stop().await,
        Err(DirectServiceStopError::WorkerFailed)
    );
    assert!(!service.lifecycle.stop_complete.load(Ordering::Acquire));
}

#[tokio::test]
async fn concurrent_stop_waiters_do_not_report_before_the_join_boundary() {
    let (_root, _database, submitter) = shared_database();
    let service = start_durable(Arc::new(DurableFakeNetwork::default()), submitter).await;
    let transition = service.lifecycle.transition.lock().await;
    let first_service = service.clone();
    let first = tokio::spawn(async move { first_service.stop().await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let second_service = service.clone();
    let second = tokio::spawn(async move { second_service.stop().await });
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert!(!first.is_finished());
    assert!(!second.is_finished());
    drop(transition);
    assert_eq!(first.await.unwrap(), Ok(()));
    assert_eq!(second.await.unwrap(), Ok(()));

    let transition = service.lifecycle.transition.lock().await;
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(10), service.stop())
            .await
            .unwrap(),
        Ok(())
    );
    drop(transition);
}

#[tokio::test]
async fn start_failure_stops_and_joins_the_prepared_callback_worker() {
    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, callbacks) = DurableDirectLxmfService::prepare(identity);
    let result = pending
        .start(
            Arc::new(DurableFakeNetwork::default()),
            Arc::new(AlwaysBusySubmitter),
            Arc::new(FixedClock(9_999)),
        )
        .await;

    assert!(matches!(
        result,
        Err(DurableDirectServiceStartError::Mailbox(
            MailboxFailure::Busy
        ))
    ));
    assert_eq!(callbacks.health.state(), crate::LxmfHealthState::Stopped);
}

#[tokio::test]
async fn stop_leaves_queued_and_restart_resumes_the_identical_wire() {
    let (_root, database, submitter) = shared_database();
    let first_network = Arc::new(DurableFakeNetwork::default());
    first_network.block_send.store(true, Ordering::Release);
    let first = start_durable(first_network.clone(), submitter.clone()).await;
    let destination = learn_durable_peer(&first).await;
    assert!(matches!(
        first
            .send_direct_text(destination, 1_700_000_007_000, b"restart", b"same wire")
            .await,
        DurableSendDirectTextOutcome::Accepted { .. }
    ));
    first_network.send_entered.notified().await;
    let first_wire = first_network.sent_wires.lock().unwrap()[0].clone();
    first.stop().await.unwrap();
    assert!(first_network.send_future_dropped.load(Ordering::Acquire));

    let MailboxReply::Listed { messages, .. } =
        execute_mailbox_request(&database, MailboxRequest::List(all_messages())).unwrap()
    else {
        panic!("unexpected list reply");
    };
    assert!(matches!(
        messages[0].delivery_state,
        DurableLxmfDeliveryState::Queued { failed_attempts: 0 }
    ));

    let second_network = Arc::new(DurableFakeNetwork::default());
    let second = start_durable(second_network.clone(), submitter).await;
    let delivered = wait_for_durable_snapshot(&second, |snapshot| {
        matches!(
            snapshot.messages[0].delivery_state,
            DurableLxmfDeliveryState::Delivered { .. }
        )
    })
    .await;
    assert_eq!(delivered.messages[0].exact_wire, first_wire);
    assert_eq!(second_network.sent_wires.lock().unwrap()[0], first_wire);
    second.stop().await.unwrap();
}

#[tokio::test]
async fn restart_after_proof_before_delivered_commit_resends_and_receiver_deduplicates() {
    let (_sender_root, sender_database) = open_database();
    let sender_database = Arc::new(sender_database);
    let stalled_completion = Arc::new(GatedWriteFailureSubmitter {
        database: sender_database.clone(),
        rejected: RejectedWrite::CompleteAttempt,
        failure: MailboxFailure::ResetRequired("must not settle before the crash".to_owned()),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let first_network = Arc::new(DurableFakeNetwork::default());
    let sender_identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, _) = DurableDirectLxmfService::prepare(sender_identity);
    let first = pending
        .start(
            first_network.clone(),
            stalled_completion.clone(),
            Arc::new(FixedClock(9_999)),
        )
        .await
        .unwrap();
    let receiver_destination = learn_durable_peer(&first).await;
    assert_eq!(
        first
            .send_direct_text(
                receiver_destination,
                1_700_000_007_250,
                b"proof crash",
                b"deduplicate the replay",
            )
            .await,
        DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
    );

    // Reaching CompleteAttempt proves that the transport proof was returned,
    // while the gate keeps the local Delivered transition uncommitted.
    stalled_completion.entered.notified().await;
    let first_wire = first_network.sent_wires.lock().unwrap()[0].clone();

    let (_receiver_root, _receiver_database, receiver_submitter) = shared_database();
    let receiver_network = Arc::new(DurableFakeNetwork::default());
    let sender_material = PrivateIdentityMaterial::from_bytes(LOCAL_SECRET);
    let sender_destination = derive_single_destination_hash(
        &sender_material.identity_hash(),
        prns_lxmf_wire::LXMF_APP_NAME,
        prns_lxmf_wire::LXMF_DELIVERY_ASPECTS,
    )
    .unwrap();
    receiver_network.add_public_key(*sender_destination.as_bytes(), &sender_material);
    let receiver_identity = LocalLxmfIdentity::from_secret_bytes(&PEER_SECRET).unwrap();
    let (receiver_pending, _) = DurableDirectLxmfService::prepare(receiver_identity);
    let receiver = receiver_pending
        .start(
            receiver_network,
            receiver_submitter,
            Arc::new(FixedClock(9_999)),
        )
        .await
        .unwrap();
    assert_eq!(receiver.local_destination(), receiver_destination);
    assert_eq!(
        receiver.callbacks().on_prns_event(&link_event(&first_wire)),
        CallbackOutcome::Enqueued
    );
    let received_once =
        wait_for_durable_snapshot(&receiver, |snapshot| snapshot.messages.len() == 1).await;
    assert_eq!(received_once.durable_revision, 1);
    assert_eq!(
        received_once.messages[0].verification,
        LxmfVerification::Verified
    );

    // Model abrupt process loss at the exact crash boundary. The blocked
    // local completion never reaches redb, so reopening must see Queued.
    let attempts = mem::take(&mut *first.lifecycle.attempts());
    assert_eq!(attempts.len(), 1);
    for (_key, attempt) in attempts {
        attempt.abort();
        assert!(attempt.await.unwrap_err().is_cancelled());
    }
    first.stop().await.unwrap();
    let MailboxReply::Listed { messages, .. } =
        execute_mailbox_request(&sender_database, MailboxRequest::List(all_messages())).unwrap()
    else {
        panic!("unexpected list reply");
    };
    assert!(matches!(
        messages[0].delivery_state,
        DurableLxmfDeliveryState::Queued { failed_attempts: 0 }
    ));

    let restarted_network = Arc::new(DurableFakeNetwork::default());
    let restarted_submitter = Arc::new(DatabaseSubmitter {
        database: sender_database,
    });
    let restarted = start_durable(restarted_network.clone(), restarted_submitter).await;
    let delivered = wait_for_durable_snapshot(&restarted, |snapshot| {
        matches!(
            snapshot.messages[0].delivery_state,
            DurableLxmfDeliveryState::Delivered { .. }
        )
    })
    .await;
    assert_eq!(restarted_network.sent_wires.lock().unwrap()[0], first_wire);
    assert_eq!(delivered.messages[0].exact_wire, first_wire);

    assert_eq!(
        receiver.callbacks().on_prns_event(&link_event(&first_wire)),
        CallbackOutcome::Enqueued
    );
    let announce = current_announce();
    assert_eq!(
        receiver
            .callbacks()
            .on_authenticated_announce(&authenticated_observation(
                *sender_destination.as_bytes(),
                sender_material.identity_hash(),
                &announce,
            )),
        CallbackOutcome::Enqueued
    );
    let deduplicated =
        wait_for_durable_snapshot(&receiver, |snapshot| !snapshot.peers.is_empty()).await;
    assert_eq!(deduplicated.messages.len(), 1);
    assert_eq!(deduplicated.durable_revision, 1);
    assert_eq!(
        deduplicated.messages[0].message_id,
        delivered.messages[0].message_id
    );
    assert_eq!(deduplicated.messages[0].exact_wire, first_wire);

    restarted.stop().await.unwrap();
    receiver.stop().await.unwrap();
}

#[tokio::test]
async fn paused_start_keeps_reopened_and_new_rows_queued_until_activation() {
    let (_root, database, submitter) = shared_database();
    let MailboxReply::OutboundInserted { .. } = execute_mailbox_request(
        &database,
        MailboxRequest::InsertOutbound(outbound(1_700_000_007_500)),
    )
    .unwrap() else {
        panic!("unexpected insert reply");
    };
    let network = Arc::new(DurableFakeNetwork::default());
    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, _callbacks) = DurableDirectLxmfService::prepare(identity);
    let service = pending
        .start_paused(network.clone(), submitter, Arc::new(FixedClock(9_999)))
        .await
        .unwrap();

    tokio::task::yield_now().await;
    assert!(network.sent_wires.lock().unwrap().is_empty());
    assert!(matches!(
        service.snapshot(all_messages()).await.unwrap().messages[0].delivery_state,
        DurableLxmfDeliveryState::Queued { failed_attempts: 0 }
    ));

    let destination = learn_durable_peer(&service).await;
    assert_eq!(
        service
            .send_direct_text(
                destination,
                1_700_000_007_600,
                b"paused",
                b"accepted while paused",
            )
            .await,
        DurableSendDirectTextOutcome::Accepted { local_record_id: 2 }
    );
    tokio::task::yield_now().await;
    assert!(network.sent_wires.lock().unwrap().is_empty());

    assert!(service.activate_queued_attempts().await);
    let settled = wait_for_durable_snapshot(&service, |snapshot| {
        snapshot.messages.iter().all(|message| {
            matches!(
                message.delivery_state,
                DurableLxmfDeliveryState::Delivered { .. }
            )
        })
    })
    .await;
    assert_eq!(settled.messages.len(), 2);
    assert_eq!(network.sent_wires.lock().unwrap().len(), 2);
    assert!(service.activate_queued_attempts().await);
    service.stop().await.unwrap();
    assert!(!service.activate_queued_attempts().await);
}

#[tokio::test]
async fn late_proofs_after_cancel_and_retry_are_diagnostic_only() {
    let (_root, database, submitter) = shared_database();
    let cancelled = inserted_outbound(&database, 1_700_000_007_700);
    let cancelled_key = AttemptKey {
        local_record_id: cancelled.local_record_id,
        generation: cancelled.generation().unwrap(),
    };
    assert!(matches!(
        execute_mailbox_request(
            &database,
            MailboxRequest::Cancel {
                local_record_id: cancelled.local_record_id,
                cancelled_at_millis: 12_345,
            },
        )
        .unwrap(),
        MailboxReply::Cancel(CancelTransition::Cancelled { .. })
    ));

    let retry_source = inserted_outbound(&database, 1_700_000_007_800);
    let retry_wire = retry_source.exact_wire.clone();
    let old_retry_key = AttemptKey {
        local_record_id: retry_source.local_record_id,
        generation: retry_source.generation().unwrap(),
    };
    execute_mailbox_request(
        &database,
        MailboxRequest::CompleteAttempt {
            key: old_retry_key,
            completion: AttemptCompletion::Failed {
                failure: DirectSendFailure::DeliveryTimedOut,
            },
        },
    )
    .unwrap();
    let MailboxReply::Retry(RetryTransition::Accepted {
        message: retried, ..
    }) = execute_mailbox_request(
        &database,
        MailboxRequest::Retry {
            local_record_id: retry_source.local_record_id,
        },
    )
    .unwrap()
    else {
        panic!("retry was not accepted");
    };
    assert_eq!(retried.generation(), Some(2));
    assert_eq!(retried.exact_wire, retry_wire);

    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, _callbacks) = DurableDirectLxmfService::prepare(identity);
    let service = pending
        .start_paused(
            Arc::new(DurableFakeNetwork::default()),
            submitter,
            Arc::new(FixedClock(9_999)),
        )
        .await
        .unwrap();
    let before = *service.subscribe().borrow();
    assert_eq!(service.late_proof_count(), 0);

    for key in [cancelled_key, old_retry_key] {
        settle_durable_attempt(
            service.shared.clone(),
            Arc::downgrade(&service.lifecycle),
            key,
            Ok(DirectDeliveryReceipt { rtt_millis: 3 }),
        )
        .await;
    }

    assert_eq!(service.late_proof_count(), 2);
    let snapshot = service.snapshot(all_messages()).await.unwrap();
    assert_eq!(snapshot.durable_revision, 5);
    assert_eq!(snapshot.projection_revision, before + 2);
    assert!(matches!(
        snapshot
            .messages
            .iter()
            .find(|message| message.local_record_id == cancelled.local_record_id)
            .unwrap()
            .delivery_state,
        DurableLxmfDeliveryState::Cancelled {
            cancelled_at_millis: 12_345
        }
    ));
    let retry_after = snapshot
        .messages
        .iter()
        .find(|message| message.local_record_id == retry_source.local_record_id)
        .unwrap();
    assert_eq!(retry_after.generation(), Some(2));
    assert_eq!(retry_after.exact_wire, retry_wire);
    assert_eq!(
        retry_after.delivery_state,
        DurableLxmfDeliveryState::Queued { failed_attempts: 1 }
    );

    service.stop().await.unwrap();
}

#[tokio::test]
async fn late_proof_diagnostic_saturates_without_refresh_churn() {
    let (_root, _database, submitter) = shared_database();
    let identity = LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap();
    let (pending, _callbacks) = DurableDirectLxmfService::prepare(identity);
    let service = pending
        .start_paused(
            Arc::new(DurableFakeNetwork::default()),
            submitter,
            Arc::new(FixedClock(9_999)),
        )
        .await
        .unwrap();
    service
        .shared
        .late_proof_count
        .store(u64::MAX, Ordering::Release);
    let before = *service.subscribe().borrow();

    service.shared.record_late_proof();

    assert_eq!(service.late_proof_count(), u64::MAX);
    assert_eq!(*service.subscribe().borrow(), before);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn durable_retry_is_byte_identical_and_cancel_wins_before_late_proof() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    *network.results.lock().unwrap() = VecDeque::from([
        Err(DirectSendFailure::DeliveryTimedOut),
        Ok(DirectDeliveryReceipt { rtt_millis: 31 }),
    ]);
    let service = start_durable(network.clone(), submitter).await;
    let destination = learn_durable_peer(&service).await;
    assert_eq!(
        service
            .send_direct_text(destination, 1_700_000_008_000, b"retry", b"same")
            .await,
        DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
    );
    wait_for_durable_snapshot(&service, |snapshot| {
        matches!(
            snapshot.messages[0].delivery_state,
            DurableLxmfDeliveryState::Failed {
                failed_attempts: 1,
                last_failure: DirectSendFailure::DeliveryTimedOut,
            }
        )
    })
    .await;
    let first_wire = network.sent_wires.lock().unwrap()[0].clone();

    network.block_send.store(true, Ordering::Release);
    assert_eq!(
        service.retry_lxmf_message(1).await,
        RetryLxmfMessageOutcome::Accepted { local_record_id: 1 }
    );
    while network.sent_wires.lock().unwrap().len() < 2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(network.sent_wires.lock().unwrap()[1], first_wire);
    assert_eq!(
        service.cancel_lxmf_message(1, 12_345).await,
        CancelLxmfMessageOutcome::Cancelled { local_record_id: 1 }
    );
    network.release_send();
    let cancelled = service.snapshot(all_messages()).await.unwrap();
    assert!(matches!(
        cancelled.messages[0].delivery_state,
        DurableLxmfDeliveryState::Cancelled {
            cancelled_at_millis: 12_345
        }
    ));
    service.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn durable_key_use_is_authenticated_and_retention_timeout_is_best_effort() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    network.block_retention.store(true, Ordering::Release);
    let (peer, peer_destination) = peer_facts();
    network.add_public_key(peer_destination, &peer);
    let service = start_durable(network.clone(), submitter).await;
    learn_durable_peer(&service).await;
    assert_eq!(
        service
            .send_direct_text(
                peer_destination,
                1_700_000_009_000,
                b"outbound",
                b"retained"
            )
            .await,
        DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
    );
    network.retention_entered.notified().await;
    assert!(matches!(
        service.snapshot(all_messages()).await.unwrap().messages[0].delivery_state,
        DurableLxmfDeliveryState::Sending { failed_attempts: 0 }
    ));
    tokio::time::advance(Duration::from_secs(1)).await;
    let delivered = wait_for_durable_snapshot(&service, |snapshot| {
        matches!(
            snapshot.messages[0].delivery_state,
            DurableLxmfDeliveryState::Delivered { .. }
        )
    })
    .await;
    assert_eq!(delivered.messages[0].local_record_id, 1);

    for (secret, title, tamper) in [
        (PEER_SECRET, b"verified".as_slice(), false),
        (
            [0x73; IDENTITY_SECRET_KEY_LEN],
            b"unknown".as_slice(),
            false,
        ),
        (PEER_SECRET, b"invalid".as_slice(), true),
    ] {
        let signer = LocalLxmfIdentity::from_secret_bytes(&secret).unwrap();
        let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = compose_basic_direct_lxmf(
            service.local_destination(),
            signer.destination(),
            1_700_000_009_000,
            title,
            b"inbound",
            None,
            &signer,
            &mut output,
        )
        .unwrap();
        let mut wire = output[..usize::from(prepared.wire_len())].to_vec();
        if tamper {
            wire[32] ^= 1;
        }
        assert_eq!(
            service.callbacks().on_prns_event(&link_event(&wire)),
            CallbackOutcome::Enqueued
        );
    }
    network.retention_entered.notified().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    let snapshot =
        wait_for_durable_snapshot(&service, |snapshot| snapshot.messages.len() == 4).await;
    for (title, verification) in [
        (b"verified".as_slice(), LxmfVerification::Verified),
        (b"unknown".as_slice(), LxmfVerification::SourceUnknown),
        (b"invalid".as_slice(), LxmfVerification::InvalidSignature),
    ] {
        assert_eq!(
            snapshot
                .messages
                .iter()
                .find(|message| message.title == title)
                .unwrap()
                .verification,
            verification
        );
    }
    assert_eq!(
        *network.used_destinations.lock().unwrap(),
        vec![peer_destination, peer_destination],
        "only the established recipient and verified source refresh key retention"
    );
    service.stop().await.unwrap();
}

#[tokio::test]
async fn verified_second_receipt_upgrades_the_first_receipt_and_notifies() {
    for initial_verification in [
        LxmfVerification::InvalidSignature,
        LxmfVerification::SourceUnknown,
    ] {
        let (_root, database, submitter) = shared_database();
        let network = Arc::new(DurableFakeNetwork::default());
        let (peer, peer_destination) = peer_facts();
        if initial_verification == LxmfVerification::InvalidSignature {
            network.add_public_key(peer_destination, &peer);
        }
        let service = start_durable(network.clone(), submitter.clone()).await;
        let signer = LocalLxmfIdentity::from_secret_bytes(&PEER_SECRET).unwrap();
        let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = compose_basic_direct_lxmf(
            service.local_destination(),
            peer_destination,
            1_700_000_009_000,
            b"same message",
            b"authentic contents",
            None,
            &signer,
            &mut output,
        )
        .unwrap();
        let verified_wire = output[..usize::from(prepared.wire_len())].to_vec();
        let mut initial_wire = verified_wire.clone();
        if initial_verification == LxmfVerification::InvalidSignature {
            initial_wire[32] ^= 1;
        }
        assert_eq!(
            service
                .callbacks()
                .on_prns_event(&link_event(&initial_wire)),
            CallbackOutcome::Enqueued
        );
        let first =
            wait_for_durable_snapshot(&service, |snapshot| snapshot.messages.len() == 1).await;
        assert_eq!(first.messages[0].verification, initial_verification);
        assert_eq!(first.durable_revision, 1);
        network.add_public_key(peer_destination, &peer);
        assert_eq!(
            service
                .callbacks()
                .on_prns_event(&link_event(&verified_wire)),
            CallbackOutcome::Enqueued
        );
        let upgraded =
            wait_for_durable_snapshot(&service, |snapshot| snapshot.durable_revision == 2).await;
        assert_eq!(upgraded.messages.len(), 1);
        assert_eq!(
            upgraded.messages[0].local_record_id,
            first.messages[0].local_record_id
        );
        assert_eq!(
            upgraded.messages[0].verification,
            LxmfVerification::Verified
        );
        assert_eq!(upgraded.messages[0].exact_wire, verified_wire);
        assert!(upgraded.projection_revision > first.projection_revision);
        let conversations = service
            .conversation_snapshot(MailboxConversationListRequest {
                before: None,
                limit: 1,
            })
            .await
            .unwrap();
        assert_eq!(conversations.messages, upgraded.messages);
        assert_eq!(conversations.durable_revision, 2);
        service.stop().await.unwrap();
        validate_mailbox_database(&database).unwrap();
        let restarted = start_durable(Arc::new(DurableFakeNetwork::default()), submitter).await;
        assert_eq!(
            restarted.snapshot(all_messages()).await.unwrap().messages,
            upgraded.messages
        );
        restarted.stop().await.unwrap();
    }
}

#[tokio::test]
async fn proof_first_inbound_commit_deduplicates_across_restart() {
    let (_root, _database, submitter) = shared_database();
    let network = Arc::new(DurableFakeNetwork::default());
    let (peer, peer_destination) = peer_facts();
    network.add_public_key(peer_destination, &peer);
    let service = start_durable(network, submitter.clone()).await;
    let local_destination = service.local_destination();
    let signer = LocalLxmfIdentity::from_secret_bytes(&PEER_SECRET).unwrap();
    let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let prepared = compose_basic_direct_lxmf(
        local_destination,
        peer_destination,
        1_700_000_009_000,
        b"inbound",
        b"persisted",
        None,
        &signer,
        &mut output,
    )
    .unwrap();
    let wire = output[..usize::from(prepared.wire_len())].to_vec();
    assert_eq!(
        service.callbacks().on_prns_event(&link_event(&wire)),
        CallbackOutcome::Enqueued
    );
    let first = wait_for_durable_snapshot(&service, |snapshot| snapshot.messages.len() == 1).await;
    assert_eq!(first.durable_revision, 1);
    assert_eq!(first.messages[0].verification, LxmfVerification::Verified);

    assert_eq!(
        service.callbacks().on_prns_event(&link_event(&wire)),
        CallbackOutcome::Enqueued
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    let duplicate = service.snapshot(all_messages()).await.unwrap();
    assert_eq!(duplicate.messages.len(), 1);
    assert_eq!(duplicate.durable_revision, 1);
    service.stop().await.unwrap();

    let restarted = start_durable(Arc::new(DurableFakeNetwork::default()), submitter).await;
    let reopened = restarted.snapshot(all_messages()).await.unwrap();
    assert_eq!(reopened.messages.len(), 1);
    assert_eq!(reopened.messages[0].exact_wire, wire);
    restarted.stop().await.unwrap();
}
async fn wait_sent(network: &DurableFakeNetwork, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while network.sent_wires.lock().unwrap().len() < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn bounded_replay_preserves_backlog_pause_stop_and_restart() {
    let (_root, database, submitter) = shared_database();
    for time in 1..=23 {
        execute_mailbox_request(&database, MailboxRequest::InsertOutbound(outbound(time))).unwrap();
    }
    let network = Arc::new(DurableFakeNetwork::default());
    network.block_send.store(true, Ordering::Release);
    let (pending, _) = DurableDirectLxmfService::prepare_with_limits(
        LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap(),
        ReplayLimits {
            max_active_attempts: 3,
            max_page_rows: 2,
        },
    )
    .unwrap();
    let service = pending
        .start_paused(network.clone(), submitter.clone(), Arc::new(FixedClock(50)))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(network.sent_wires.lock().unwrap().is_empty());
    assert!(service.lifecycle.attempts().is_empty());
    assert!(service.activate_queued_attempts().await);
    wait_sent(&network, 3).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(network.sent_wires.lock().unwrap().len(), 3);
    assert_eq!(service.lifecycle.attempts().len(), 3);
    assert!(matches!(
        service.cancel_lxmf_message(2, 51).await,
        CancelLxmfMessageOutcome::Cancelled { local_record_id: 2 }
    ));
    wait_sent(&network, 4).await;
    assert_eq!(service.lifecycle.attempts().len(), 3);
    service.stop().await.unwrap();
    drop(service);
    validate_mailbox_database(&database).unwrap();
    let MailboxReply::QueuedPage { messages, .. } = execute_mailbox_request(
        &database,
        MailboxRequest::Replay(MailboxReplayRequest {
            after: None,
            limit: 100,
        }),
    )
    .unwrap() else {
        panic!("replay")
    };
    assert_eq!(messages.len(), 22);
    assert!(messages.iter().all(|row| row.local_record_id != 2));
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    wait_sent(&network, 22).await;
    let snapshot = wait_for_durable_snapshot(&service, |s| {
        s.messages
            .iter()
            .filter(|r| matches!(r.delivery_state, DurableLxmfDeliveryState::Delivered { .. }))
            .count()
            == 22
    })
    .await;
    assert_eq!(snapshot.messages.len(), 23);
    assert_eq!(network.sent_wires.lock().unwrap().len(), 22);
    service.stop().await.unwrap();
    validate_mailbox_database(&database).unwrap();
}

#[tokio::test]
async fn bounded_replay_rediscovers_lower_id_retry_and_lost_insert_wakeup() {
    let (_root, database, submitter) = shared_database();
    for time in 1..=5 {
        execute_mailbox_request(&database, MailboxRequest::InsertOutbound(outbound(time))).unwrap();
    }
    execute_mailbox_request(
        &database,
        MailboxRequest::CompleteAttempt {
            key: AttemptKey {
                local_record_id: 1,
                generation: 1,
            },
            completion: AttemptCompletion::Failed {
                failure: DirectSendFailure::NoRoute,
            },
        },
    )
    .unwrap();
    let network = Arc::new(DurableFakeNetwork::default());
    let service = start_durable(network.clone(), submitter).await;
    wait_sent(&network, 4).await;
    wait_for_durable_snapshot(&service, |s| {
        s.messages
            .iter()
            .filter(|r| matches!(r.delivery_state, DurableLxmfDeliveryState::Delivered { .. }))
            .count()
            == 4
    })
    .await;
    assert!(matches!(
        service.retry_lxmf_message(1).await,
        RetryLxmfMessageOutcome::Accepted { local_record_id: 1 }
    ));
    wait_sent(&network, 5).await;
    wait_for_durable_snapshot(&service, |s| {
        s.messages
            .iter()
            .all(|r| matches!(r.delivery_state, DurableLxmfDeliveryState::Delivered { .. }))
    })
    .await;
    // Simulates an admitted owner commit whose result waiter never delivered
    // a wake to this generation. The row itself is the durable authority.
    execute_mailbox_request(&database, MailboxRequest::InsertOutbound(outbound(6))).unwrap();
    wait_sent(&network, 6).await;
    let snapshot = wait_for_durable_snapshot(&service, |s| {
        s.messages.len() == 6
            && s.messages
                .iter()
                .all(|r| matches!(r.delivery_state, DurableLxmfDeliveryState::Delivered { .. }))
    })
    .await;
    assert_eq!(snapshot.messages[5].generation(), Some(2));
    service.stop().await.unwrap();
    validate_mailbox_database(&database).unwrap();
}

#[tokio::test]
async fn replay_completion_uncertainty_holds_the_selected_active_slot() {
    let (_root, database, _) = shared_database();
    for time in 1..=2 {
        execute_mailbox_request(&database, MailboxRequest::InsertOutbound(outbound(time))).unwrap();
    }
    let submitter = Arc::new(TransientCompletionSubmitter {
        database: database.clone(),
        remaining_failures: AtomicUsize::new(1000),
        completion_submissions: AtomicUsize::new(0),
    });
    let network = Arc::new(DurableFakeNetwork::default());
    let (pending, _) = DurableDirectLxmfService::prepare_with_limits(
        LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap(),
        ReplayLimits {
            max_active_attempts: 1,
            max_page_rows: 1,
        },
    )
    .unwrap();
    let service = pending
        .start(network.clone(), submitter.clone(), Arc::new(FixedClock(50)))
        .await
        .unwrap();
    wait_sent(&network, 1).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(network.sent_wires.lock().unwrap().len(), 1);
    assert_eq!(service.lifecycle.attempts().len(), 1);
    assert!(submitter.completion_submissions.load(Ordering::Acquire) > 1);
    submitter.remaining_failures.store(0, Ordering::Release);
    wait_sent(&network, 2).await;
    wait_for_durable_snapshot(&service, |s| {
        s.messages
            .iter()
            .all(|r| matches!(r.delivery_state, DurableLxmfDeliveryState::Delivered { .. }))
    })
    .await;
    assert_eq!(network.sent_wires.lock().unwrap().len(), 2);
    service.stop().await.unwrap();
}
#[tokio::test]
async fn dropping_last_owner_during_uncertain_settlement_releases_tasks_and_keeps_queue() {
    let (_root, database, _) = shared_database();
    execute_mailbox_request(&database, MailboxRequest::InsertOutbound(outbound(1))).unwrap();
    let submitter = Arc::new(TransientCompletionSubmitter {
        database: database.clone(),
        remaining_failures: AtomicUsize::new(usize::MAX),
        completion_submissions: AtomicUsize::new(0),
    });
    let network = Arc::new(DurableFakeNetwork::default());
    let (pending, _) = DurableDirectLxmfService::prepare(
        LocalLxmfIdentity::from_secret_bytes(&LOCAL_SECRET).unwrap(),
    );
    let service = pending
        .start(network.clone(), submitter.clone(), Arc::new(FixedClock(50)))
        .await
        .unwrap();
    wait_sent(&network, 1).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while submitter.completion_submissions.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let owner = Arc::downgrade(&service.lifecycle);
    drop(service);
    tokio::time::timeout(Duration::from_secs(2), async {
        while owner.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let MailboxReply::QueuedPage { messages, .. } = execute_mailbox_request(
        &database,
        MailboxRequest::Replay(MailboxReplayRequest {
            after: None,
            limit: 1,
        }),
    )
    .unwrap() else {
        panic!("wrong queue reply");
    };
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].generation(), Some(1));
    validate_mailbox_database(&database).unwrap();
}
