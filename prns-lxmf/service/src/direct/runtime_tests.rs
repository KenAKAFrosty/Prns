//! Exercise the injected engine with no concrete host or mailbox adapter.
use super::*;
use crate::wire;
use personal_rns::{identity::PrivateIdentityMaterial, routing::announce::AnnounceObservation};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::sync::Notify;

#[derive(Default)]
struct Network {
    block: AtomicBool,
    panic_send: AtomicBool,
    calls: AtomicUsize,
    released: Notify,
}
impl DirectNetwork for Network {
    fn has_route(&self, _: [u8; 16]) -> DirectNetworkFuture<'_, bool> {
        Box::pin(async { true })
    }
    fn request_path(&self, _: [u8; 16]) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>> {
        Box::pin(async { Ok(()) })
    }
    fn establish_link(
        &self,
        _: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<[u8; 16], DirectSendFailure>> {
        Box::pin(async { Ok([5; 16]) })
    }
    fn send_link_packet(
        &self,
        _: [u8; 16],
        _: Vec<u8>,
    ) -> DirectNetworkFuture<'_, Result<DirectDeliveryReceipt, DirectSendFailure>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(
                !self.panic_send.load(Ordering::SeqCst),
                "injected send failure"
            );
            while self.block.load(Ordering::SeqCst) {
                self.released.notified().await;
            }
            Ok(DirectDeliveryReceipt { rtt_millis: 1 })
        })
    }
    fn destination_public_key(&self, _: [u8; 16]) -> DirectNetworkFuture<'_, Option<[u8; 64]>> {
        Box::pin(async {
            Some(
                *PrivateIdentityMaterial::from_bytes([2; 64])
                    .public()
                    .as_bytes(),
            )
        })
    }
    fn mark_destination_used(&self, _: [u8; 16]) -> DirectNetworkFuture<'_, ()> {
        Box::pin(async {})
    }
    fn announce(&self, _: [u8; 16]) -> DirectNetworkFuture<'_, Result<(), DirectAnnounceFailure>> {
        Box::pin(async { Ok(()) })
    }
}
async fn service(limits: DirectLimits) -> (DirectLxmfService, Arc<Network>, [u8; 16]) {
    let identity = LocalLxmfIdentity::from_secret_bytes(&[1; 64]).unwrap();
    let (pending, callbacks) = DirectLxmfService::prepare_with_limits(identity, limits).unwrap();
    let network = Arc::new(Network::default());
    let service = pending.start(network.clone()).unwrap();
    let peer = PrivateIdentityMaterial::from_bytes([2; 64]);
    let destination = derive_single_destination_hash(
        &peer.identity_hash(),
        wire::LXMF_APP_NAME,
        wire::LXMF_DELIVERY_ASPECTS,
    )
    .unwrap();
    assert_eq!(
        callbacks.on_accepted_announce(&AnnounceObservation {
            destination,
            announced_identity: peer.identity_hash(),
            hops: personal_rns::units::HopCount(1),
            source_interface: personal_rns::interfaces::InterfaceId::new([0; 8]),
            arrived_at: personal_rns::units::InstantMillis(0),
            app_data: &[],
            is_path_response: false
        }),
        CallbackOutcome::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while service.snapshot().await.peers.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    (service, network, *destination.as_bytes())
}

#[tokio::test]
async fn selected_history_capacity_preserves_records_and_refuses_new_admission() {
    let (service, _, peer) = service(DirectLimits {
        max_messages: 1,
        max_message_bytes: 8192,
        max_active_attempts: 1,
    })
    .await;
    assert!(matches!(
        service.send_direct_text(peer, 1, b"one", b"kept").await,
        SendDirectTextOutcome::Started { .. }
    ));
    assert_eq!(
        service.send_direct_text(peer, 2, b"two", b"full").await,
        SendDirectTextOutcome::HistoryFull
    );
    let snapshot = service.snapshot().await;
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.messages[0].content, b"kept");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn inbound_overflow_is_visible_and_does_not_claim_dedup_admission() {
    let (service, _, peer) = service(DirectLimits {
        max_messages: 1,
        max_message_bytes: 8192,
        max_active_attempts: 1,
    })
    .await;
    assert!(matches!(
        service.send_direct_text(peer, 1, b"one", b"kept").await,
        SendDirectTextOutcome::Started { .. }
    ));
    let signer = LocalLxmfIdentity::from_secret_bytes(&[2; 64]).unwrap();
    let mut bytes = [0; MAX_BASIC_LXMF_WIRE_BYTES];
    let encoded = compose_basic_direct_lxmf(
        service.local_destination(),
        peer,
        2,
        b"incoming",
        b"full",
        None,
        &signer,
        &mut bytes,
    )
    .unwrap();
    let message_id = encoded.message_id();
    let wire = &bytes[..usize::from(encoded.wire_len())];
    assert_eq!(
        service.callbacks().on_scoped_link_delivery(
            Some(service.local_destination()),
            wire,
            0,
            [0; 8]
        ),
        CallbackOutcome::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while service.snapshot().await.health.inbound_overflow_count == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let state = service.shared.state.lock().await;
    assert_eq!(state.messages.len(), 1);
    assert!(!state.seen_message_ids.contains(&message_id));
    drop(state);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn caller_loss_does_not_release_an_active_attempt_budget_early() {
    let (service, network, peer) = service(DirectLimits {
        max_messages: 4,
        max_message_bytes: 16384,
        max_active_attempts: 1,
    })
    .await;
    network.block.store(true, Ordering::SeqCst);
    let sender = service.clone();
    let waiter =
        tokio::spawn(async move { sender.send_direct_text(peer, 1, b"one", b"pending").await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while network.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    waiter.abort();
    let _ = waiter.await;
    assert_eq!(
        service.send_direct_text(peer, 2, b"two", b"blocked").await,
        SendDirectTextOutcome::Busy
    );
    assert_eq!(service.snapshot().await.messages.len(), 1);
    network.block.store(false, Ordering::SeqCst);
    network.released.notify_waiters();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !service.lifecycle.attempts().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        service.send_direct_text(peer, 3, b"three", b"next").await,
        SendDirectTextOutcome::Started { .. }
    ));
    assert_eq!(network.calls.load(Ordering::SeqCst), 2);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn cancelled_stop_retains_join_boundary_for_retry() {
    let (service, _, _) = service(DirectLimits::default()).await;
    let state = service.shared.state.lock().await;
    let mut first = Box::pin(service.stop());
    std::future::poll_fn(|cx| {
        assert!(first.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(first);
    assert!(service.lifecycle.worker().is_some());
    drop(state);

    // No executor yield has occurred since cancellation. The aborted worker
    // still needs to finish before any retry can acknowledge cleanup.
    let mut retry = Box::pin(service.stop());
    std::future::poll_fn(|cx| {
        assert!(retry.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    retry.await.unwrap();
    assert!(service.lifecycle.worker().is_none());
    assert!(service.lifecycle.attempts().is_empty());
    assert_eq!(service.stop().await, Ok(()));
}

#[tokio::test(start_paused = true)]
async fn stop_timeout_retains_worker_for_retry() {
    let (service, _, _) = service(DirectLimits::default()).await;
    let state = service.shared.state.lock().await;
    assert_eq!(
        tokio::time::timeout(STOP_JOIN_TIMEOUT + Duration::from_secs(1), service.stop()).await,
        Ok(Err(DirectServiceStopError::JoinTimedOut))
    );
    assert!(service.lifecycle.worker().is_some());
    drop(state);
    assert_eq!(service.stop().await, Ok(()));
    assert!(service.lifecycle.worker().is_none());
}

#[tokio::test]
async fn failed_worker_remains_a_failed_stop_on_retry() {
    let (service, _, _) = service(DirectLimits::default()).await;
    let worker = service.lifecycle.worker().take().unwrap();
    worker.abort();
    let _ = worker.await;
    *service.lifecycle.worker() = Some(tokio::spawn(async {
        panic!("injected callback worker failure");
    }));
    while !service.lifecycle.worker().as_ref().unwrap().is_finished() {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        service.stop().await,
        Err(DirectServiceStopError::WorkerFailed)
    );
    assert_eq!(
        service.stop().await,
        Err(DirectServiceStopError::WorkerFailed)
    );
}

#[tokio::test]
async fn failed_attempt_observed_by_sender_remains_a_failed_stop_on_retry() {
    let (service, network, peer) = service(DirectLimits::default()).await;
    network.panic_send.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .send_direct_text(peer, 1, b"failed", b"attempt")
            .await,
        SendDirectTextOutcome::LocalNodeStopped
    );
    assert!(service.lifecycle.attempts().is_empty());
    assert_eq!(
        service.stop().await,
        Err(DirectServiceStopError::WorkerFailed)
    );
    assert_eq!(
        service.stop().await,
        Err(DirectServiceStopError::WorkerFailed)
    );
}
