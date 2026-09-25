use std::error::Error;
use std::future::{poll_fn, Future};
use std::num::NonZeroUsize;
use std::task::Poll;

use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, DialOutcome, RadioMode,
    ScanningMode, CONTROL_MAX_LEN,
};

use crate::ble::*;
use crate::{SimulationDurationInTicks, TopologyConfig};

const MAX_PEERS: usize = 1;
const FRAME_LENGTH: usize = 8;

struct Scenario {
    lab: VirtualBleLab,
    scanner: VirtualBleBackend,
}

impl Scenario {
    async fn new(discovered_peers: NonZeroUsize) -> Result<Self, Box<dyn Error>> {
        let lab = VirtualBleLab::new(BleMediumConfig::new(
            TopologyConfig::FullyConnected,
            4,
            4,
            4,
            8,
        )?);
        let mut scanner = lab.attach_backend(config(BleAddress::new([0; 6]), discovered_peers)?)?;
        BleBackend::<MAX_PEERS>::set_radio_mode(&mut scanner, RadioMode::On).await?;
        BleBackend::<MAX_PEERS>::set_scanning(&mut scanner, ScanningMode::On).await?;
        Ok(Self { lab, scanner })
    }

    async fn advertiser(&self, address: BleAddress) -> Result<VirtualBleBackend, Box<dyn Error>> {
        let mut backend = self
            .lab
            .attach_backend(config(address, NonZeroUsize::MIN)?)?;
        BleBackend::<MAX_PEERS>::set_radio_mode(&mut backend, RadioMode::On).await?;
        BleBackend::<MAX_PEERS>::set_advertising(&mut backend, AdvertisingMode::On).await?;
        Ok(backend)
    }

    fn emit(&self) -> Result<(), Box<dyn Error>> {
        let report = self.lab.advance_to(self.lab.now())?;
        assert_eq!(
            (report.advertisements_emitted, report.observations_queued),
            (1, 1)
        );
        Ok(())
    }

    async fn sighting(&mut self, expected: BleAddress) {
        assert!(
            matches!(BleBackend::<MAX_PEERS>::next_event(&mut self.scanner).await,
            BleEvent::Sighting { address, rssi: Some(-40) } if address == expected)
        );
    }

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        BleBackend::<MAX_PEERS>::dial(&mut self.scanner, address).await
    }
}

fn config(
    address: BleAddress,
    discovered_peers: NonZeroUsize,
) -> Result<VirtualBleBackendConfig, VirtualBleBackendConfigError> {
    VirtualBleBackendConfig::new(
        address,
        -40,
        BleRoleCapabilities::DualRole,
        SimulationDurationInTicks::from_ticks(1),
        VirtualBleBackendLimits {
            inbound_links: NonZeroUsize::MIN,
            connections: NonZeroUsize::MIN,
            discovered_peers,
        },
        VirtualBleLinkConfig::new(
            1,
            1,
            FRAME_LENGTH,
            VirtualGattConfig::new(CONTROL_MAX_LEN, 20)
                .unwrap_or_else(|error| unreachable!("valid GATT: {error}")),
        )?,
    )
}

#[tokio::test]
async fn eviction_requires_rediscovery_but_does_not_close_existing_links(
) -> Result<(), Box<dyn Error>> {
    let mut scenario = Scenario::new(NonZeroUsize::MIN).await?;
    let first_address = BleAddress::new([1; 6]);
    let mut first = scenario.advertiser(first_address).await?;
    scenario.emit()?;
    scenario.sighting(first_address).await;
    assert_eq!(scenario.dial(first_address).await, DialOutcome::Started);
    let BleEvent::LinkReady { link: dialed, .. } =
        BleBackend::<MAX_PEERS>::next_event(&mut scenario.scanner).await
    else {
        unreachable!("dialer receives its link")
    };
    let BleEvent::LinkReady { link: accepted, .. } =
        BleBackend::<MAX_PEERS>::next_event(&mut first).await
    else {
        unreachable!("listener receives its link")
    };
    let (dialed_source, mut sink) = dialed.into_data();
    let (mut source, accepted_sink) = accepted.into_data();
    let second_address = BleAddress::new([2; 6]);
    let _second = scenario.advertiser(second_address).await?;
    scenario.emit()?;
    scenario.sighting(second_address).await;
    let snapshot = scenario.scanner.discovery_snapshot();
    assert_eq!(
        snapshot
            .peers
            .iter()
            .map(|peer| peer.address)
            .collect::<Vec<_>>(),
        vec![second_address]
    );
    assert_eq!(snapshot.evicted_peers, 1);
    assert_eq!(scenario.dial(first_address).await, DialOutcome::UnknownPeer);
    sink.send_frame(b"survives").await?;
    let mut received = [0; FRAME_LENGTH];
    assert_eq!(source.recv_frame(&mut received).await?, FRAME_LENGTH);
    assert_eq!(&received, b"survives");
    assert_eq!(scenario.lab.active_connection_count(), 1);
    drop((dialed_source, sink, source, accepted_sink));
    BleBackend::<MAX_PEERS>::set_advertising(&mut first, AdvertisingMode::Off).await?;
    BleBackend::<MAX_PEERS>::set_advertising(&mut first, AdvertisingMode::On).await?;
    scenario.emit()?;
    scenario.sighting(first_address).await;
    assert_eq!(scenario.scanner.discovery_snapshot().evicted_peers, 2);
    assert_eq!(scenario.dial(first_address).await, DialOutcome::Started);
    Ok(())
}

