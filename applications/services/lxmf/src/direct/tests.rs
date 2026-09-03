use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use personal_rns::identity::{IdentityHash, PrivateIdentityMaterial, Zeroizing};
use personal_rns::interfaces::InterfaceId;
use personal_rns::routing::announce::{derive_single_destination_hash, AnnounceObservation};
use personal_rns::routing::delivery::{Delivery, LinkDelivery};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{Diagnostic, Message, PrnsEvent};
use personal_rns::units::{HopCount, InstantMillis};
use personal_rns::wire::DestinationHash;
use prns_lxmf_wire::{
    compose_basic_direct_lxmf, encode_current_lxmf_announce, MAX_BASIC_LXMF_WIRE_BYTES,
};
use tokio::sync::Notify;

use super::*;

const LOCAL_SECRET: [u8; IDENTITY_SECRET_KEY_LEN] = [0x31; IDENTITY_SECRET_KEY_LEN];
const PEER_SECRET: [u8; IDENTITY_SECRET_KEY_LEN] = [0x52; IDENTITY_SECRET_KEY_LEN];
const OTHER_SECRET: [u8; IDENTITY_SECRET_KEY_LEN] = [0x73; IDENTITY_SECRET_KEY_LEN];
const TEST_INTERFACE: InterfaceId = InterfaceId::new([4, 1, 2, 3, 4, 5, 6, 7]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    HasRoute,
    RequestPath,
    EstablishLink,
    SendLinkPacket,
    DestinationPublicKey,
    Announce,
}

struct FakeNetwork {
    has_route: AtomicBool,
    request_result: StdMutex<Result<(), DirectSendFailure>>,
    establish_result: StdMutex<Result<[u8; 16], DirectSendFailure>>,
    send_result: StdMutex<Result<(), DirectSendFailure>>,
    public_keys: StdMutex<BTreeMap<[u8; 16], [u8; 64]>>,
    calls: StdMutex<Vec<Call>>,
    sent_wires: StdMutex<Vec<Vec<u8>>>,
    block_send: AtomicBool,
    send_entered: Notify,
    send_released: Notify,
    send_future_dropped: AtomicBool,
    block_public_key: AtomicBool,
    public_key_entered: Notify,
    public_key_released: Notify,
    public_key_future_dropped: AtomicBool,
}

struct DropSignal<'a>(&'a AtomicBool);

impl Drop for DropSignal<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Default for FakeNetwork {
    fn default() -> Self {
        Self {
            has_route: AtomicBool::new(true),
            request_result: StdMutex::new(Ok(())),
            establish_result: StdMutex::new(Ok([0xa5; 16])),
            send_result: StdMutex::new(Ok(())),
            public_keys: StdMutex::new(BTreeMap::new()),
            calls: StdMutex::new(Vec::new()),
            sent_wires: StdMutex::new(Vec::new()),
            block_send: AtomicBool::new(false),
            send_entered: Notify::new(),
            send_released: Notify::new(),
            send_future_dropped: AtomicBool::new(false),
            block_public_key: AtomicBool::new(false),
            public_key_entered: Notify::new(),
            public_key_released: Notify::new(),
            public_key_future_dropped: AtomicBool::new(false),
        }
    }
}

impl FakeNetwork {
    fn record(&self, call: Call) {
        self.calls.lock().expect("call log is available").push(call);
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("call log is available").clone()
    }

    fn add_public_key(&self, destination: [u8; 16], material: &PrivateIdentityMaterial) {
        self.public_keys
            .lock()
            .expect("public-key map is available")
            .insert(destination, *material.public().as_bytes());
    }

    fn release_send(&self) {
        self.block_send.store(false, Ordering::Release);
        self.send_released.notify_one();
    }
}

