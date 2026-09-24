use super::*;

fn config(trace_capacity: usize, faults: FaultPlan) -> VirtualMediumConfig {
    match VirtualMediumConfig::new(2, 1, trace_capacity, faults) {
        Ok(config) => config,
        Err(error) => unreachable!("test constants are valid: {error}"),
    }
}

#[test]
fn configuration_rejects_each_zero_capacity_as_a_whole_value() {
    for (capacities, field) in [
        ((0, 1, 1), CapacityField::Endpoints),
        ((1, 0, 1), CapacityField::EndpointReceiveQueue),
        ((1, 1, 0), CapacityField::Trace),
    ] {
        assert_eq!(
            VirtualMediumConfig::new(capacities.0, capacities.1, capacities.2, FaultPlan::none(),),
            Err(VirtualMediumConfigError::ZeroCapacity(field)),
        );
    }
}

#[test]
fn fault_plan_rejects_duplicate_and_unsorted_ordinals() {
    for invalid in [
        vec![TransmissionOrdinal::new(2), TransmissionOrdinal::new(2)],
        vec![TransmissionOrdinal::new(2), TransmissionOrdinal::new(1)],
    ] {
        assert!(FaultPlan::drop_transmissions(invalid).is_err());
    }
}

#[test]
fn scheduled_drop_is_deterministic_and_visible() {
    let faults = match FaultPlan::drop_transmissions(vec![TransmissionOrdinal::new(0)]) {
        Ok(faults) => faults,
        Err(error) => unreachable!("test plan is canonical: {error}"),
    };
    let medium = VirtualMedium::new(config(8, faults));
    let first = match medium.attach(b"first") {
        Ok(interface) => interface,
        Err(error) => unreachable!("first endpoint attaches: {error}"),
    };
    let second = match medium.attach(b"second") {
        Ok(interface) => interface,
        Err(error) => unreachable!("second endpoint attaches: {error}"),
    };

    assert_eq!(medium.transmit(first.endpoint_id(), vec![1, 2, 3]), Ok(()));
    let trace = medium.trace();
    assert!(trace.events.iter().any(|event| matches!(
        event,
        MediumEvent::ReceptionDropped {
            ordinal,
            to,
            reason: ReceptionDropReason::ScheduledFault,
        } if *ordinal == TransmissionOrdinal::new(0) && *to == second.endpoint_id()
    )));
    assert!(!trace.events.iter().any(|event| matches!(
        event,
        MediumEvent::ReceptionQueued { ordinal, to }
            if *ordinal == TransmissionOrdinal::new(0) && *to == second.endpoint_id()
    )));
}

#[test]
fn bounded_trace_counts_evicted_events() {
    let medium = VirtualMedium::new(config(2, FaultPlan::none()));
    let first = match medium.attach(b"first") {
        Ok(interface) => interface,
        Err(error) => unreachable!("first endpoint attaches: {error}"),
    };
    let _second = match medium.attach(b"second") {
        Ok(interface) => interface,
        Err(error) => unreachable!("second endpoint attaches: {error}"),
    };
    assert_eq!(medium.transmit(first.endpoint_id(), vec![9]), Ok(()));

    let trace = medium.trace();
    assert_eq!(trace.events.len(), 2);
    assert_eq!(trace.discarded_events, 2);
}
