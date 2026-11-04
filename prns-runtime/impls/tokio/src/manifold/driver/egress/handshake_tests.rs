use super::super::interface_status::TokioInterfaceStatus;
use super::*;
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::interfaces::{ConnectionState, IfacContext, IfacSize};
use crate::manifold::grant_lane::tokio_grant_lane;
use crate::routing::links::{handshake::write_link_request, LinkId, LinkMode};
use crate::wire::DestinationHash;
use crate::wire::{ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType};
use std::cell::RefCell;

thread_local! { static EVENTS: RefCell<Vec<EgressHandshakeEvent>> = const { RefCell::new(Vec::new()) }; }
fn accept(event: EgressHandshakeEvent) -> bool {
    EVENTS.with(|events| events.borrow_mut().push(event));
    true
}
fn reject(_: EgressHandshakeEvent) -> bool {
    false
}
fn take_events() -> Vec<EgressHandshakeEvent> {
    EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}
fn frames() -> (Vec<u8>, Vec<u8>, LinkId) {
    let destination = DestinationHash::new([0x31; 16]);
    let encryption = X25519PublicKey([0x32; 32]);
    let signing = Ed25519PublicKey([0x33; 32]);
    let link = LinkId::derive(&destination, &encryption, &signing);
    let mut request = vec![0; 128];
    let len = write_link_request(
        &destination,
        None,
        &encryption,
        &signing,
        500,
        LinkMode::Aes256Cbc,
        &mut request,
    )
    .unwrap();
    request.truncate(len);
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Proof,
        hops: 0,
        transport_id: None,
        address: link.to_address(),
        context: WireContext::LinkRequestProof,
    };
    let mut proof = vec![0; 128];
    let len = header.write(&mut proof).unwrap();
    proof.truncate(len + 96);
    (request, proof, link)
}
fn configured(id: InterfaceId) -> (Egress, crate::manifold::driver::TokioGrantConsumer) {
    take_events();
    let (producer, consumer) = tokio_grant_lane(512, 1);
    let mut egress = Egress::new(vec![(id, producer)]);
    egress.set_handshake_observer(Some(accept));
    (egress, consumer)
}

#[test]
fn handshake_admission_tracks_request_and_pending_proof_without_claiming_transmission() {
    let id = InterfaceId::new([0x41; 8]);
    let (request, proof, link) = frames();
    let (mut egress, mut consumer) = configured(id);
    assert_eq!(egress.enqueue(id, &request), EgressEnqueueOutcome::Enqueued);
    assert_eq!(egress.enqueue(id, &proof), EgressEnqueueOutcome::Deferred);
    let admitted = take_events();
    assert_eq!(admitted.len(), 2);
    assert_eq!(admitted[0].phase, EgressHandshakePhase::Enqueued);
    assert_eq!(admitted[0].packet, Some(EgressHandshakePacket::Request));
    assert_eq!(admitted[1].phase, EgressHandshakePhase::Deferred);
    assert_eq!(admitted[1].packet, Some(EgressHandshakePacket::Proof));
    assert!(admitted.iter().all(|event| event.link_id == Some(link)));
    assert_eq!(consumer.try_peek().unwrap().frame(), request);
    consumer.release();
    assert_eq!(egress.flush_pending(1), 1);
    let flushed = take_events();
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0].phase, EgressHandshakePhase::Flushed);
    assert_eq!(flushed[0].sequence, 3);
    assert_eq!(flushed[0].link_id, Some(link));
    assert!(flushed[0].queue_age_ms.is_some());
    assert_eq!(consumer.try_peek().unwrap().frame(), proof);
}

