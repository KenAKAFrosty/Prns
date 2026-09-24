use super::*;
use crate::{SimulationDurationInTicks, TransmissionAction};
use personal_rns::interfaces::MacAddress;
use personal_rns::wifi_halow::HaLowDatagrams;

fn mac(value: u8) -> PeerMac {
    PeerMac::new(MacAddress::new([2, 0, 0, 0, 0, value])).expect("unicast MAC")
}
fn medium(receive: usize, pending: usize) -> VirtualHaLowMedium {
    let nz = |n| NonZeroUsize::new(n).expect("positive budget");
    VirtualHaLowMedium::new(HaLowMediumLimits {
        radios: nz(3),
        receive_datagrams: nz(receive),
        pending_deliveries: nz(pending),
        trace_events: nz(128),
        armed_faults: nz(4),
    })
}
fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}
fn delay(value: u64) -> TransmissionAction {
    TransmissionAction::Delay {
        by: SimulationDurationInTicks::from_ticks(value),
    }
}

#[tokio::test]
async fn directed_paths_broadcast_once_and_preserve_source_mac() {
    let medium = medium(4, 4);
    let a = medium.attach(mac(1)).expect("a");
    let b = medium.attach(mac(2)).expect("b");
    let c = medium.attach(mac(3)).expect("c");
    medium.set_path(a.id(), b.id(), PathState::Reachable);
    medium.set_path(a.id(), c.id(), PathState::Reachable);
    a.send(Destination::Broadcast, b"group")
        .await
        .expect("send");
    a.send(Destination::Peer(mac(2)), b"direct")
        .await
        .expect("send");
    b.send(Destination::Peer(mac(1)), b"no reverse path")
        .await
        .expect("acceptance is not delivery");
    let mut bytes = [0; 16];
    for radio in [&b, &c] {
        let received = radio.receive(&mut bytes).await.expect("group receive");
        assert_eq!(
            (received.source, &bytes[..received.length]),
            (mac(1), b"group".as_slice())
        );
    }
    let received = b.receive(&mut bytes).await.expect("direct receive");
    assert_eq!(&bytes[..received.length], b"direct");
    assert_eq!(medium.snapshot().queued, 0);
    assert_eq!(
        medium
            .snapshot()
            .events
            .iter()
            .filter(|e| matches!(
                e,
                HaLowEvent::Transmitted {
                    destination: Destination::Broadcast,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn broadcast_reception_does_not_prove_a_usable_unicast_path() {
    let medium = medium(4, 4);
    let a = medium.attach(mac(1)).expect("a");
    let b = medium.attach(mac(2)).expect("b");
    medium.set_path(a.id(), b.id(), PathState::BroadcastOnly);
    a.send(Destination::Broadcast, b"group")
        .await
        .expect("accepted");
    a.send(Destination::Peer(mac(2)), b"unicast without a kernel path")
        .await
        .expect("acceptance is not delivery");
    let mut bytes = [0; 64];
    let received = b.receive(&mut bytes).await.expect("group");
    assert_eq!(
        (received.source, &bytes[..received.length]),
        (mac(1), b"group".as_slice())
    );
    assert_eq!(medium.snapshot().queued, 0);
    medium.set_path(a.id(), b.id(), PathState::Reachable);
    a.send(Destination::Peer(mac(2)), b"path ready")
        .await
        .expect("accepted");
    let received = b.receive(&mut bytes).await.expect("unicast");
    assert_eq!(&bytes[..received.length], b"path ready");
}

#[tokio::test]
async fn delayed_duplicate_reorders_and_cannot_land_on_reused_mac() {
    let medium = medium(4, 4);
    let a = medium.attach(mac(1)).expect("a");
    let b = medium.attach(mac(2)).expect("b");
    medium.set_path(a.id(), b.id(), PathState::Reachable);
    medium
        .arm_next(NextDatagramFault {
            from: a.id(),
            destination: FaultDestination::Any,
            action: TransmissionAction::Duplicate {
                first_after: SimulationDurationInTicks::from_ticks(10),
                second_after: SimulationDurationInTicks::from_ticks(20),
            },
        })
        .expect("fault");
    a.send(Destination::Peer(mac(2)), b"old")
        .await
        .expect("send");
    a.send(Destination::Peer(mac(2)), b"overtake")
        .await
        .expect("send");
    let mut bytes = [0; 16];
    assert_eq!(b.receive(&mut bytes).await.expect("receive").length, 8);
    assert_eq!(&bytes[..8], b"overtake");
    medium.advance_to_next_event(tick(10)).expect("time");
    assert_eq!(b.receive(&mut bytes).await.expect("receive").length, 3);
    let old = b.id();
    drop(b);
    let replacement = medium.attach(mac(2)).expect("new generation");
    assert_ne!(old, replacement.id());
    medium.advance_to_next_event(tick(20)).expect("time");
    assert_eq!(medium.snapshot().queued, 0);
    assert!(
        matches!(medium.snapshot().events.last(), Some(HaLowEvent::Delivery { to, outcome: DeliveryOutcome::Detached, .. }) if *to == old)
    );
}

#[tokio::test]
async fn medium_capacity_loss_is_explicit_and_reusable() {
    let medium = medium(1, 1);
    let a = medium.attach(mac(1)).expect("a");
    let b = medium.attach(mac(2)).expect("b");
    medium.set_path(a.id(), b.id(), PathState::Reachable);
    for _ in 0..2 {
        a.send(Destination::Broadcast, b"queue")
            .await
            .expect("send");
    }
    for _ in 0..2 {
        medium
            .arm_next(NextDatagramFault {
                from: a.id(),
                destination: FaultDestination::Any,
                action: delay(5),
            })
            .expect("fault");
        a.send(Destination::Broadcast, b"pending")
            .await
            .expect("send");
    }
    let snapshot = medium.snapshot();
    assert_eq!(
        (
            snapshot.queued,
            snapshot.pending,
            snapshot.peak_queued,
            snapshot.peak_pending
        ),
        (1, 1, 1, 1)
    );
    for reason in [
        DeliveryOutcome::ReceiveQueueFull,
        DeliveryOutcome::PendingCapacity,
    ] {
        assert!(snapshot
            .events
            .iter()
            .any(|e| matches!(e, HaLowEvent::Delivery { outcome, .. } if *outcome == reason)));
    }
    b.receive(&mut [0; 16]).await.expect("drain");
    medium.advance_to_next_event(tick(5)).expect("time");
    b.receive(&mut [0; 16]).await.expect("drain delayed");
    a.send(Destination::Broadcast, b"fresh")
        .await
        .expect("capacity reusable");
    b.receive(&mut [0; 16]).await.expect("fresh receive");
    assert_eq!(
        (medium.snapshot().queued, medium.snapshot().pending),
        (0, 0)
    );
}

#[tokio::test(start_paused = true)]
async fn canceled_stalled_send_is_not_transmitted_later_and_receive_never_truncates() {
    let medium = medium(4, 4);
    let a = medium.attach(mac(1)).expect("a");
    let b = medium.attach(mac(2)).expect("b");
    medium.set_path(a.id(), b.id(), PathState::Reachable);
    medium.set_send_behavior(a.id(), SendBehavior::Stalled);
    assert!(tokio::time::timeout(
        std::time::Duration::from_secs(2),
        a.send(Destination::Broadcast, b"canceled")
    )
    .await
    .is_err());
    medium.set_send_behavior(a.id(), SendBehavior::Ready);
    assert_eq!(medium.snapshot().queued, 0);
    a.send(Destination::Broadcast, b"complete")
        .await
        .expect("send");
    assert_eq!(
        b.receive(&mut [0; 2])
            .await
            .err()
            .expect("refuse prefix")
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    medium.set_receive_behavior(b.id(), ReceiveBehavior::Failed);
    assert_eq!(
        b.receive(&mut [0; 16])
            .await
            .err()
            .expect("receive failure")
            .kind(),
        std::io::ErrorKind::BrokenPipe
    );
    medium.set_send_behavior(a.id(), SendBehavior::Failed);
    assert_eq!(
        a.send(Destination::Broadcast, b"fail")
            .await
            .expect_err("send failure")
            .kind(),
        std::io::ErrorKind::BrokenPipe
    );
}

#[test]
fn manual_clock_coordinates_halow_deliveries_and_refuses_backward_time() {
    let medium = medium(4, 4);
    assert_eq!(
        medium.advance_to_next_event(tick(7)).expect("time").to,
        tick(7)
    );
    let before = medium.snapshot();
    assert!(medium.advance_to_next_event(tick(6)).is_err());
    assert_eq!(before, medium.snapshot());
    #[cfg(feature = "controlled-time")]
    {
        let mut clock = crate::ManualTimeDriver::new(
            crate::ManualMedium::HaLow(medium.clone()),
            std::time::Duration::from_millis(1),
        )
        .expect("clock");
        assert!(
            matches!(clock.advance_to_next_event(tick(12)).expect("coordinated time"), crate::ManualAdvance::HaLow(crate::AdvanceReport { from, to, .. }) if from == tick(7) && to == tick(12))
        );
        assert_eq!(clock.snapshot().expect("snapshot").tick, tick(12));
    }
}

#[test]
fn attachment_and_fault_admission_are_bounded_and_retired_faults_release_capacity() {
    let medium = medium(4, 4);
    let a = medium.attach(mac(1)).expect("a");
    assert!(matches!(
        medium.attach(mac(1)),
        Err(AttachError::AddressInUse)
    ));
    let _b = medium.attach(mac(2)).expect("b");
    let _c = medium.attach(mac(3)).expect("c");
    assert!(matches!(medium.attach(mac(4)), Err(AttachError::Capacity)));
    assert_eq!(
        medium.arm_next(NextDatagramFault {
            from: a.id(),
            destination: FaultDestination::Any,
            action: TransmissionAction::Duplicate {
                first_after: SimulationDurationInTicks::from_ticks(2),
                second_after: SimulationDurationInTicks::from_ticks(1)
            }
        }),
        Err(ArmFaultError::NoncanonicalDuplicate)
    );
    let fault = NextDatagramFault {
        from: a.id(),
        destination: FaultDestination::Any,
        action: TransmissionAction::Drop,
    };
    for _ in 0..4 {
        medium.arm_next(fault).expect("bounded fault");
    }
    assert_eq!(medium.arm_next(fault), Err(ArmFaultError::Capacity));
    drop(a);
    assert_eq!(medium.snapshot().armed_faults, 0);
    assert_eq!(medium.arm_next(fault), Err(ArmFaultError::Detached));
}

#[test]
fn trace_budget_exhaustion_cannot_silently_discard_evidence() {
    let medium = VirtualHaLowMedium::new(HaLowMediumLimits {
        radios: NonZeroUsize::MIN,
        receive_datagrams: NonZeroUsize::MIN,
        pending_deliveries: NonZeroUsize::MIN,
        trace_events: NonZeroUsize::MIN,
        armed_faults: NonZeroUsize::MIN,
    });
    let radio = medium.attach(mac(1)).expect("first trace event");
    medium.set_send_behavior(radio.id(), SendBehavior::Failed);
    let snapshot = medium.snapshot();
    assert_eq!(snapshot.events.len(), 1);
    assert_eq!(
        snapshot.retention,
        TraceRetention::Exhausted { omitted_events: 1 }
    );
    drop(radio);
    assert_eq!(
        medium.snapshot().retention,
        TraceRetention::Exhausted { omitted_events: 2 }
    );
}