impl DirectNetwork for FakeNetwork {
    fn has_route(&self, _destination: [u8; 16]) -> DirectNetworkFuture<'_, bool> {
        Box::pin(async move {
            self.record(Call::HasRoute);
            self.has_route.load(Ordering::Acquire)
        })
    }

    fn request_path(
        &self,
        _destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>> {
        Box::pin(async move {
            self.record(Call::RequestPath);
            *self
                .request_result
                .lock()
                .expect("request result is available")
        })
    }

    fn establish_link(
        &self,
        _destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<[u8; 16], DirectSendFailure>> {
        Box::pin(async move {
            self.record(Call::EstablishLink);
            *self
                .establish_result
                .lock()
                .expect("establish result is available")
        })
    }

    fn send_link_packet(
        &self,
        _link: [u8; 16],
        complete_wire: Vec<u8>,
    ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>> {
        Box::pin(async move {
            let _drop_signal = DropSignal(&self.send_future_dropped);
            self.record(Call::SendLinkPacket);
            self.sent_wires
                .lock()
                .expect("sent-wire log is available")
                .push(complete_wire);
            self.send_entered.notify_one();
            while self.block_send.load(Ordering::Acquire) {
                self.send_released.notified().await;
            }
            *self.send_result.lock().expect("send result is available")
        })
    }

    fn destination_public_key(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Option<[u8; 64]>> {
        Box::pin(async move {
            let _drop_signal = DropSignal(&self.public_key_future_dropped);
            self.record(Call::DestinationPublicKey);
            self.public_key_entered.notify_one();
            while self.block_public_key.load(Ordering::Acquire) {
                self.public_key_released.notified().await;
            }
            self.public_keys
                .lock()
                .expect("public-key map is available")
                .get(&destination)
                .copied()
        })
    }

    fn announce(
        &self,
        _destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectAnnounceFailure>> {
        Box::pin(async move {
            self.record(Call::Announce);
            Ok(())
        })
    }
}

#[test]
fn production_adapter_preserves_stale_route_failures_as_no_route() {
    assert_eq!(
        classify_establish_link_failure(SendError::Failed(EstablishLinkFailure::Rejected(
            EstablishLinkRejection::NoRouteToDestination,
        ))),
        DirectSendFailure::NoRoute
    );
    assert_eq!(
        classify_establish_link_failure(SendError::Failed(EstablishLinkFailure::WriteFailed(
            WriteEstablishLinkRejection::RouteVanished,
        ))),
        DirectSendFailure::NoRoute
    );
    assert_eq!(
        classify_establish_link_failure(SendError::Failed(EstablishLinkFailure::Timeout)),
        DirectSendFailure::LinkFailed
    );
    assert_eq!(
        classify_establish_link_failure(SendError::NodeStopped),
        DirectSendFailure::LocalNodeStopped
    );
}

fn identity(secret: &[u8; IDENTITY_SECRET_KEY_LEN]) -> LocalLxmfIdentity {
    LocalLxmfIdentity::from_secret_bytes(secret).expect("lxmf.delivery name is valid")
}

fn peer_facts(secret: &[u8; IDENTITY_SECRET_KEY_LEN]) -> (PrivateIdentityMaterial, [u8; 16]) {
    let material = PrivateIdentityMaterial::from_bytes(*secret);
    let destination = derive_single_destination_hash(
        &material.identity_hash(),
        prns_lxmf_wire::LXMF_APP_NAME,
        prns_lxmf_wire::LXMF_DELIVERY_ASPECTS,
    )
    .expect("lxmf.delivery name is valid");
    (material, *destination.as_bytes())
}

fn current_announce(name: &[u8]) -> Vec<u8> {
    let mut output = [0u8; MAX_ANNOUNCE_APP_DATA_BYTES];
    let length = encode_current_lxmf_announce(name, &mut output)
        .expect("fixture announce fits the callback bound");
    output[..length].to_vec()
}

fn accepted_observation<'a>(
    destination: [u8; 16],
    announced_identity: IdentityHash,
    app_data: &'a [u8],
) -> AnnounceObservation<'a> {
    AnnounceObservation {
        destination: DestinationHash::new(destination),
        announced_identity,
        hops: HopCount(2),
        source_interface: TEST_INTERFACE,
        arrived_at: InstantMillis(4_200),
        app_data,
        is_path_response: false,
    }
}

fn link_event(wire: &[u8]) -> PrnsEvent<'_> {
    PrnsEvent::Message(Message::Delivered(Delivery::Link(LinkDelivery {
        link_id: LinkId::new([0xc7; 16]),
        plaintext: wire,
        arrived_at: InstantMillis(7_700),
        source_interface: TEST_INTERFACE,
    })))
}

async fn wait_for_snapshot(
    service: &DirectLxmfService,
    predicate: impl Fn(&LxmfSnapshot) -> bool,
) -> LxmfSnapshot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let snapshot = service.snapshot().await;
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "service state did not converge: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn learn_peer(
    service: &DirectLxmfService,
    callbacks: &LxmfCallbacks,
    secret: &[u8; IDENTITY_SECRET_KEY_LEN],
) -> (PrivateIdentityMaterial, [u8; 16]) {
    let (material, destination) = peer_facts(secret);
    let announce = current_announce(b"Python peer");
    assert_eq!(
        callbacks.on_accepted_announce(accepted_observation(
            destination,
            material.identity_hash(),
            &announce,
        )),
        CallbackOutcome::Enqueued
    );
    let _snapshot = wait_for_snapshot(service, |snapshot| snapshot.peers.len() == 1).await;
    (material, destination)
}

