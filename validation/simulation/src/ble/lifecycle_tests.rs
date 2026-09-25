use std::error::Error;
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use super::*;
use crate::{
    Reachability, SimulationDurationInTicks, SimulationTick, TopologyConfig, TopologyError,
};

const ADVERTISER_ADDRESS: BleAddress = BleAddress::new([1; 6]);
const SCANNER_ADDRESS: BleAddress = BleAddress::new([2; 6]);
const THIRD_ADDRESS: BleAddress = BleAddress::new([3; 6]);

fn medium() -> Result<VirtualBleMedium, BleMediumConfigError> {
    Ok(VirtualBleMedium::new(BleMediumConfig::new(
        TopologyConfig::Explicit {
            max_neighbors: NonZeroUsize::MIN,
        },
        2,
        2,
        2,
        32,
    )?))
}

#[test]
fn capacity_is_a_live_radio_limit_and_refused_attachments_do_not_consume_ids(
) -> Result<(), Box<dyn Error>> {
    let medium = medium()?;
    let first = medium.attach(ADVERTISER_ADDRESS, -40)?;
    let before = medium.trace();
    assert_eq!(
        medium.attach(ADVERTISER_ADDRESS, -50),
        Err(BleSimulationError::DuplicateAddress)
    );
    assert_eq!(medium.trace(), before);
    let second = medium.attach(SCANNER_ADDRESS, -50)?;
    let before = medium.trace();
    assert_eq!(
        medium.attach(THIRD_ADDRESS, -60),
        Err(BleSimulationError::RadioCapacityReached)
    );
    assert_eq!(medium.trace(), before);
    medium.detach(first);
    let before = medium.trace();
    medium.detach(first);
    assert_eq!(medium.trace(), before);
    let third = medium.attach(THIRD_ADDRESS, -60)?;
    assert_eq!([first.get(), second.get(), third.get()], [0, 1, 2]);
    assert_eq!(medium.take_observation(second), Ok(None));
    assert_eq!(medium.take_observation(third), Ok(None));
    Ok(())
}

#[test]
fn configured_live_capacity_is_not_artificially_limited_to_sixteen_bit_ids() {
    assert!(BleMediumConfig::new(
        TopologyConfig::FullyConnected,
        usize::from(u16::MAX) + 2,
        1,
        1,
        1
    )
    .is_ok());
}

#[tokio::test]
async fn stale_handles_and_queued_sightings_never_target_the_replacement(
) -> Result<(), Box<dyn Error>> {
    let medium = medium()?;
    let old = medium.attach(ADVERTISER_ADDRESS, -40)?;
    let scanner = medium.attach(SCANNER_ADDRESS, -50)?;
    medium.set_reachability(old, scanner, Reachability::Reachable)?;
    for radio in [old, scanner] {
        medium.set_radio_power(radio, BleRadioPower::On)?;
    }
    medium.set_scanning(scanner, BleScanState::On)?;
    let parameters = BleAdvertisingParameters::new(
        BleAdvertisement::reticulum(BleRoleCapabilities::DualRole)?,
        SimulationDurationInTicks::from_ticks(1),
    )?;
    medium.set_advertising(old, Some(parameters))?;
    medium.advance_to(SimulationTick::ZERO)?;
    medium.detach(old);
    let replacement = medium.attach(ADVERTISER_ADDRESS, -60)?;
    assert_ne!(old, replacement);
    let before = medium.trace();
    let unknown = BleSimulationError::UnknownRadio(old);
    assert_eq!(medium.set_radio_power(old, BleRadioPower::On), Err(unknown));
    assert_eq!(medium.set_scanning(old, BleScanState::On), Err(unknown));
    assert_eq!(medium.set_advertising(old, Some(parameters)), Err(unknown));
    assert_eq!(medium.take_observation(old), Err(unknown));
    assert_eq!(medium.next_observation(old).await, Err(unknown));
    assert_eq!(
        medium.set_reachability(old, scanner, Reachability::Reachable),
        Err(TopologyError::UnknownNode(old))
    );
    medium.detach(old);
    assert_eq!(medium.trace(), before);
    assert!(!medium.is_powered(replacement));
    medium.set_radio_power(replacement, BleRadioPower::On)?;
    medium.set_advertising(replacement, Some(parameters))?;
    assert!(!medium.is_connectable(scanner, replacement));
    assert_eq!(
        medium.advance_to(SimulationTick::ZERO)?,
        BleAdvanceReport {
            from: SimulationTick::ZERO,
            to: SimulationTick::ZERO,
            advertisements_emitted: 1,
            observations_queued: 0,
            observations_dropped: 0,
        }
    );
    assert_eq!(
        medium.take_observation(scanner)?,
        Some(BleObservation {
            advertiser: old,
            address: ADVERTISER_ADDRESS,
            received_signal_strength_dbm: -40,
            at: SimulationTick::ZERO,
            advertisement: parameters.advertisement(),
        })
    );
    assert_eq!(medium.take_observation(scanner), Ok(None));
    medium.set_reachability(replacement, scanner, Reachability::Reachable)?;
    medium.advance_to(SimulationTick::from_ticks(1))?;
    assert_eq!(
        medium.take_observation(scanner)?,
        Some(BleObservation {
            advertiser: replacement,
            address: ADVERTISER_ADDRESS,
            received_signal_strength_dbm: -60,
            at: SimulationTick::from_ticks(1),
            advertisement: parameters.advertisement(),
        })
    );
    Ok(())
}

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn detach_wakes_every_waiter_without_consuming_replacement_observations(
) -> Result<(), Box<dyn Error>> {
    let medium = medium()?;
    let radio = medium.attach(SCANNER_ADDRESS, -40)?;
    let mut first = std::pin::pin!(medium.next_observation(radio));
    let mut second = std::pin::pin!(medium.next_observation(radio));
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut context = Context::from_waker(&waker);
    assert_eq!(first.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(second.as_mut().poll(&mut context), Poll::Pending);
    medium.detach(radio);
    assert_eq!(wakes.0.load(Ordering::SeqCst), 2);
    let replacement = medium.attach(SCANNER_ADDRESS, -50)?;
    assert_ne!(radio, replacement);
    let advertiser = medium.attach(ADVERTISER_ADDRESS, -60)?;
    let parameters = BleAdvertisingParameters::new(
        BleAdvertisement::reticulum(BleRoleCapabilities::DualRole)?,
        SimulationDurationInTicks::from_ticks(1),
    )?;
    for radio in [replacement, advertiser] {
        medium.set_radio_power(radio, BleRadioPower::On)?;
    }
    medium.set_reachability(advertiser, replacement, Reachability::Reachable)?;
    medium.set_scanning(replacement, BleScanState::On)?;
    medium.set_advertising(advertiser, Some(parameters))?;
    medium.advance_to(SimulationTick::ZERO)?;
    for mut future in [first.as_mut(), second.as_mut()] {
        assert_eq!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Err(BleSimulationError::UnknownRadio(radio)))
        );
    }
    assert_eq!(
        medium.take_observation(replacement)?,
        Some(BleObservation {
            advertiser,
            address: ADVERTISER_ADDRESS,
            received_signal_strength_dbm: -60,
            at: SimulationTick::ZERO,
            advertisement: parameters.advertisement(),
        })
    );
    Ok(())
}
