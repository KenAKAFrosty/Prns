use super::*;

fn config(
    endpoints: usize,
    receive_queue: usize,
    pending: usize,
    trace: usize,
    faults: FaultPlan,
) -> VirtualMediumConfig {
    match VirtualMediumConfig::new(endpoints, receive_queue, pending, trace, faults) {
        Ok(config) => config,
        Err(error) => unreachable!("test constants are valid: {error}"),
    }
}

fn attached_pair(medium: &VirtualMedium) -> (VirtualInterface, VirtualInterface) {
    let first = match medium.attach(b"first") {
        Ok(interface) => interface,
        Err(error) => unreachable!("first endpoint attaches: {error}"),
    };
    let second = match medium.attach(b"second") {
        Ok(interface) => interface,
        Err(error) => unreachable!("second endpoint attaches: {error}"),
    };
    (first, second)
}

#[test]
fn configuration_rejects_each_zero_capacity_as_a_whole_value() {
    for (capacities, field) in [
        ((0, 1, 1, 1), CapacityField::Endpoints),
        ((1, 0, 1, 1), CapacityField::EndpointReceiveQueue),
        ((1, 1, 0, 1), CapacityField::PendingDeliveries),
        ((1, 1, 1, 0), CapacityField::Trace),
    ] {
        assert_eq!(
            VirtualMediumConfig::new(
                capacities.0,
                capacities.1,
                capacities.2,
                capacities.3,
                FaultPlan::none(),
            ),
            Err(VirtualMediumConfigError::ZeroCapacity(field)),
        );
    }
}

#[test]
fn fault_plan_rejects_noncanonical_rules_as_whole_values() {
    for invalid in [
        vec![TransmissionOrdinal::new(2), TransmissionOrdinal::new(2)],
        vec![TransmissionOrdinal::new(2), TransmissionOrdinal::new(1)],
    ] {
        assert!(FaultPlan::drop_transmissions(invalid).is_err());
    }
    let duplicate = TransmissionRule::duplicate(
        TransmissionOrdinal::new(0),
        SimulationDurationInTicks::from_ticks(2),
        SimulationDurationInTicks::from_ticks(1),
    );
    assert!(matches!(
        FaultPlan::new(vec![duplicate]),
        Err(FaultPlanError::DuplicateDeliveriesNotCanonical { .. }),
    ));
}

#[test]
fn scheduled_drop_is_deterministic_and_visible() {
    let faults = match FaultPlan::drop_transmissions(vec![TransmissionOrdinal::new(0)]) {
        Ok(faults) => faults,
        Err(error) => unreachable!("test plan is canonical: {error}"),
    };
    let medium = VirtualMedium::new(config(2, 1, 2, 8, faults));
    let (first, second) = attached_pair(&medium);

    assert_eq!(medium.transmit(first.endpoint_id(), vec![1, 2, 3]), Ok(()));
    let trace = medium.trace();
    assert!(trace.events.iter().any(|event| matches!(
        event,
        MediumEvent::ReceptionDropped {
            ordinal,
            to,
            reason: ReceptionDropReason::ScheduledFault,
            ..
        } if *ordinal == TransmissionOrdinal::new(0) && *to == second.endpoint_id()
    )));
    assert!(!trace.events.iter().any(|event| matches!(
        event,
        MediumEvent::ReceptionQueued { ordinal, to, .. }
            if *ordinal == TransmissionOrdinal::new(0) && *to == second.endpoint_id()
    )));
}

#[test]
fn logical_time_makes_duplication_and_reordering_exact() {
    let faults = match FaultPlan::new(vec![
        TransmissionRule::delay(
            TransmissionOrdinal::new(0),
            SimulationDurationInTicks::from_ticks(2),
        ),
        TransmissionRule::duplicate(
            TransmissionOrdinal::new(1),
            SimulationDurationInTicks::ZERO,
            SimulationDurationInTicks::from_ticks(1),
        ),
    ]) {
        Ok(faults) => faults,
        Err(error) => unreachable!("test plan is canonical: {error}"),
    };
    let medium = VirtualMedium::new(config(2, 8, 4, 32, faults));
    let (first, _second) = attached_pair(&medium);

    assert_eq!(medium.transmit(first.endpoint_id(), vec![0]), Ok(()));
    assert_eq!(medium.transmit(first.endpoint_id(), vec![1]), Ok(()));
    assert_eq!(medium.pending_delivery_count(), 2);

    let first_advance = match medium.advance_by(SimulationDurationInTicks::from_ticks(1)) {
        Ok(report) => report,
        Err(error) => unreachable!("one tick is representable: {error}"),
    };
    assert_eq!(first_advance.receptions_queued, 1);
    assert_eq!(first_advance.settled(), 1);
    assert_eq!(medium.pending_delivery_count(), 1);

    let second_advance = match medium.advance_to(SimulationTick::from_ticks(2)) {
        Ok(report) => report,
        Err(error) => unreachable!("time moves forward: {error}"),
    };
    assert_eq!(second_advance.receptions_queued, 1);
    assert_eq!(medium.pending_delivery_count(), 0);

    let queued: Vec<_> = medium
        .trace()
        .events
        .into_iter()
        .filter_map(|event| match event {
            MediumEvent::ReceptionQueued {
                ordinal, copy, at, ..
            } => Some((ordinal, copy, at)),
            _ => None,
        })
        .collect();
    assert_eq!(
        queued,
        vec![
            (
                TransmissionOrdinal::new(1),
                DeliveryCopy::Original,
                SimulationTick::ZERO,
            ),
            (
                TransmissionOrdinal::new(1),
                DeliveryCopy::Duplicate,
                SimulationTick::from_ticks(1),
            ),
            (
                TransmissionOrdinal::new(0),
                DeliveryCopy::Original,
                SimulationTick::from_ticks(2),
            ),
        ],
    );
}