fn compose_wire(
    signer: &LocalLxmfIdentity,
    destination: [u8; 16],
    timestamp_unix_ms: u64,
    content: &[u8],
) -> Vec<u8> {
    let mut output = [0u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let prepared = compose_basic_direct_lxmf(
        destination,
        signer.destination(),
        timestamp_unix_ms,
        b"subject",
        content,
        None,
        signer,
        &mut output,
    )
    .expect("fixture is one direct message");
    output[..usize::from(prepared.wire_len())].to_vec()
}

#[test]
fn destination_construction_retains_a_zeroizing_signer() {
    let announce = current_announce(b"Local peer");
    let (signer, destination) =
        prepare_local_lxmf_destination(Zeroizing::new(LOCAL_SECRET), &announce)
            .expect("local destination is valid");
    drop(destination);
    let (_peer, peer_destination) = peer_facts(&PEER_SECRET);
    let wire = compose_wire(&signer, peer_destination, 1_700_000_000_000, b"after move");
    let parsed = MessageView::parse_complete(&wire, WireLimits::new(431, 431, 128, 512, 8_192, 16))
        .expect("retained signer emits a parseable message");
    let public_key = signer.public_key();
    let bound = parsed
        .bind_source_identity(&public_key)
        .expect("source binds to retained identity");
    parsed
        .verify_signature(&bound)
        .expect("retained signer signature verifies");
}

#[tokio::test]
async fn authenticated_observer_is_the_only_peer_discovery_lane() {
    let fake = Arc::new(FakeNetwork::default());
    let service =
        DirectLxmfService::start(identity(&LOCAL_SECRET), fake).expect("test owns a Tokio runtime");
    let callbacks = service.callbacks();
    let (peer_material, peer_destination) = peer_facts(&PEER_SECRET);
    let announce = current_announce(b" \0 Python \0 peer \t");

    let diagnostic_event = PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
        destination: DestinationHash::new(peer_destination),
        hops: 2,
        source_interface: TEST_INTERFACE,
        app_data: &announce,
    });
    assert_eq!(
        callbacks.on_prns_event(&diagnostic_event),
        CallbackOutcome::Ignored
    );
    let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = diagnostic_event
    else {
        panic!("the borrowed event remains available to the aggregate consumer");
    };
    assert_eq!(destination, DestinationHash::new(peer_destination));
    assert!(service.snapshot().await.peers.is_empty());

    assert_eq!(
        callbacks.on_accepted_announce(accepted_observation(
            [0xff; 16],
            peer_material.identity_hash(),
            &announce,
        )),
        CallbackOutcome::InvalidDestinationAssociation
    );
    assert_eq!(
        callbacks.on_accepted_announce(accepted_observation(
            peer_destination,
            peer_material.identity_hash(),
            &announce,
        )),
        CallbackOutcome::Enqueued
    );
    let snapshot = wait_for_snapshot(&service, |snapshot| snapshot.peers.len() == 1).await;
    assert_eq!(
        snapshot.peers[0].announced_identity,
        *peer_material.identity_hash().as_bytes()
    );
    assert_eq!(snapshot.peers[0].observed_at_millis, 4_200);
    assert!(!snapshot.peers[0].is_path_response);
    assert_eq!(
        snapshot.peers[0].display_name.as_deref(),
        Some("Python  peer")
    );
    service.stop().await.expect("service tasks stop promptly");
}

