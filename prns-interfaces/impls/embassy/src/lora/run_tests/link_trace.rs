use super::*;
use crate::lora::link_trace::{event, identity};
use prns_core::wire::WireContext;

std::thread_local! {
    static EVENTS: RefCell<Vec<LoRaLinkEvent>> = const { RefCell::new(Vec::new()) };
}
fn record(event: LoRaLinkEvent) -> bool {
    EVENTS.with_borrow_mut(|events| events.push(event));
    true
}
fn events() -> Vec<LoRaLinkEvent> {
    EVENTS.with_borrow(Clone::clone)
}
fn clear() {
    EVENTS.with_borrow_mut(Vec::clear);
}
fn request() -> Vec<u8> {
    let mut raw = vec![0; 83];
    raw[0] = 2;
    raw[2..18].fill(7);
    raw[19..].fill(9);
    raw
}
fn link_data(len: usize) -> Vec<u8> {
    let mut raw = vec![0; len];
    raw[0] = 12;
    raw[2..18].fill(8);
    raw
}

#[test]
fn request_and_proof_correlation_ignores_transport_and_hops_and_rejects_ifac() {
    let raw = request();
    let (kind, id) = identity(&raw);
    assert_eq!(kind, LoRaTracePacket::Request);
    assert!(id.is_some());
    let mut routed = raw.clone();
    routed[0] |= 0x50;
    routed[1] = 5;
    routed.splice(2..2, [4; 16]);
    assert_eq!(identity(&routed), (kind, id));
    let mut proof = link_data(118);
    proof[0] = 15;
    proof[18] = WireContext::LinkRequestProof.to_byte();
    proof[2..18].copy_from_slice(id.unwrap().as_bytes());
    assert_eq!(identity(&proof), (LoRaTracePacket::Proof, id));
    routed[0] |= 0x80;
    assert_eq!(identity(&routed), (LoRaTracePacket::Other, None));
    let mut malformed = raw;
    malformed.push(0);
    assert_eq!(identity(&malformed), (LoRaTracePacket::Other, None));
}

#[test]
fn producer_and_sink_limits_are_independent_and_counters_keep_running() {
    clear();
    let trace = LoRaLinkTrace::new(record);
    for _ in 0..LORA_LINK_TRACE_LIMIT + 3 {
        trace.observe(event(
            &request(),
            LoRaLinkPhase::QueueActive,
            &crate::lora::link_trace::EventContext {
                time: crate::lora::link_trace::EventTime {
                    at_ms: Some(10),
                    enqueued_at: Some(1),
                },
                queue_bytes: 0,
                active: true,
            },
        ));
    }
    let snapshot = trace.snapshot();
    assert_eq!(snapshot.attempted, LORA_LINK_TRACE_LIMIT + 3);
    assert_eq!(snapshot.emitted, LORA_LINK_TRACE_LIMIT + 1);
    assert_eq!(snapshot.producer_dropped, 3);
    assert_eq!(snapshot.queue_accepted, LORA_LINK_TRACE_LIMIT + 3);
    assert_eq!(snapshot.sink_dropped, 0);
    assert_eq!(events().last().unwrap().phase, LoRaLinkPhase::LimitReached);
    let rejected = LoRaLinkTrace::new(|_| false);
    rejected.observe(event(
        &request(),
        LoRaLinkPhase::TxStarted,
        &crate::lora::link_trace::EventContext {
            time: crate::lora::link_trace::EventTime {
                at_ms: Some(10),
                enqueued_at: None,
            },
            queue_bytes: 0,
            active: true,
        },
    ));
    assert_eq!(rejected.snapshot().sink_dropped, 1);
    assert_eq!(rejected.snapshot().emitted, 0);
    assert_eq!(rejected.snapshot().tx_started, 1);
    // Non-Link background traffic counts, but consumes no per-Link event budget.
    rejected.observe(event(
        &[0; 19],
        LoRaLinkPhase::QueuePending,
        &crate::lora::link_trace::EventContext {
            time: crate::lora::link_trace::EventTime {
                at_ms: Some(10),
                enqueued_at: None,
            },
            queue_bytes: 0,
            active: true,
        },
    ));
    assert_eq!(rejected.snapshot().queue_accepted, 1);
    assert_eq!(rejected.snapshot().attempted, 1);
}

