use personal_rns::interfaces::bluetooth_auto::{columba_role_capabilities, MAX_ADVERTISEMENT_LEN};

use super::*;
use crate::{SimulationDurationInTicks, SimulationTick, TopologyConfig};

fn config(radios: usize, observations: usize, emissions: usize, trace: usize) -> BleMediumConfig {
    BleMediumConfig::new(
        TopologyConfig::FullyConnected,
        radios,
        observations,
        emissions,
        trace,
    )
    .unwrap_or_else(|error| unreachable!("test configuration is valid: {error}"))
}

fn parameters(role: BleRoleCapabilities, interval: u64) -> BleAdvertisingParameters {
    let advertisement = BleAdvertisement::reticulum(role)
        .unwrap_or_else(|error| unreachable!("production advertisement fits: {error}"));
    BleAdvertisingParameters::new(
        advertisement,
        SimulationDurationInTicks::from_ticks(interval),
    )
    .unwrap_or_else(|error| unreachable!("test interval is nonzero: {error}"))
}

fn attach_pair(medium: &VirtualBleMedium) -> (BleRadioId, BleRadioId) {
    let advertiser = medium
        .attach(BleAddress::new([1, 2, 3, 4, 5, 6]), -42)
        .unwrap_or_else(|error| unreachable!("advertiser attaches: {error}"));
    let scanner = medium
        .attach(BleAddress::new([6, 5, 4, 3, 2, 1]), -70)
        .unwrap_or_else(|error| unreachable!("scanner attaches: {error}"));
    (advertiser, scanner)
}

#[test]
fn reticulum_advertisement_preserves_production_semantics() {
    for role in [
        BleRoleCapabilities::DualRole,
        BleRoleCapabilities::PeripheralOnly,
    ] {
        let advertisement = BleAdvertisement::reticulum(role)
            .unwrap_or_else(|error| unreachable!("production advertisement fits: {error}"));
        assert!(advertisement.contains_reticulum_service());
        assert_eq!(
            columba_role_capabilities(advertisement.as_bytes()),
            Some(role),
        );
    }
}

#[test]
fn advertisement_and_configuration_reject_invalid_whole_values() {
    assert_eq!(
        BleAdvertisement::new(&[]),
        Err(BleAdvertisementError::Empty)
    );
    assert!(matches!(
        BleAdvertisement::new(&[0; MAX_ADVERTISEMENT_LEN + 1]),
        Err(BleAdvertisementError::TooLong { .. }),
    ));
    let advertisement = BleAdvertisement::new(&[1])
        .unwrap_or_else(|error| unreachable!("one byte is valid: {error}"));
    assert_eq!(
        BleAdvertisingParameters::new(advertisement, SimulationDurationInTicks::ZERO),
        Err(BleAdvertisingParametersError::ZeroInterval),
    );

    for (values, field) in [
        ((0, 1, 1, 1), BleCapacityField::Radios),
        ((1, 0, 1, 1), BleCapacityField::ObservationQueue),
        ((1, 1, 0, 1), BleCapacityField::EmissionsPerAdvance),
        ((1, 1, 1, 0), BleCapacityField::Trace),
    ] {
        assert_eq!(
            BleMediumConfig::new(
                TopologyConfig::FullyConnected,
                values.0,
                values.1,
                values.2,
                values.3
            ),
            Err(BleMediumConfigError::ZeroCapacity(field)),
        );
    }
}

#[test]
fn radio_advertising_and_scanning_states_all_gate_discovery() {
    let medium = VirtualBleMedium::new(config(2, 4, 8, 32));
    let (advertiser, scanner) = attach_pair(&medium);
    assert_eq!(
        medium.set_advertising(
            advertiser,
            Some(parameters(BleRoleCapabilities::DualRole, 2)),
        ),
        Ok(BleRadioMutation::Applied),
    );
    assert_eq!(
        medium
            .advance_to(SimulationTick::ZERO)
            .map(|r| r.advertisements_emitted),
        Ok(0)
    );

    assert_eq!(
        medium.set_radio_power(advertiser, BleRadioPower::On),
        Ok(BleRadioMutation::Applied),
    );
    assert_eq!(
        medium
            .advance_to(SimulationTick::ZERO)
            .map(|r| r.advertisements_emitted),
        Ok(1)
    );
    assert_eq!(medium.take_observation(scanner), Ok(None));

    assert_eq!(
        medium.set_radio_power(scanner, BleRadioPower::On),
        Ok(BleRadioMutation::Applied),
    );
    assert_eq!(
        medium.set_scanning(scanner, BleScanState::On),
        Ok(BleRadioMutation::Applied),
    );
    let report = medium
        .advance_to(SimulationTick::from_ticks(2))
        .unwrap_or_else(|error| unreachable!("advance is bounded: {error}"));
    assert_eq!(report.advertisements_emitted, 1);
    assert_eq!(report.observations_queued, 1);
    let observed = medium
        .take_observation(scanner)
        .unwrap_or_else(|error| unreachable!("scanner exists: {error}"))
        .unwrap_or_else(|| unreachable!("one advertisement was observed"));
    assert_eq!(observed.advertiser, advertiser);
    assert_eq!(observed.at, SimulationTick::from_ticks(2));
    assert_eq!(observed.received_signal_strength_dbm, -42);
    assert!(observed.advertisement.contains_reticulum_service());
}