#[tokio::test]
async fn direct_send_is_single_attempt_and_delivered_only_after_proof_result() {
    let fake = Arc::new(FakeNetwork::default());
    fake.block_send.store(true, Ordering::Release);
    let service = DirectLxmfService::start(identity(&LOCAL_SECRET), fake.clone())
        .expect("test owns a Tokio runtime");
    let callbacks = service.callbacks();
    let (_peer, peer_destination) = learn_peer(&service, &callbacks, &PEER_SECRET).await;

    let send_service = service.clone();
    let send = tokio::spawn(async move {
        send_service
            .send_direct_text(peer_destination, 1_700_000_000_000, b"", &[0x42; 319])
            .await
    });
    fake.send_entered.notified().await;
    let sending = service.snapshot().await;
    assert_eq!(
        sending.messages[0].delivery_state,
        LxmfDeliveryState::Sending
    );
    assert_eq!(sending.messages[0].exact_wire.len(), 431);

    fake.release_send();
    let outcome = send.await.expect("send task completes after proof");
    let SendDirectTextOutcome::Started { local_record_id } = outcome else {
        panic!("319-byte selector input must complete from proof: {outcome:?}");
    };
    let delivered = service.snapshot().await;
    assert_eq!(delivered.messages[0].local_record_id, local_record_id);
    assert_eq!(
        fake.calls(),
        vec![Call::HasRoute, Call::EstablishLink, Call::SendLinkPacket]
    );
    assert_eq!(
        fake.sent_wires.lock().expect("sent-wire log is available")[0].len(),
        431
    );

    assert_eq!(
        service
            .send_direct_text(peer_destination, 1_700_000_000_000, b"", &[0x42; 320])
            .await,
        SendDirectTextOutcome::NeedsResource { wire_bytes: 432 }
    );
    service.stop().await.expect("service tasks stop promptly");
}

async fn assert_failure(
    has_route: bool,
    request_result: Result<(), DirectSendFailure>,
    establish_result: Result<[u8; 16], DirectSendFailure>,
    send_result: Result<(), DirectSendFailure>,
    expected: DirectSendFailure,
    expected_outcome: SendDirectTextOutcome,
    expected_calls: Vec<Call>,
) {
    let fake = Arc::new(FakeNetwork::default());
    fake.has_route.store(has_route, Ordering::Release);
    *fake
        .request_result
        .lock()
        .expect("request result is available") = request_result;
    *fake
        .establish_result
        .lock()
        .expect("establish result is available") = establish_result;
    *fake.send_result.lock().expect("send result is available") = send_result;
    let service = DirectLxmfService::start(identity(&LOCAL_SECRET), fake.clone())
        .expect("test owns a Tokio runtime");
    let callbacks = service.callbacks();
    let (_peer, peer_destination) = learn_peer(&service, &callbacks, &PEER_SECRET).await;
    assert_eq!(
        service
            .send_direct_text(peer_destination, 1_700_000_000_000, b"failure", b"fixture")
            .await,
        expected_outcome
    );
    let failed = service.snapshot().await;
    assert_eq!(failed.messages[0].failure, Some(expected));
    assert_eq!(fake.calls(), expected_calls);
    service.stop().await.expect("service tasks stop promptly");
}

