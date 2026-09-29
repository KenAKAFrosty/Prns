use crate::*;

fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

fn medium(rules: Vec<TransmissionRule>, receive_capacity: usize) -> VirtualMedium {
    let faults =
        FaultPlan::new(rules).unwrap_or_else(|error| unreachable!("canonical test plan: {error}"));
    VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::FullyConnected,
            2,
            receive_capacity,
            8,
            64,
            faults,
        )
        .unwrap_or_else(|error| unreachable!("valid test capacities: {error}")),
    )
}

fn pair(medium: &VirtualMedium) -> (VirtualInterface, VirtualInterface) {
    (
        medium
            .attach(b"sender")
            .unwrap_or_else(|error| unreachable!("sender attaches: {error}")),
        medium
            .attach(b"receiver")
            .unwrap_or_else(|error| unreachable!("receiver attaches: {error}")),
    )
}

fn delay(ordinal: u64, by: u64) -> TransmissionRule {
    TransmissionRule::delay(
        TransmissionOrdinal::new(ordinal),
        SimulationDurationInTicks::from_ticks(by),
    )
}

#[test]
fn frame_steps_stop_at_each_due_tick_and_accept_reactions_before_later_events() {
    let medium = medium(vec![delay(0, 7), delay(1, 3), delay(2, 3), delay(3, 1)], 8);
    let (sender, _receiver) = pair(&medium);
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(0),
            next_event_at: None
        }
    );
    for byte in 0..3 {
        assert_eq!(medium.transmit(sender.endpoint_id(), vec![byte]), Ok(()));
    }
    let before = medium.trace();
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(0),
            next_event_at: Some(tick(3))
        }
    );
    assert_eq!(medium.trace(), before);
    assert_eq!(
        medium.advance_to_next_event(tick(2)),
        Ok(AdvanceReport {
            from: tick(0),
            to: tick(2),
            receptions_queued: 0,
            receptions_dropped: 0,
        })
    );
    assert_eq!(
        medium.advance_to_next_event(tick(100)),
        Ok(AdvanceReport {
            from: tick(2),
            to: tick(3),
            receptions_queued: 2,
            receptions_dropped: 0,
        })
    );
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(3),
            next_event_at: Some(tick(7))
        }
    );

    // A reaction can insert an earlier event after the snapshot; stepping must reconsider it.
    assert_eq!(medium.transmit(sender.endpoint_id(), vec![3]), Ok(()));
    assert_eq!(
        medium.advance_to_next_event(tick(100)),
        Ok(AdvanceReport {
            from: tick(3),
            to: tick(4),
            receptions_queued: 1,
            receptions_dropped: 0,
        })
    );
    assert_eq!(
        medium.advance_to_next_event(tick(100)),
        Ok(AdvanceReport {
            from: tick(4),
            to: tick(7),
            receptions_queued: 1,
            receptions_dropped: 0,
        })
    );
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(7),
            next_event_at: None
        }
    );
    assert_eq!(
        medium.advance_to_next_event(tick(100)),
        Ok(AdvanceReport {
            from: tick(7),
            to: tick(100),
            receptions_queued: 0,
            receptions_dropped: 0,
        })
    );

    let delivered: Vec<_> = medium
        .trace()
        .events
        .into_iter()
        .filter_map(|event| match event {
            MediumEvent::ReceptionQueued { ordinal, at, .. } => Some((ordinal, at)),
            _ => None,
        })
        .collect();
    assert_eq!(
        delivered,
        vec![
            (TransmissionOrdinal::new(1), tick(3)),
            (TransmissionOrdinal::new(2), tick(3)),
            (TransmissionOrdinal::new(3), tick(4)),
            (TransmissionOrdinal::new(0), tick(7)),
        ]
    );
}