#[test]
fn emissions_have_stable_time_then_radio_order() {
    let medium = VirtualBleMedium::new(config(3, 8, 16, 64));
    let first = medium
        .attach(BleAddress::new([1; 6]), -10)
        .unwrap_or_else(|error| unreachable!("first attaches: {error}"));
    let second = medium
        .attach(BleAddress::new([2; 6]), -20)
        .unwrap_or_else(|error| unreachable!("second attaches: {error}"));
    let scanner = medium
        .attach(BleAddress::new([3; 6]), -30)
        .unwrap_or_else(|error| unreachable!("scanner attaches: {error}"));
    for radio in [first, second, scanner] {
        assert_eq!(
            medium.set_radio_power(radio, BleRadioPower::On),
            Ok(BleRadioMutation::Applied),
        );
    }
    assert_eq!(
        medium.set_scanning(scanner, BleScanState::On),
        Ok(BleRadioMutation::Applied),
    );
    for radio in [second, first] {
        assert_eq!(
            medium.set_advertising(radio, Some(parameters(BleRoleCapabilities::DualRole, 2)),),
            Ok(BleRadioMutation::Applied),
        );
    }

    let report = medium
        .advance_to(SimulationTick::from_ticks(2))
        .unwrap_or_else(|error| unreachable!("four emissions fit: {error}"));
    assert_eq!(report.advertisements_emitted, 4);
    let observed: Vec<_> = (0..4)
        .map(|_| {
            medium
                .take_observation(scanner)
                .unwrap_or_else(|error| unreachable!("scanner exists: {error}"))
                .unwrap_or_else(|| unreachable!("observation is queued"))
        })
        .map(|observation| (observation.at, observation.advertiser))
        .collect();
    assert_eq!(
        observed,
        vec![
            (SimulationTick::ZERO, first),
            (SimulationTick::ZERO, second),
            (SimulationTick::from_ticks(2), first),
            (SimulationTick::from_ticks(2), second),
        ],
    );
}

#[test]
fn excessive_advance_is_rejected_before_time_or_queues_change() {
    let medium = VirtualBleMedium::new(config(2, 8, 2, 32));
    let (advertiser, scanner) = attach_pair(&medium);
    for radio in [advertiser, scanner] {
        assert!(medium.set_radio_power(radio, BleRadioPower::On).is_ok());
    }
    assert!(medium.set_scanning(scanner, BleScanState::On).is_ok());
    assert!(medium
        .set_advertising(
            advertiser,
            Some(parameters(BleRoleCapabilities::DualRole, 1)),
        )
        .is_ok());

    assert_eq!(
        medium.advance_to(SimulationTick::from_ticks(2)),
        Err(BleAdvanceError::EmissionBudgetExceeded { maximum: 2 }),
    );
    assert_eq!(medium.now(), SimulationTick::ZERO);
    assert_eq!(medium.take_observation(scanner), Ok(None));

    let report = medium
        .advance_to(SimulationTick::from_ticks(1))
        .unwrap_or_else(|error| unreachable!("two emissions fit: {error}"));
    assert_eq!(report.advertisements_emitted, 2);
    assert_eq!(report.observations_queued, 2);
}

#[test]
fn full_observation_queue_is_a_typed_visible_drop() {
    let medium = VirtualBleMedium::new(config(2, 1, 4, 32));
    let (advertiser, scanner) = attach_pair(&medium);
    for radio in [advertiser, scanner] {
        assert!(medium.set_radio_power(radio, BleRadioPower::On).is_ok());
    }
    assert!(medium.set_scanning(scanner, BleScanState::On).is_ok());
    assert!(medium
        .set_advertising(
            advertiser,
            Some(parameters(BleRoleCapabilities::DualRole, 1)),
        )
        .is_ok());

    let report = medium
        .advance_to(SimulationTick::from_ticks(1))
        .unwrap_or_else(|error| unreachable!("two emissions fit: {error}"));
    assert_eq!(report.observations_queued, 1);
    assert_eq!(report.observations_dropped, 1);
    assert!(medium.trace().events.iter().any(|event| matches!(
        event,
        BleSimulationEvent::ObservationDropped {
            advertiser: source,
            scanner: target,
            reason: BleObservationDropReason::ObservationQueueFull,
            ..
        } if *source == advertiser && *target == scanner
    )));
}