#[tokio::test]
async fn direct_send_retains_typed_single_attempt_failures() {
    assert_failure(
        false,
        Err(DirectSendFailure::NoRoute),
        Ok([0xa5; 16]),
        Ok(()),
        DirectSendFailure::NoRoute,
        SendDirectTextOutcome::NoRoute,
        vec![Call::HasRoute, Call::RequestPath],
    )
    .await;
    assert_failure(
        true,
        Ok(()),
        Err(DirectSendFailure::LinkFailed),
        Ok(()),
        DirectSendFailure::LinkFailed,
        SendDirectTextOutcome::LinkFailed,
        vec![Call::HasRoute, Call::EstablishLink],
    )
    .await;
    assert_failure(
        true,
        Ok(()),
        Ok([0xa5; 16]),
        Err(DirectSendFailure::DeliveryTimedOut),
        DirectSendFailure::DeliveryTimedOut,
        SendDirectTextOutcome::DeliveryTimedOut,
        vec![Call::HasRoute, Call::EstablishLink, Call::SendLinkPacket],
    )
    .await;
    assert_failure(
        true,
        Ok(()),
        Ok([0xa5; 16]),
        Err(DirectSendFailure::LocalNodeStopped),
        DirectSendFailure::LocalNodeStopped,
        SendDirectTextOutcome::LocalNodeStopped,
        vec![Call::HasRoute, Call::EstablishLink, Call::SendLinkPacket],
    )
    .await;
}

#[tokio::test]
async fn stop_cancels_and_joins_inflight_send_and_worker() {
    let fake = Arc::new(FakeNetwork::default());
    fake.block_send.store(true, Ordering::Release);
    let service = DirectLxmfService::start(identity(&LOCAL_SECRET), fake.clone())
        .expect("test owns a Tokio runtime");
    let callbacks = service.callbacks();
    let (_peer, peer_destination) = learn_peer(&service, &callbacks, &PEER_SECRET).await;
    let send_service = service.clone();
    let send = tokio::spawn(async move {
        send_service
            .send_direct_text(
                peer_destination,
                1_700_000_000_000,
                b"cancelled",
                b"fixture",
            )
            .await
    });
    fake.send_entered.notified().await;
    service.stop().await.expect("cancelled tasks join promptly");
    assert_eq!(
        send.await.expect("send waiter observes cancellation"),
        SendDirectTextOutcome::LocalNodeStopped
    );
    assert!(fake.send_future_dropped.load(Ordering::Acquire));
    let stopped = service.snapshot().await;
    assert_eq!(stopped.health.state, LxmfHealthState::Stopped);
    assert_eq!(
        stopped.messages[0].delivery_state,
        LxmfDeliveryState::Failed
    );
    assert_eq!(
        stopped.messages[0].failure,
        Some(DirectSendFailure::LocalNodeStopped)
    );

    let worker_fake = Arc::new(FakeNetwork::default());
    worker_fake.block_public_key.store(true, Ordering::Release);
    let local = identity(&LOCAL_SECRET);
    let local_destination = local.destination();
    let worker_service =
        DirectLxmfService::start(local, worker_fake.clone()).expect("test owns a Tokio runtime");
    let worker_callbacks = worker_service.callbacks();
    let wire = compose_wire(
        &identity(&PEER_SECRET),
        local_destination,
        1_700_000_000_000,
        b"cancel worker",
    );
    assert_eq!(
        worker_callbacks.on_prns_event(&link_event(&wire)),
        CallbackOutcome::Enqueued
    );
    worker_fake.public_key_entered.notified().await;
    worker_service
        .stop()
        .await
        .expect("in-flight worker joins promptly");
    assert!(worker_fake
        .public_key_future_dropped
        .load(Ordering::Acquire));
    assert!(worker_service.snapshot().await.messages.is_empty());
}