#[test]
fn handshake_admission_distinguishes_full_missing_offline_and_ifac_refusal() {
    let id = InterfaceId::new([0x42; 8]);
    let missing = InterfaceId::new([0x43; 8]);
    let offline = InterfaceId::new([0x44; 8]);
    let (request, _, link) = frames();
    let (mut egress, _consumer) = configured(id);
    assert_eq!(
        egress.enqueue(id, b"occupied"),
        EgressEnqueueOutcome::Enqueued
    );
    for _ in 0..TOKIO_EGRESS_PENDING_DEPTH {
        assert_eq!(
            egress.enqueue(id, b"pending"),
            EgressEnqueueOutcome::Deferred
        );
    }
    assert_eq!(
        egress.enqueue(id, &request),
        EgressEnqueueOutcome::DroppedFull
    );
    assert_eq!(
        egress.enqueue(missing, &request),
        EgressEnqueueOutcome::LaneMissing
    );
    let (producer, _rx) = tokio_grant_lane(512, 1);
    let status = TokioInterfaceStatus::new_unaccounted(offline, ConnectionState::Disconnected);
    egress.add_lane(offline, offline, producer, Some(ConnectionView::of(status)));
    assert_eq!(
        egress.enqueue(offline, &request),
        EgressEnqueueOutcome::Unavailable
    );
    let ifacs: InterfaceIfacs = vec![InterfaceIfac {
        id,
        context: IfacContext::derive(Some("handshake"), Some("test"), IfacSize::NARROW).unwrap(),
    }]
    .into();
    enqueue_for_wire(&mut egress, &ifacs, id, &request, &mut [0; 1]);
    let events = take_events();
    assert_eq!(
        events.iter().map(|event| event.phase).collect::<Vec<_>>(),
        vec![
            EgressHandshakePhase::DroppedFull,
            EgressHandshakePhase::LaneMissing,
            EgressHandshakePhase::Unavailable,
            EgressHandshakePhase::IfacRejected,
        ]
    );
    assert!(events.iter().all(|event| event.link_id == Some(link)));
    assert_eq!(egress.handshake_snapshot().attempted, 4);
}

#[test]
fn handshake_pending_discard_and_masked_flush_keep_unmasked_correlation() {
    let id = InterfaceId::new([0x45; 8]);
    let (request, proof, link) = frames();
    let (mut egress, mut consumer) = configured(id);
    let ifacs: InterfaceIfacs = vec![InterfaceIfac {
        id,
        context: IfacContext::derive(Some("handshake"), Some("mask"), IfacSize::NARROW).unwrap(),
    }]
    .into();
    let mut masked = [0; 512];
    enqueue_for_wire(&mut egress, &ifacs, id, &request, &mut masked);
    enqueue_for_wire(&mut egress, &ifacs, id, &proof, &mut masked);
    assert_ne!(consumer.try_peek().unwrap().frame(), request);
    consumer.release();
    assert_eq!(egress.flush_pending(1), 1);
    enqueue_for_wire(&mut egress, &ifacs, id, &request, &mut masked);
    egress.remove_lane(id);
    let events = take_events();
    assert_eq!(
        events.iter().map(|event| event.phase).collect::<Vec<_>>(),
        vec![
            EgressHandshakePhase::Enqueued,
            EgressHandshakePhase::Deferred,
            EgressHandshakePhase::Flushed,
            EgressHandshakePhase::Deferred,
            EgressHandshakePhase::PendingDiscarded,
        ]
    );
    assert!(events.iter().all(|event| event.link_id == Some(link)));
    assert_eq!(egress.pending_frames, 0);
}

#[test]
fn handshake_disconnect_reports_pending_retirement_before_offline_rejection() {
    let id = InterfaceId::new([0x46; 8]);
    let (request, _, _) = frames();
    let (producer, _rx) = tokio_grant_lane(512, 1);
    let status = TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected);
    let mut egress = Egress::new(vec![]);
    egress.set_handshake_observer(Some(accept));
    take_events();
    egress.add_lane(id, id, producer, Some(ConnectionView::of(status.clone())));
    egress.enqueue(id, &request);
    egress.enqueue(id, &request);
    take_events();
    status.set_connection(ConnectionState::Disconnected);
    assert_eq!(
        egress.enqueue(id, &request),
        EgressEnqueueOutcome::Unavailable
    );
    assert_eq!(
        take_events()
            .iter()
            .map(|event| event.phase)
            .collect::<Vec<_>>(),
        vec![
            EgressHandshakePhase::PendingDiscarded,
            EgressHandshakePhase::Unavailable,
        ]
    );
}

#[test]
fn handshake_diagnostics_default_off_and_budget_never_reset_on_reenable() {
    let id = InterfaceId::new([0x47; 8]);
    let (request, _, _) = frames();
    let mut egress = Egress::new(vec![]);
    egress.enqueue(id, &request);
    assert_eq!(
        egress.handshake_snapshot(),
        EgressHandshakeSnapshot::default()
    );
    take_events();
    egress.set_handshake_observer(Some(accept));
    for _ in 0..EGRESS_HANDSHAKE_EVENT_LIMIT + 2 {
        egress.enqueue(id, &request);
    }
    let events = take_events();
    assert_eq!(events.len() as u64, EGRESS_HANDSHAKE_EVENT_LIMIT + 1);
    let cap = events.last().unwrap();
    assert_eq!(cap.phase, EgressHandshakePhase::LimitReached);
    assert!(cap.link_id.is_none());
    assert_eq!(cap.snapshot.producer_dropped, 1);
    assert_eq!(egress.handshake_snapshot().producer_dropped, 2);
    egress.set_handshake_observer(None);
    egress.set_handshake_observer(Some(accept));
    egress.enqueue(id, &request);
    assert!(take_events().is_empty());
    assert_eq!(egress.handshake_snapshot().producer_dropped, 3);
}