#[test]
fn queue_observer_preserves_fifo_and_original_clock_domain_and_age() {
    clear();
    let trace = LoRaLinkTrace::new(record);
    let mut storage = [0; 1024];
    let mut backlog = TransmitBacklog::new(&mut storage);
    backlog.link_trace = Some(&trace);
    let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
    let one = request();
    let two = link_data(40);
    assert_eq!(
        backlog.accept_frame(
            OutboundFrame {
                bytes: &one,
                enqueued_at: Some(InstantMillis(100))
            },
            &profile,
            5,
            150
        ),
        Ok(PacketPlacement::Active)
    );
    assert_eq!(
        backlog.accept_frame(
            OutboundFrame {
                bytes: &two,
                enqueued_at: Some(InstantMillis(110))
            },
            &profile,
            10,
            155
        ),
        Ok(PacketPlacement::Queued)
    );
    assert_eq!(events()[0].age_ms, Some(50));
    assert_eq!(events()[1].age_ms, Some(45));
    assert_eq!(events()[1].queue_bytes, 50);
    backlog.active.clear();
    assert!(backlog.activate_next_at(&profile, 999, 200));
    assert_eq!(&backlog.active.bytes[..two.len()], &two);
    assert_eq!(backlog.active.enqueued_at_ms, 110);
    assert_eq!(events()[2].age_ms, Some(90));
    assert_eq!(events()[2].at_ms, Some(200));
    assert_eq!(backlog.active.activated_at_ms, 999);
    backlog
        .accept_frame(
            OutboundFrame {
                bytes: &one,
                enqueued_at: Some(InstantMillis(120)),
            },
            &profile,
            1000,
            210,
        )
        .unwrap();
    assert_eq!(
        backlog.discard_expired_queue_head(120_120),
        Some(OutboundQueueDropReason::Expired)
    );
    let last = *events().last().unwrap();
    assert_eq!(
        last.phase,
        LoRaLinkPhase::QueueDropped(OutboundQueueDropReason::Expired)
    );
    assert_eq!(last.link_id, identity(&one).1);
    assert_eq!(last.age_ms, Some(120_000));
    assert!(backlog.queue.is_empty());
}

#[test]
fn pending_tx_is_not_success_and_second_frame_failure_keeps_partial_outcome() {
    clear();
    let trace = LoRaLinkTrace::new(record);
    let lab = Lab::new();
    let mut radio = Radio(lab.clone());
    let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
    let status = EmbassyInterfaceStatus::new_accounted(
        LoRaInterface::<Radio>::interface_id(&profile),
        ConnectionState::Connected,
    );
    let mut airtime = AirtimeLedger::new();
    let mut throughput = ThroughputLedger::new();
    let mut frame = [0; LORA_SINGLE_FRAME_MAX];
    let mut sequence = 0;
    let started = Instant::now();
    let packet = link_data(300);
    lab.pause_tx.set(true);
    block_on(async {
        radio.initialize_band(profile).await.unwrap();
        let (result, ()) = join(
            transmit_packet(
                &mut radio,
                &packet,
                &mut sequence,
                &mut airtime,
                &mut throughput,
                &status,
                &profile,
                &started,
                &EmbassyClock,
                &mut frame,
                Some(TxTrace {
                    trace: &trace,
                    enqueued_at: started.as_millis(),
                    queue_bytes: 300,
                }),
            ),
            async {
                lab.tx_started.wait().await;
                assert_eq!(
                    events().last().unwrap().phase,
                    LoRaLinkPhase::TxFrameStarted
                );
                assert!(!events().iter().any(|e| matches!(
                    e.phase,
                    LoRaLinkPhase::TxFrameComplete | LoRaLinkPhase::TxComplete
                )));
                lab.fault
                    .set(Some((Operation::Transmit, Fault::Recoverable)));
                lab.tx_release.signal(());
            },
        )
        .await;
        assert_eq!(result, Err(LoRaTransmitError::Radio(Fault::Recoverable)));
    });
    assert_eq!(
        events()
            .iter()
            .filter(|e| e.phase == LoRaLinkPhase::TxFrameComplete)
            .count(),
        1
    );
    assert_eq!(events().last().unwrap().phase, LoRaLinkPhase::TxRadioError);
    assert_eq!(events().last().unwrap().frame_index, 1);
    assert_eq!(events().last().unwrap().frame_count, 2);
    assert_eq!(trace.snapshot().tx_started, 1);
    assert_eq!(trace.snapshot().tx_failed, 1);
    assert_eq!(trace.snapshot().tx_complete, 0);
    assert_eq!(status.tx_bytes(), 255);
    assert_eq!(sequence, 0x10);
    clear();
    let packet = request();
    block_on(transmit_packet(
        &mut radio,
        &packet,
        &mut sequence,
        &mut airtime,
        &mut throughput,
        &status,
        &profile,
        &started,
        &EmbassyClock,
        &mut frame,
        Some(TxTrace {
            trace: &trace,
            enqueued_at: started.as_millis(),
            queue_bytes: 0,
        }),
    ))
    .unwrap();
    assert_eq!(events().last().unwrap().phase, LoRaLinkPhase::TxComplete);
    assert_eq!(trace.snapshot().tx_complete, 1);
    assert_eq!(&lab.transmissions.borrow().last().unwrap().1[1..], packet);
}