#[tokio::test]
async fn inbound_messages_retain_verification_exact_wire_and_logical_dedup() {
    let fake = Arc::new(FakeNetwork::default());
    let local = identity(&LOCAL_SECRET);
    let local_destination = local.destination();
    let service = DirectLxmfService::start(local, fake.clone()).expect("test owns a Tokio runtime");
    let callbacks = service.callbacks();

    let verified_signer = identity(&PEER_SECRET);
    let unknown_signer = identity(&OTHER_SECRET);
    let invalid_signer = identity(&[0x94; IDENTITY_SECRET_KEY_LEN]);
    let verified_wire = compose_wire(
        &verified_signer,
        local_destination,
        1_700_000_000_001,
        b"verified",
    );
    let unknown_wire = compose_wire(
        &unknown_signer,
        local_destination,
        1_700_000_000_002,
        b"unknown",
    );
    let invalid_wire = compose_wire(
        &invalid_signer,
        local_destination,
        1_700_000_000_003,
        b"invalid",
    );
    let verified_material = PrivateIdentityMaterial::from_bytes(PEER_SECRET);
    fake.add_public_key(verified_signer.destination(), &verified_material);
    let wrong_material = PrivateIdentityMaterial::from_bytes([0xb5; IDENTITY_SECRET_KEY_LEN]);
    fake.add_public_key(invalid_signer.destination(), &wrong_material);

    assert_eq!(
        callbacks.on_prns_event(&link_event(&verified_wire)),
        CallbackOutcome::Enqueued
    );
    assert_eq!(
        callbacks.on_prns_event(&link_event(&unknown_wire)),
        CallbackOutcome::Enqueued
    );
    assert_eq!(
        callbacks.on_prns_event(&link_event(&invalid_wire)),
        CallbackOutcome::Enqueued
    );
    let snapshot = wait_for_snapshot(&service, |snapshot| snapshot.messages.len() == 3).await;
    assert_eq!(
        snapshot.messages[0].verification,
        LxmfVerification::Verified
    );
    assert_eq!(
        snapshot.messages[1].verification,
        LxmfVerification::SourceUnknown
    );
    assert_eq!(
        snapshot.messages[2].verification,
        LxmfVerification::InvalidSignature
    );
    assert_eq!(snapshot.messages[0].exact_wire, verified_wire);
    assert_eq!(snapshot.messages[0].arrived_at_millis, Some(7_700));
    assert_eq!(
        snapshot.messages[0].source_interface,
        Some(*TEST_INTERFACE.as_bytes())
    );

    assert_eq!(
        callbacks.on_prns_event(&link_event(&verified_wire)),
        CallbackOutcome::Enqueued
    );
    let dedup_barrier = compose_wire(
        &verified_signer,
        local_destination,
        1_700_000_000_004,
        b"dedup barrier",
    );
    assert_eq!(
        callbacks.on_prns_event(&link_event(&dedup_barrier)),
        CallbackOutcome::Enqueued
    );
    let after_duplicate =
        wait_for_snapshot(&service, |snapshot| snapshot.messages.len() == 4).await;
    let verified_id = after_duplicate.messages[0].message_id;
    assert_eq!(
        after_duplicate
            .messages
            .iter()
            .filter(|message| message.message_id == verified_id)
            .count(),
        1
    );

    let mut stamped = verified_wire;
    stamped[96] = 0x95;
    stamped.extend_from_slice(&[0xc4, 0x10]);
    stamped.extend_from_slice(&[0x77; 16]);
    let parsed_stamp =
        MessageView::parse_complete(&stamped, WireLimits::new(431, 431, 128, 512, 8_192, 16))
            .expect("the stamped fixture reaches the service-layer exclusion");
    assert!(parsed_stamp.payload().stamp().is_some());
    assert_eq!(
        callbacks.on_prns_event(&link_event(&stamped)),
        CallbackOutcome::Enqueued
    );
    let stamp_barrier = compose_wire(
        &verified_signer,
        local_destination,
        1_700_000_000_005,
        b"stamp barrier",
    );
    assert_eq!(
        callbacks.on_prns_event(&link_event(&stamp_barrier)),
        CallbackOutcome::Enqueued
    );
    let after_stamp = wait_for_snapshot(&service, |snapshot| snapshot.messages.len() == 5).await;
    assert!(after_stamp
        .messages
        .iter()
        .all(|message| message.exact_wire != stamped));
    service.stop().await.expect("service tasks stop promptly");
}

