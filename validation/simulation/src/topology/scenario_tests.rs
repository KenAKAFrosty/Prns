use crate::ble::{
    BleAddress, BleAdvanceReport, BleAdvertisement, BleAdvertisingParameters, BleMediumConfig,
    BleObservation, BleRadioPower, BleRoleCapabilities, BleScanState, VirtualBleMedium,
};
use crate::*;

#[test]
fn a_thousand_radio_chain_delivers_only_to_adjacent_scanners(
) -> Result<(), Box<dyn std::error::Error>> {
    const RADIOS: u16 = 1_024;
    let topology = TopologyConfig::Explicit {
        max_neighbors: 2.try_into()?,
    };
    let medium = VirtualBleMedium::new(BleMediumConfig::new(
        topology,
        usize::from(RADIOS),
        2,
        usize::from(RADIOS),
        64,
    )?);
    let advertisement = BleAdvertisement::reticulum(BleRoleCapabilities::DualRole)?;
    let parameters =
        BleAdvertisingParameters::new(advertisement, SimulationDurationInTicks::from_ticks(1))?;
    let address = |index: u16| {
        let [high, low] = index.to_be_bytes();
        BleAddress::new([high, low, 0, 0, 0, 1])
    };
    let mut radios = Vec::new();
    for index in 0..RADIOS {
        let radio = medium.attach(address(index), -40)?;
        medium.set_radio_power(radio, BleRadioPower::On)?;
        medium.set_scanning(radio, BleScanState::On)?;
        medium.set_advertising(radio, Some(parameters))?;
        radios.push(radio);
    }
    for adjacent in radios.windows(2) {
        medium.set_reachability(adjacent[0], adjacent[1], Reachability::Reachable)?;
    }
    assert_eq!(
        medium.advance_to(SimulationTick::ZERO)?,
        BleAdvanceReport {
            from: SimulationTick::ZERO,
            to: SimulationTick::ZERO,
            advertisements_emitted: usize::from(RADIOS),
            observations_queued: 2 * usize::from(RADIOS - 1),
            observations_dropped: 0,
        }
    );
    for (index, radio) in radios.iter().copied().enumerate() {
        let mut actual = Vec::new();
        while let Some(observation) = medium.take_observation(radio)? {
            actual.push(observation);
        }
        let expected: Vec<_> = [
            index.checked_sub(1),
            (index + 1 < radios.len()).then_some(index + 1),
        ]
        .into_iter()
        .flatten()
        .map(|neighbor| BleObservation {
            advertiser: radios[neighbor],
            address: address(neighbor as u16),
            received_signal_strength_dbm: -40,
            at: SimulationTick::ZERO,
            advertisement,
        })
        .collect();
        assert_eq!(actual, expected);
    }
    assert_eq!(medium.trace().events.len(), 64);
    assert!(medium.trace().discarded_events > 0);
    Ok(())
}

#[test]
fn frame_partitions_preserve_in_flight_deliveries_but_block_new_ones(
) -> Result<(), Box<dyn std::error::Error>> {
    let faults = FaultPlan::new(vec![TransmissionRule::delay(
        TransmissionOrdinal::new(0),
        SimulationDurationInTicks::from_ticks(1),
    )])?;
    let medium = VirtualMedium::new(VirtualMediumConfig::new(
        TopologyConfig::Explicit {
            max_neighbors: 2.try_into()?,
        },
        4,
        4,
        4,
        64,
        faults,
    )?);
    let first = medium.attach(b"first")?;
    let bridge = medium.attach(b"bridge")?;
    let last = medium.attach(b"last")?;
    let _isolated = medium.attach(b"isolated")?;
    let (a, b, c) = (
        first.endpoint_id(),
        bridge.endpoint_id(),
        last.endpoint_id(),
    );
    medium.set_reachability(a, b, Reachability::Reachable)?;
    medium.set_reachability(b, c, Reachability::Reachable)?;
    assert_eq!(medium.transmit(b, vec![1]), Ok(()));
    assert_eq!(medium.pending_delivery_count(), 2);
    medium.set_reachability(b, c, Reachability::Isolated)?;
    assert_eq!(medium.transmit(b, vec![2]), Ok(()));
    assert_eq!(
        medium.advance_by(SimulationDurationInTicks::from_ticks(1))?,
        AdvanceReport {
            from: SimulationTick::ZERO,
            to: SimulationTick::from_ticks(1),
            receptions_queued: 2,
            receptions_dropped: 0,
        }
    );
    medium.set_reachability(b, c, Reachability::Reachable)?;
    assert_eq!(medium.transmit(b, vec![3]), Ok(()));
    let recipients: Vec<_> = medium
        .trace()
        .events
        .into_iter()
        .filter_map(|event| match event {
            MediumEvent::ReceptionQueued { ordinal, to, .. } => Some((ordinal, to)),
            _ => None,
        })
        .collect();
    assert_eq!(
        recipients,
        vec![
            (TransmissionOrdinal::new(1), a),
            (TransmissionOrdinal::new(0), a),
            (TransmissionOrdinal::new(0), c),
            (TransmissionOrdinal::new(2), a),
            (TransmissionOrdinal::new(2), c),
        ]
    );
    drop(last);
    let before = medium.trace();
    assert_eq!(
        medium.set_reachability(b, c, Reachability::Reachable),
        Err(TopologyError::UnknownNode(c))
    );
    assert_eq!(medium.trace(), before);
    Ok(())
}
