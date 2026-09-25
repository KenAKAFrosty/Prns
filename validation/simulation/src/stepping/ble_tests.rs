use crate::ble::*;
use crate::{MediumSchedule, SimulationDurationInTicks, SimulationTick, TopologyConfig};

fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

fn parameters(interval: u64) -> BleAdvertisingParameters {
    BleAdvertisingParameters::new(
        BleAdvertisement::reticulum(BleRoleCapabilities::DualRole)
            .unwrap_or_else(|error| unreachable!("valid advertisement: {error}")),
        SimulationDurationInTicks::from_ticks(interval),
    )
    .unwrap_or_else(|error| unreachable!("nonzero interval: {error}"))
}

fn medium(emissions: usize) -> VirtualBleMedium {
    VirtualBleMedium::new(
        BleMediumConfig::new(TopologyConfig::FullyConnected, 3, 8, emissions, 128)
            .unwrap_or_else(|error| unreachable!("valid test capacities: {error}")),
    )
}

fn attach(medium: &VirtualBleMedium, address: u8) -> BleRadioId {
    medium
        .attach(BleAddress::new([address; 6]), -40)
        .unwrap_or_else(|error| unreachable!("unique address: {error}"))
}

#[test]
fn ble_schedule_tracks_radio_power_advertising_reconfiguration_and_detach() {
    let medium = medium(4);
    let advertiser = attach(&medium, 1);
    assert!(medium
        .set_advertising(advertiser, Some(parameters(5)))
        .is_ok());
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(0),
            next_event_at: None
        }
    );
    assert!(medium
        .set_radio_power(advertiser, BleRadioPower::On)
        .is_ok());
    let before = medium.trace();
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(0),
            next_event_at: Some(tick(0))
        }
    );
    assert_eq!(medium.trace(), before);
    assert_eq!(
        medium.advance_to_next_event(tick(100)),
        Ok(BleAdvanceReport {
            from: tick(0),
            to: tick(0),
            advertisements_emitted: 1,
            observations_queued: 0,
            observations_dropped: 0,
        })
    );
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(0),
            next_event_at: Some(tick(5))
        }
    );
    assert!(medium.advance_to_next_event(tick(3)).is_ok());
    assert!(medium.set_scanning(advertiser, BleScanState::On).is_ok());
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(3),
            next_event_at: Some(tick(5))
        }
    );

    assert_eq!(
        medium.set_advertising(advertiser, Some(parameters(5))),
        Ok(BleRadioMutation::Unchanged)
    );
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(3),
            next_event_at: Some(tick(5))
        }
    );
    assert!(medium
        .set_advertising(advertiser, Some(parameters(11)))
        .is_ok());
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(3),
            next_event_at: Some(tick(3))
        }
    );
    assert!(medium.advance_to_next_event(tick(100)).is_ok());
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(3),
            next_event_at: Some(tick(14))
        }
    );

    assert!(medium
        .set_radio_power(advertiser, BleRadioPower::Off)
        .is_ok());
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(3),
            next_event_at: None
        }
    );
    assert!(medium.advance_to_next_event(tick(20)).is_ok());
    assert!(medium
        .set_radio_power(advertiser, BleRadioPower::On)
        .is_ok());
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(20),
            next_event_at: Some(tick(20))
        }
    );
    assert!(medium.set_advertising(advertiser, None).is_ok());
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(20),
            next_event_at: None
        }
    );
    assert!(medium
        .set_advertising(advertiser, Some(parameters(5)))
        .is_ok());
    medium.detach(advertiser);
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(20),
            next_event_at: None
        }
    );
}

#[test]
fn ble_steps_settle_ties_in_radio_order_and_allow_reactions_between_ticks() {
    let medium = medium(4);
    let first = attach(&medium, 1);
    let second = attach(&medium, 2);
    let scanner = attach(&medium, 3);
    for radio in [first, second, scanner] {
        assert!(medium.set_radio_power(radio, BleRadioPower::On).is_ok());
    }
    assert!(medium.set_scanning(scanner, BleScanState::On).is_ok());
    for radio in [second, first] {
        assert!(medium.set_advertising(radio, Some(parameters(5))).is_ok());
    }
    assert_eq!(
        medium.advance_to_next_event(tick(100)),
        Ok(BleAdvanceReport {
            from: tick(0),
            to: tick(0),
            advertisements_emitted: 2,
            observations_queued: 2,
            observations_dropped: 0,
        })
    );
    let observed: Vec<_> = (0..2)
        .map(|_| {
            medium
                .take_observation(scanner)
                .unwrap_or_else(|error| unreachable!("scanner attached: {error}"))
        })
        .collect();
    let observation = |advertiser| {
        Some(BleObservation {
            advertiser,
            address: BleAddress::new([if advertiser == first { 1 } else { 2 }; 6]),
            received_signal_strength_dbm: -40,
            at: tick(0),
            advertisement: parameters(5).advertisement(),
        })
    };
    assert_eq!(observed, vec![observation(first), observation(second)]);

    assert!(medium.set_advertising(first, None).is_ok());
    assert_eq!(
        medium.advance_to_next_event(tick(100)),
        Ok(BleAdvanceReport {
            from: tick(0),
            to: tick(5),
            advertisements_emitted: 1,
            observations_queued: 1,
            observations_dropped: 0,
        })
    );
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(5),
            next_event_at: Some(tick(10))
        }
    );
    let emitted: Vec<_> = medium
        .trace()
        .events
        .into_iter()
        .filter_map(|event| match event {
            BleSimulationEvent::AdvertisementEmitted { radio, at, .. } => Some((radio, at)),
            _ => None,
        })
        .collect();
    assert_eq!(
        emitted,
        vec![(first, tick(0)), (second, tick(0)), (second, tick(5))]
    );
}