#[test]
fn split_receive_correlates_only_after_complete_and_reset_is_explicit() {
    clear();
    let trace = LoRaLinkTrace::new(record);
    let lab = Lab::new();
    let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
    let status = EmbassyInterfaceStatus::new_accounted(
        LoRaInterface::<Radio>::interface_id(&profile),
        ConnectionState::Connected,
    );
    let mut throughput = ThroughputLedger::new();
    let mut reassembler = LoRaReassembler::new();
    let mut seam = Seam {
        lab: lab.clone(),
        current: Vec::new(),
        sink: Vec::new(),
    };
    let packet = link_data(300);
    let mut frame = [0; LORA_SINGLE_FRAME_MAX];
    for index in 0..2 {
        let n = encode_air_frame_part(&packet, 0, index, &mut frame).unwrap();
        block_on(deliver_rx(
            ObservedAirFrame {
                bytes: &frame[..n],
                phy: PacketPhyStats::default(),
                spreading_factor: SpreadingFactor::Sf7,
                arrived_at: InstantMillis(index as u64),
            },
            &status,
            &mut throughput,
            &mut reassembler,
            &mut seam,
            Some(&trace),
            100 + index as u64,
        ));
        if index == 0 {
            assert_eq!(events().last().unwrap().phase, LoRaLinkPhase::RxAwaiting);
            assert_eq!(trace.snapshot().rx_packets, 0);
        }
    }
    assert_eq!(
        lab.delivered.borrow().as_slice(),
        core::slice::from_ref(&packet)
    );
    let observed = events();
    assert_eq!(observed[1].phase, LoRaLinkPhase::RxPacket);
    assert_eq!(observed[1].link_id, identity(&packet).1);
    assert_eq!(observed[2].phase, LoRaLinkPhase::RxDelivered);
    assert_eq!(observed[2].at_ms, None);
    assert_eq!(trace.snapshot().rx_frames, 2);
    assert_eq!(trace.snapshot().rx_packets, 1);
    let n = encode_air_frame_part(&packet, 0x10, 0, &mut frame).unwrap();
    reassembler.feed_with_phy(&frame[..n], PacketPhyStats::default());
    reset_reassembly_observed(&mut reassembler, &status, Some(&trace), 103);
    assert_eq!(events().last().unwrap().phase, LoRaLinkPhase::RxReset);
}

#[test]
fn observer_has_bounded_fixed_storage() {
    assert!(size_of::<LoRaLinkTrace>() <= 64);
    assert!(size_of::<LoRaLinkEvent>() <= 80);
    assert!(size_of::<TxTrace<'_>>() <= 24);
    println!(
        "trace_bytes={} event_bytes={} tx_context_bytes={}",
        size_of::<LoRaLinkTrace>(),
        size_of::<LoRaLinkEvent>(),
        size_of::<TxTrace<'_>>()
    );
}