enum SightingDelivery {
    BeforeReplacement,
    AfterReplacement,
}

#[tokio::test]
async fn cached_and_queued_sightings_do_not_authorize_a_reused_address(
) -> Result<(), Box<dyn Error>> {
    for delivery in [
        SightingDelivery::BeforeReplacement,
        SightingDelivery::AfterReplacement,
    ] {
        let mut scenario = Scenario::new(NonZeroUsize::MIN).await?;
        let address = BleAddress::new([1; 6]);
        let old = scenario.advertiser(address).await?;
        scenario.emit()?;
        if let SightingDelivery::BeforeReplacement = delivery {
            scenario.sighting(address).await;
        }
        drop(old);
        let _replacement = scenario.advertiser(address).await?;
        if let SightingDelivery::AfterReplacement = delivery {
            scenario.sighting(address).await;
        }
        let old_snapshot = scenario.scanner.discovery_snapshot();
        assert_eq!(scenario.dial(address).await, DialOutcome::UnknownPeer);
        assert_eq!(scenario.scanner.discovery_snapshot(), old_snapshot);
        scenario.emit()?;
        scenario.sighting(address).await;
        let replacement = scenario.scanner.discovery_snapshot();
        assert_eq!(replacement.peers.len(), 1);
        assert_ne!(replacement.peers[0].radio, old_snapshot.peers[0].radio);
        assert_eq!(replacement.evicted_peers, 0);
        assert_eq!(scenario.dial(address).await, DialOutcome::Started);
    }
    Ok(())
}

#[tokio::test]
async fn power_off_clears_both_delivered_and_queued_discovery_but_scan_stop_does_not(
) -> Result<(), Box<dyn Error>> {
    let mut scenario = Scenario::new(NonZeroUsize::MIN).await?;
    let first_address = BleAddress::new([1; 6]);
    let _first = scenario.advertiser(first_address).await?;
    scenario.emit()?;
    scenario.sighting(first_address).await;
    let second_address = BleAddress::new([2; 6]);
    let mut second = scenario.advertiser(second_address).await?;
    scenario.emit()?;
    scenario.sighting(second_address).await;
    BleBackend::<MAX_PEERS>::set_advertising(&mut second, AdvertisingMode::Off).await?;
    BleBackend::<MAX_PEERS>::set_advertising(&mut second, AdvertisingMode::On).await?;
    scenario.emit()?;
    for mode in [RadioMode::Off, RadioMode::Off, RadioMode::On] {
        BleBackend::<MAX_PEERS>::set_radio_mode(&mut scenario.scanner, mode).await?;
    }
    let empty = BleDiscoverySnapshot {
        capacity: NonZeroUsize::MIN,
        peers: vec![],
        evicted_peers: 1,
    };
    assert_eq!(scenario.scanner.discovery_snapshot(), empty);
    {
        let mut next = std::pin::pin!(BleBackend::<MAX_PEERS>::next_event(&mut scenario.scanner));
        poll_fn(|cx| {
            assert!(next.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert_eq!(scenario.scanner.discovery_snapshot(), empty);
    assert_eq!(
        scenario.dial(second_address).await,
        DialOutcome::UnknownPeer
    );
    BleBackend::<MAX_PEERS>::set_advertising(&mut second, AdvertisingMode::Off).await?;
    BleBackend::<MAX_PEERS>::set_advertising(&mut second, AdvertisingMode::On).await?;
    scenario.emit()?;
    scenario.sighting(second_address).await;
    let seen = scenario.scanner.discovery_snapshot();
    BleBackend::<MAX_PEERS>::set_scanning(&mut scenario.scanner, ScanningMode::Off).await?;
    assert_eq!(scenario.scanner.discovery_snapshot(), seen);
    assert_eq!(scenario.dial(second_address).await, DialOutcome::Started);
    Ok(())
}

#[tokio::test]
async fn thousands_of_departed_peers_retain_only_the_configured_history(
) -> Result<(), Box<dyn Error>> {
    const CAPACITY: usize = 3;
    const PEERS: u16 = 2_048;
    let capacity = NonZeroUsize::new(CAPACITY).unwrap_or_else(|| unreachable!());
    let mut scenario = Scenario::new(capacity).await?;
    let mut expected = Vec::new();
    for ordinal in 1..=PEERS {
        let [high, low] = ordinal.to_be_bytes();
        let address = BleAddress::new([0, 0, 0, 0, high, low]);
        let peer = scenario.advertiser(address).await?;
        let radio = scenario
            .lab
            .trace()
            .events
            .into_iter()
            .rev()
            .find_map(|event| {
                if let BleSimulationEvent::RadioAttached { radio } = event {
                    Some(radio)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| unreachable!("new attachment is in the bounded trace"));
        scenario.emit()?;
        scenario.sighting(address).await;
        let snapshot = scenario.scanner.discovery_snapshot();
        expected.push(BleDiscoveredPeer { address, radio });
        if expected.len() > CAPACITY {
            expected.remove(0);
        }
        assert_eq!(
            snapshot,
            BleDiscoverySnapshot {
                capacity,
                peers: expected.clone(),
                evicted_peers: u64::from(ordinal).saturating_sub(CAPACITY as u64),
            }
        );
        drop(peer);
        assert_eq!(scenario.dial(address).await, DialOutcome::UnknownPeer);
    }
    assert_eq!(scenario.lab.active_connection_count(), 0);
    Ok(())
}