#[test]
fn frame_stepping_preserves_bulk_delivery_order_without_intervening_reactions() {
    let rules = vec![
        TransmissionRule::duplicate(
            TransmissionOrdinal::new(0),
            SimulationDurationInTicks::from_ticks(2),
            SimulationDurationInTicks::from_ticks(5),
        ),
        delay(1, 2),
    ];
    let stepped = medium(rules.clone(), 8);
    let bulk = medium(rules, 8);
    let (stepped_sender, _stepped_receiver) = pair(&stepped);
    let (bulk_sender, _bulk_receiver) = pair(&bulk);
    for (medium, sender) in [(&stepped, stepped_sender), (&bulk, bulk_sender)] {
        for byte in [4, 9] {
            assert_eq!(medium.transmit(sender.endpoint_id(), vec![byte]), Ok(()));
        }
    }
    assert!(bulk.advance_to(tick(9)).is_ok());
    for expected in [2, 5, 9] {
        assert_eq!(
            stepped
                .advance_to_next_event(tick(9))
                .map(|report| report.to),
            Ok(tick(expected))
        );
    }
    let without_clock = |trace: TraceSnapshot| {
        trace
            .events
            .into_iter()
            .filter(|event| !matches!(event, MediumEvent::TimeAdvanced { .. }))
            .collect::<Vec<_>>()
    };
    assert_eq!(without_clock(stepped.trace()), without_clock(bulk.trace()));
    assert_eq!(stepped.schedule(), bulk.schedule());
}

#[test]
fn frame_drops_still_settle_at_their_deadline_and_do_not_hide_later_work() {
    let medium = medium(vec![delay(0, 3), delay(1, 3), delay(2, 5)], 1);
    let (sender, receiver) = pair(&medium);
    for byte in 0..3 {
        assert_eq!(medium.transmit(sender.endpoint_id(), vec![byte]), Ok(()));
    }
    assert_eq!(
        medium.advance_to_next_event(tick(10)),
        Ok(AdvanceReport {
            from: tick(0),
            to: tick(3),
            receptions_queued: 1,
            receptions_dropped: 1,
        })
    );
    let receiver_id = receiver.endpoint_id();
    drop(receiver);
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(3),
            next_event_at: Some(tick(5))
        }
    );
    assert_eq!(
        medium.advance_to_next_event(tick(10)),
        Ok(AdvanceReport {
            from: tick(3),
            to: tick(5),
            receptions_queued: 0,
            receptions_dropped: 1,
        })
    );
    let drops: Vec<_> = medium
        .trace()
        .events
        .into_iter()
        .filter(|event| matches!(event, MediumEvent::ReceptionDropped { .. }))
        .collect();
    assert_eq!(
        drops,
        vec![
            MediumEvent::ReceptionDropped {
                ordinal: TransmissionOrdinal::new(1),
                to: receiver_id,
                copy: DeliveryCopy::Original,
                at: tick(3),
                intended_for: tick(3),
                reason: ReceptionDropReason::ReceiveQueueFull,
            },
            MediumEvent::ReceptionDropped {
                ordinal: TransmissionOrdinal::new(2),
                to: receiver_id,
                copy: DeliveryCopy::Original,
                at: tick(5),
                intended_for: tick(5),
                reason: ReceptionDropReason::EndpointClosed,
            },
        ]
    );
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(5),
            next_event_at: None
        }
    );
}

#[test]
fn frame_step_refuses_backward_time_and_settles_the_last_representable_deadline() {
    let medium = medium(vec![delay(0, u64::MAX)], 1);
    let (sender, _receiver) = pair(&medium);
    assert_eq!(medium.transmit(sender.endpoint_id(), vec![1]), Ok(()));
    assert!(medium.advance_to_next_event(tick(1)).is_ok());
    let before = (
        medium.schedule(),
        medium.trace(),
        medium.pending_delivery_count(),
    );
    assert_eq!(
        medium.advance_to_next_event(tick(0)),
        Err(AdvanceError::BeforeCurrent {
            current: tick(1),
            requested: tick(0),
        })
    );
    assert_eq!(
        (
            medium.schedule(),
            medium.trace(),
            medium.pending_delivery_count()
        ),
        before
    );
    assert_eq!(
        medium.advance_to_next_event(tick(u64::MAX)),
        Ok(AdvanceReport {
            from: tick(1),
            to: tick(u64::MAX),
            receptions_queued: 1,
            receptions_dropped: 0,
        })
    );
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(u64::MAX),
            next_event_at: None
        }
    );
    assert_eq!(
        medium.advance_to_next_event(tick(u64::MAX)),
        Ok(AdvanceReport {
            from: tick(u64::MAX),
            to: tick(u64::MAX),
            receptions_queued: 0,
            receptions_dropped: 0,
        })
    );
}