#[tokio::test]
async fn bounded_lane_saturates_reports_loss_recovers_and_restarts_empty() {
    let (pending, callbacks) = DirectLxmfService::prepare(identity(&LOCAL_SECRET));
    let refresh = callbacks.health.refresh.subscribe();
    let invalid_wire = [0u8; 1];
    for _ in 0..DIRECT_JOB_CAPACITY {
        assert_eq!(
            callbacks.on_prns_event(&link_event(&invalid_wire)),
            CallbackOutcome::Enqueued
        );
    }
    assert_eq!(
        callbacks.on_prns_event(&link_event(&invalid_wire)),
        CallbackOutcome::LaneFull
    );
    assert_eq!(callbacks.health.snapshot().state, LxmfHealthState::Degraded);
    assert_eq!(callbacks.health.snapshot().inbound_overflow_count, 1);
    assert_eq!(*refresh.borrow(), 1);
    callbacks
        .health
        .inbound_overflow_count
        .store(u64::MAX, Ordering::Release);
    assert_eq!(
        callbacks.on_prns_event(&link_event(&invalid_wire)),
        CallbackOutcome::LaneFull
    );
    assert_eq!(callbacks.health.snapshot().inbound_overflow_count, u64::MAX);
    assert_eq!(*refresh.borrow(), 1);

    let service = pending
        .start(Arc::new(FakeNetwork::default()))
        .expect("test owns a Tokio runtime");
    let recovered = wait_for_snapshot(&service, |snapshot| {
        snapshot.health.state == LxmfHealthState::Ready
    })
    .await;
    assert_eq!(recovered.health.inbound_overflow_count, u64::MAX);
    service.stop().await.expect("service tasks stop promptly");
    assert_eq!(
        callbacks.on_prns_event(&link_event(&invalid_wire)),
        CallbackOutcome::Stopped
    );

    let restarted =
        DirectLxmfService::start(identity(&LOCAL_SECRET), Arc::new(FakeNetwork::default()))
            .expect("test owns a Tokio runtime");
    let restarted_snapshot = restarted.snapshot().await;
    assert!(restarted_snapshot.peers.is_empty());
    assert!(restarted_snapshot.messages.is_empty());
    assert_eq!(restarted_snapshot.health.inbound_overflow_count, 0);
    restarted
        .stop()
        .await
        .expect("restarted service tasks stop promptly");
}

#[tokio::test]
async fn stamped_peer_is_visible_but_send_is_explicitly_unsupported() {
    let fake = Arc::new(FakeNetwork::default());
    let service =
        DirectLxmfService::start(identity(&LOCAL_SECRET), fake).expect("test owns a Tokio runtime");
    let callbacks = service.callbacks();
    let (peer_material, peer_destination) = peer_facts(&PEER_SECRET);
    let stamped_announce = [0x93, 0xc4, 0x04, b's', b't', b'a', b'm', 0x05, 0x90];
    assert_eq!(
        callbacks.on_accepted_announce(accepted_observation(
            peer_destination,
            peer_material.identity_hash(),
            &stamped_announce,
        )),
        CallbackOutcome::Enqueued
    );
    let snapshot = wait_for_snapshot(&service, |snapshot| snapshot.peers.len() == 1).await;
    assert_eq!(snapshot.peers[0].required_stamp_cost, Some(5));
    assert_eq!(
        service
            .send_direct_text(peer_destination, 1_700_000_000_000, b"", b"blocked")
            .await,
        SendDirectTextOutcome::UnsupportedRemoteStampRequirement
    );
    service.stop().await.expect("service tasks stop promptly");
}