#[test]
fn ble_step_budget_refusal_is_atomic_even_when_events_are_due_now() {
    let medium = medium(1);
    let first = attach(&medium, 1);
    let second = attach(&medium, 2);
    let scanner = attach(&medium, 3);
    for radio in [first, second, scanner] {
        assert!(medium.set_radio_power(radio, BleRadioPower::On).is_ok());
    }
    assert!(medium.set_scanning(scanner, BleScanState::On).is_ok());
    for radio in [first, second] {
        assert!(medium.set_advertising(radio, Some(parameters(5))).is_ok());
    }
    let before = (medium.schedule(), medium.trace());
    assert_eq!(
        medium.advance_to_next_event(tick(u64::MAX)),
        Err(BleAdvanceError::EmissionBudgetExceeded { maximum: 1 })
    );
    assert_eq!((medium.schedule(), medium.trace()), before);
    assert_eq!(medium.take_observation(scanner), Ok(None));

    assert!(medium.set_radio_power(second, BleRadioPower::Off).is_ok());
    assert_eq!(
        medium.advance_to_next_event(tick(u64::MAX)),
        Ok(BleAdvanceReport {
            from: tick(0),
            to: tick(0),
            advertisements_emitted: 1,
            observations_queued: 1,
            observations_dropped: 0,
        })
    );
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(0),
            next_event_at: Some(tick(5))
        }
    );
}

#[test]
fn ble_step_refuses_backward_time_and_emits_the_last_tick_only_once() {
    let medium = medium(1);
    let advertiser = attach(&medium, 1);
    assert!(medium.advance_to_next_event(tick(1)).is_ok());
    let before = (medium.schedule(), medium.trace());
    assert_eq!(
        medium.advance_to_next_event(tick(0)),
        Err(BleAdvanceError::BeforeCurrent {
            current: tick(1),
            requested: tick(0),
        })
    );
    assert_eq!((medium.schedule(), medium.trace()), before);
    assert!(medium.advance_to_next_event(tick(u64::MAX)).is_ok());
    assert!(medium
        .set_radio_power(advertiser, BleRadioPower::On)
        .is_ok());
    assert!(medium
        .set_advertising(advertiser, Some(parameters(1)))
        .is_ok());
    assert_eq!(
        medium.schedule(),
        MediumSchedule {
            now: tick(u64::MAX),
            next_event_at: Some(tick(u64::MAX))
        }
    );
    assert_eq!(
        medium.advance_to_next_event(tick(u64::MAX)),
        Ok(BleAdvanceReport {
            from: tick(u64::MAX),
            to: tick(u64::MAX),
            advertisements_emitted: 1,
            observations_queued: 0,
            observations_dropped: 0,
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
        Ok(BleAdvanceReport {
            from: tick(u64::MAX),
            to: tick(u64::MAX),
            advertisements_emitted: 0,
            observations_queued: 0,
            observations_dropped: 0,
        })
    );
}

#[test]
fn ble_stepping_preserves_bulk_emissions_without_intervening_reactions() {
    let stepped = medium(16);
    let bulk = medium(16);
    for medium in [&stepped, &bulk] {
        let first = attach(medium, 1);
        let second = attach(medium, 2);
        let scanner = attach(medium, 3);
        for radio in [first, second, scanner] {
            assert!(medium.set_radio_power(radio, BleRadioPower::On).is_ok());
        }
        assert!(medium.set_scanning(scanner, BleScanState::On).is_ok());
        for (radio, interval) in [(first, 2), (second, 3)] {
            assert!(medium
                .set_advertising(radio, Some(parameters(interval)))
                .is_ok());
        }
    }
    assert!(bulk.advance_to(tick(7)).is_ok());
    for expected in [0, 2, 3, 4, 6, 7] {
        assert_eq!(
            stepped
                .advance_to_next_event(tick(7))
                .map(|report| report.to),
            Ok(tick(expected))
        );
    }
    let without_clock = |trace: BleTraceSnapshot| {
        trace
            .events
            .into_iter()
            .filter(|event| !matches!(event, BleSimulationEvent::TimeAdvanced { .. }))
            .collect::<Vec<_>>()
    };
    assert_eq!(without_clock(stepped.trace()), without_clock(bulk.trace()));
    assert_eq!(stepped.schedule(), bulk.schedule());
}