#[test]
fn pending_capacity_rejects_a_delayed_fanout_as_a_unit() {
    let faults = match FaultPlan::new(vec![TransmissionRule::delay(
        TransmissionOrdinal::new(0),
        SimulationDurationInTicks::from_ticks(1),
    )]) {
        Ok(faults) => faults,
        Err(error) => unreachable!("test plan is canonical: {error}"),
    };
    let medium = VirtualMedium::new(config(3, 1, 1, 16, faults));
    let first = match medium.attach(b"first") {
        Ok(interface) => interface,
        Err(error) => unreachable!("first endpoint attaches: {error}"),
    };
    let mut recipients = Vec::new();
    for tag in [b"second".as_slice(), b"third".as_slice()] {
        match medium.attach(tag) {
            Ok(interface) => recipients.push(interface),
            Err(error) => unreachable!("recipient attaches: {error}"),
        }
    }

    assert_eq!(medium.transmit(first.endpoint_id(), vec![7]), Ok(()));
    assert_eq!(medium.pending_delivery_count(), 0);
    let capacity_drops = medium
        .trace()
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                MediumEvent::ReceptionDropped {
                    reason: ReceptionDropReason::PendingCapacityReached,
                    at,
                    intended_for,
                    ..
                } if *at == SimulationTick::ZERO
                    && *intended_for == SimulationTick::from_ticks(1)
            )
        })
        .count();
    assert_eq!(capacity_drops, 2);
    assert_eq!(recipients.len(), 2);
}

#[test]
fn time_refuses_to_move_backward() {
    let medium = VirtualMedium::new(config(1, 1, 1, 4, FaultPlan::none()));
    let advanced = medium.advance_to(SimulationTick::from_ticks(2));
    assert!(advanced.is_ok());
    assert_eq!(
        medium.advance_to(SimulationTick::from_ticks(1)),
        Err(AdvanceError::BeforeCurrent {
            current: SimulationTick::from_ticks(2),
            requested: SimulationTick::from_ticks(1),
        }),
    );
    assert_eq!(medium.now(), SimulationTick::from_ticks(2));

    let at_end = medium.advance_to(SimulationTick::from_ticks(u64::MAX));
    assert!(at_end.is_ok());
    assert_eq!(
        medium.advance_by(SimulationDurationInTicks::from_ticks(1)),
        Err(AdvanceError::Overflow {
            current: SimulationTick::from_ticks(u64::MAX),
            by: SimulationDurationInTicks::from_ticks(1),
        }),
    );
}

#[test]
fn delayed_delivery_to_a_detached_endpoint_settles_as_a_typed_drop() {
    let faults = match FaultPlan::new(vec![TransmissionRule::delay(
        TransmissionOrdinal::new(0),
        SimulationDurationInTicks::from_ticks(1),
    )]) {
        Ok(faults) => faults,
        Err(error) => unreachable!("test plan is canonical: {error}"),
    };
    let medium = VirtualMedium::new(config(2, 1, 1, 16, faults));
    let (first, second) = attached_pair(&medium);
    let second_id = second.endpoint_id();
    assert_eq!(medium.transmit(first.endpoint_id(), vec![4]), Ok(()));
    drop(second);

    let report = match medium.advance_by(SimulationDurationInTicks::from_ticks(1)) {
        Ok(report) => report,
        Err(error) => unreachable!("one tick is representable: {error}"),
    };
    assert_eq!(report.receptions_dropped, 1);
    assert_eq!(report.settled(), 1);
    assert_eq!(medium.pending_delivery_count(), 0);
    assert!(medium.trace().events.iter().any(|event| matches!(
        event,
        MediumEvent::ReceptionDropped {
            to,
            at,
            intended_for,
            reason: ReceptionDropReason::EndpointClosed,
            ..
        } if *to == second_id
            && *at == SimulationTick::from_ticks(1)
            && *intended_for == SimulationTick::from_ticks(1)
    )));
}

#[test]
fn bounded_trace_counts_evicted_events() {
    let medium = VirtualMedium::new(config(2, 1, 1, 2, FaultPlan::none()));
    let (first, _second) = attached_pair(&medium);
    assert_eq!(medium.transmit(first.endpoint_id(), vec![9]), Ok(()));

    let trace = medium.trace();
    assert_eq!(trace.events.len(), 2);
    assert_eq!(trace.discarded_events, 2);
}
