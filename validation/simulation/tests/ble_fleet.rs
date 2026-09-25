use std::error::Error;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, DialOutcome, RadioMode,
    ScanningMode, CONTROL_MAX_LEN,
};
use prns_simulation::ble::{
    BleAddress, BleAdvanceReport, BleMediumConfig, BleRoleCapabilities, VirtualBleBackend,
    VirtualBleBackendConfig, VirtualBleBackendLimits, VirtualBleDisconnectReport, VirtualBleError,
    VirtualBleLab, VirtualBleLink, VirtualBleLinkConfig, VirtualGattConfig,
};
use prns_simulation::{Reachability, SimulationDurationInTicks, SimulationTick, TopologyConfig};

const MAX_PEERS: usize = 1;
const FRAME_LENGTH: usize = 8;
const TRACE_CAPACITY: usize = 32;
const MEASUREMENT_REPLACEMENTS: usize = 512;

struct BackendPair {
    dialer: VirtualBleBackend,
    listener: VirtualBleBackend,
    dialer_address: BleAddress,
    listener_address: BleAddress,
}

impl BackendPair {
    async fn connect(&mut self) -> (VirtualBleLink, VirtualBleLink) {
        assert_eq!(
            BleBackend::<MAX_PEERS>::dial(&mut self.dialer, self.listener_address).await,
            DialOutcome::Started
        );
        let BleEvent::LinkReady { link: dialed, .. } =
            BleBackend::<MAX_PEERS>::next_event(&mut self.dialer).await
        else {
            unreachable!("dialer receives its ready link")
        };
        let BleEvent::LinkReady { link: accepted, .. } =
            BleBackend::<MAX_PEERS>::next_event(&mut self.listener).await
        else {
            unreachable!("listener receives its ready link")
        };
        (dialed, accepted)
    }
}

struct ConnectedPair {
    backends: BackendPair,
    links: (VirtualBleLink, VirtualBleLink),
}

struct Fleet {
    lab: VirtualBleLab,
    pairs: Vec<ConnectedPair>,
}