#[test]
fn handshake_sink_rejections_are_counted_separately_from_producer_caps() {
    let id = InterfaceId::new([0x48; 8]);
    let (request, _, _) = frames();
    let mut egress = Egress::new(vec![]);
    egress.set_handshake_observer(Some(reject));
    egress.enqueue(id, &request);
    assert_eq!(egress.handshake_snapshot().sink_dropped, 1);
    assert_eq!(egress.handshake_snapshot().emitted, 0);
    assert_eq!(egress.handshake_snapshot().producer_dropped, 0);
    egress.set_handshake_observer(Some(accept));
    take_events();
    egress.enqueue(id, &request);
    assert_eq!(take_events()[0].snapshot.sink_dropped, 1);
}

#[test]
fn handshake_forward_copy_and_move_paths_preserve_link_token() {
    let id = InterfaceId::new([0x49; 8]);
    let missing = InterfaceId::new([0x50; 8]);
    let (_, proof, link) = frames();
    let (header, body) = WirePacketHeader::parse(&proof).unwrap();
    let (mut egress, mut consumer) = configured(id);
    let mut scratch = WireScratch::new(512);
    forward_for_wire(
        &mut egress,
        &InterfaceIfacs::default(),
        id,
        header,
        body,
        &mut scratch,
    );
    assert_eq!(consumer.try_peek().unwrap().frame(), proof);
    consumer.release();
    forward_for_wire(
        &mut egress,
        &InterfaceIfacs::default(),
        missing,
        header,
        body,
        &mut scratch,
    );

    let (mut source_tx, mut source_rx) = tokio_grant_lane(512, 1);
    source_tx.try_grant().unwrap().fill(&proof);
    source_tx.commit();
    source_rx.try_peek().unwrap();
    let source = source_rx.take_peeked().unwrap();
    assert!(matches!(
        egress.try_move_forwarded_slot(id, header, header.wire_len()..proof.len(), source),
        ForwardedSlotOutcome::Moved { .. }
    ));
    assert_eq!(consumer.try_peek().unwrap().frame(), proof);
    consumer.release();

    let ifacs: InterfaceIfacs = vec![InterfaceIfac {
        id,
        context: IfacContext::derive(Some("forward"), Some("mask"), IfacSize::NARROW).unwrap(),
    }]
    .into();
    forward_for_wire(&mut egress, &ifacs, id, header, body, &mut scratch);
    assert_ne!(consumer.try_peek().unwrap().frame(), proof);
    let events = take_events();
    assert_eq!(
        events.iter().map(|event| event.phase).collect::<Vec<_>>(),
        vec![
            EgressHandshakePhase::Enqueued,
            EgressHandshakePhase::LaneMissing,
            EgressHandshakePhase::Enqueued,
            EgressHandshakePhase::Enqueued,
        ]
    );
    assert!(events
        .iter()
        .all(|event| event.link_id == Some(link)
            && event.packet == Some(EgressHandshakePacket::Proof)));
}

#[test]
fn handshake_emit_keeps_deferred_admission_when_a_slot_frees_during_fill() {
    let id = InterfaceId::new([0x51; 8]);
    let (_, proof, link) = frames();
    let (mut egress, mut consumer) = configured(id);
    assert_eq!(
        egress.enqueue(id, b"occupied"),
        EgressEnqueueOutcome::Enqueued
    );
    let mut scratch = [0; 512];
    egress.emit(
        id,
        proof.len(),
        &mut |output| {
            assert_eq!(consumer.try_peek().unwrap().frame(), b"occupied");
            consumer.release();
            output[..proof.len()].copy_from_slice(&proof);
            Some(proof.len())
        },
        &mut scratch,
    );
    assert!(
        consumer.try_peek().is_none(),
        "the original failed grant still chooses the pending queue"
    );
    assert!(egress.has_pending());
    let events = take_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].phase, EgressHandshakePhase::Deferred);
    assert_eq!(events[0].link_id, Some(link));
    assert_eq!(egress.flush_pending(1), 1);
    assert_eq!(consumer.try_peek().unwrap().frame(), proof);
    assert_eq!(take_events()[0].phase, EgressHandshakePhase::Flushed);
}