impl Fleet {
    async fn new(pair_count: usize) -> Result<Self, Box<dyn Error>> {
        let lab = VirtualBleLab::new(BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: NonZeroUsize::MIN,
            },
            pair_count * 2,
            1,
            pair_count,
            TRACE_CAPACITY,
        )?);
        let link = VirtualBleLinkConfig::new(
            1,
            1,
            FRAME_LENGTH,
            VirtualGattConfig::new(CONTROL_MAX_LEN, 20)?,
        )?;
        let mut backends = Vec::with_capacity(pair_count);
        for index in 0..pair_count {
            let address = |ordinal: usize| -> Result<BleAddress, Box<dyn Error>> {
                let bytes = u32::try_from(ordinal)?.to_be_bytes();
                Ok(BleAddress::new([
                    2, 0, bytes[0], bytes[1], bytes[2], bytes[3],
                ]))
            };
            let dialer_address = address(index * 2)?;
            let listener_address = address(index * 2 + 1)?;
            let config = |address| {
                VirtualBleBackendConfig::new(
                    address,
                    -40,
                    BleRoleCapabilities::DualRole,
                    SimulationDurationInTicks::from_ticks(1),
                    VirtualBleBackendLimits {
                        inbound_links: NonZeroUsize::MIN,
                        connections: NonZeroUsize::MIN,
                        discovered_peers: NonZeroUsize::MIN,
                    },
                    link,
                )
            };
            let mut dialer = lab.attach_backend(config(dialer_address)?)?;
            let mut listener = lab.attach_backend(config(listener_address)?)?;
            lab.set_reachability(dialer_address, listener_address, Reachability::Reachable)?;
            BleBackend::<MAX_PEERS>::set_radio_mode(&mut dialer, RadioMode::On).await?;
            BleBackend::<MAX_PEERS>::set_radio_mode(&mut listener, RadioMode::On).await?;
            BleBackend::<MAX_PEERS>::set_scanning(&mut dialer, ScanningMode::On).await?;
            BleBackend::<MAX_PEERS>::set_advertising(&mut listener, AdvertisingMode::On).await?;
            backends.push(BackendPair {
                dialer,
                listener,
                dialer_address,
                listener_address,
            });
        }
        assert_eq!(
            lab.advance_to(SimulationTick::ZERO)?,
            BleAdvanceReport {
                from: SimulationTick::ZERO,
                to: SimulationTick::ZERO,
                advertisements_emitted: pair_count,
                observations_queued: pair_count,
                observations_dropped: 0,
            }
        );
        let mut pairs = Vec::with_capacity(pair_count);
        for mut backends in backends {
            assert!(matches!(
                BleBackend::<MAX_PEERS>::next_event(&mut backends.dialer).await,
                BleEvent::Sighting { address, .. } if address == backends.listener_address
            ));
            let links = backends.connect().await;
            pairs.push(ConnectedPair { backends, links });
        }
        assert_eq!(lab.active_connection_count(), pair_count);
        Ok(Self { lab, pairs })
    }

    async fn churn(&mut self, replacements: usize) -> Duration {
        let started = Instant::now();
        for step in 0..replacements {
            let index = step % self.pairs.len();
            let pair = &mut self.pairs[index];
            assert_eq!(
                BleBackend::<MAX_PEERS>::dial(
                    &mut pair.backends.dialer,
                    pair.backends.listener_address
                )
                .await,
                DialOutcome::Busy
            );
            let closed = if step % 2 == 0 {
                self.lab.disconnect_between(
                    pair.backends.dialer_address,
                    pair.backends.listener_address,
                )
            } else {
                self.lab.disconnect_radio(pair.backends.listener_address)
            };
            assert_eq!(
                closed,
                VirtualBleDisconnectReport {
                    connections_closed: 1
                }
            );
            assert_eq!(
                pair.links.0.control_recv().await,
                Err(VirtualBleError::LinkClosed)
            );
            assert_eq!(
                pair.links.1.control_recv().await,
                Err(VirtualBleError::LinkClosed)
            );
            // Old endpoints outlive admission of their replacements.
            pair.links = pair.backends.connect().await;
        }
        started.elapsed()
    }

    async fn verify_and_retire(self) -> Result<(), Box<dyn Error>> {
        assert_eq!(self.lab.active_connection_count(), self.pairs.len());
        for (index, pair) in self.pairs.into_iter().enumerate() {
            let payload = u64::try_from(index)?.to_be_bytes();
            let (_dialer_source, mut dialer_sink) = pair.links.0.into_data();
            let (mut listener_source, _listener_sink) = pair.links.1.into_data();
            dialer_sink.send_frame(&payload).await?;
            let mut received = [0; FRAME_LENGTH];
            assert_eq!(
                listener_source.recv_frame(&mut received).await,
                Ok(FRAME_LENGTH)
            );
            assert_eq!(received, payload);
        }
        assert_eq!(self.lab.active_connection_count(), 0);
        Ok(())
    }
}

#[tokio::test]
async fn sparse_fleet_churn_preserves_unrelated_links_and_reclaims_capacity(
) -> Result<(), Box<dyn Error>> {
    let mut fleet = Fleet::new(1_024).await?;
    let _ = fleet.churn(128).await;
    fleet.verify_and_retire().await
}

#[tokio::test]
#[ignore = "manual comparative timing; no performance threshold or release claim"]
async fn measure_fleet_connection_churn() -> Result<(), Box<dyn Error>> {
    for pair_count in [32, 256, 2_048] {
        let mut elapsed = Vec::new();
        for _ in 0..3 {
            let mut fleet = Fleet::new(pair_count).await?;
            elapsed.push(fleet.churn(MEASUREMENT_REPLACEMENTS).await);
            fleet.verify_and_retire().await?;
        }
        println!(
            "BLE_CONNECTION_CHURN pairs={pair_count} replacements={MEASUREMENT_REPLACEMENTS} samples={elapsed:?}"
        );
    }
    Ok(())
}
