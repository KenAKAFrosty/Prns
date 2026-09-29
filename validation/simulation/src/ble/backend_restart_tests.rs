use std::error::Error;
use std::future::{poll_fn, Future};
use std::num::NonZeroUsize;
use std::task::Poll;
use std::time::Duration;

use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, CloseReason, Control,
    DialOutcome, RadioMode, ScanningMode, BLE_HW_MTU, CONTROL_MAX_LEN,
};

use super::*;
use crate::{
    Reachability, SimulationDurationInTicks, SimulationTick, TopologyConfig, TopologyMutation,
};

const MAX_PEERS: usize = 1;
const FIRST: BleAddress = BleAddress::new([1; 6]);
const SECOND: BleAddress = BleAddress::new([2; 6]);
const FRAME_BYTES: usize = 64;
const OLD_BYTES: [u8; FRAME_BYTES] = [0x31; FRAME_BYTES];
const NEW_BYTES: [u8; FRAME_BYTES] = [0xA6; FRAME_BYTES];
const CANARY: u8 = 0xCC;

#[derive(Clone, Copy)]
enum Rebuilt {
    Dialer,
    Listener,
}

impl Rebuilt {
    fn index(self) -> usize {
        match self {
            Self::Dialer => 0,
            Self::Listener => 1,
        }
    }
}

struct Pair {
    lab: VirtualBleLab,
    backends: Vec<VirtualBleBackend>,
}

impl Pair {
    fn config(index: usize) -> Result<VirtualBleBackendConfig, Box<dyn Error>> {
        Ok(VirtualBleBackendConfig::new(
            [FIRST, SECOND][index],
            -40,
            BleRoleCapabilities::DualRole,
            SimulationDurationInTicks::from_ticks(1),
            VirtualBleBackendLimits {
                inbound_links: NonZeroUsize::MIN,
                connections: NonZeroUsize::MIN,
                discovered_peers: NonZeroUsize::MIN,
            },
            VirtualBleLinkConfig::new(
                1,
                1,
                BLE_HW_MTU,
                VirtualGattConfig::new(CONTROL_MAX_LEN, 20)?,
            )?,
        )?)
    }

    async fn power(&mut self, index: usize) -> Result<(), VirtualBleError> {
        let backend = &mut self.backends[index];
        BleBackend::<MAX_PEERS>::set_radio_mode(backend, RadioMode::On).await?;
        match index {
            0 => BleBackend::<MAX_PEERS>::set_scanning(backend, ScanningMode::On).await,
            1 => BleBackend::<MAX_PEERS>::set_advertising(backend, AdvertisingMode::On).await,
            _ => unreachable!("two-radio fixture"),
        }
    }

    async fn new() -> Result<Self, Box<dyn Error>> {
        let lab = VirtualBleLab::new(BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: NonZeroUsize::MIN,
            },
            2,
            2,
            2,
            128,
        )?);
        let backends = (0..2)
            .map(|index| Ok(lab.attach_backend(Self::config(index)?)?))
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
        let mut pair = Self { lab, backends };
        pair.power(0).await?;
        pair.power(1).await?;
        pair.discover().await?;
        Ok(pair)
    }

    async fn discover(&mut self) -> Result<(), Box<dyn Error>> {
        assert_eq!(
            self.lab
                .set_reachability(FIRST, SECOND, Reachability::Reachable)?,
            TopologyMutation::Applied
        );
        self.lab
            .advance_to_next_event(SimulationTick::from_ticks(self.lab.now().get() + 1))?;
        assert!(matches!(
            BleBackend::<MAX_PEERS>::next_event(&mut self.backends[0]).await,
            BleEvent::Sighting {
                address: SECOND,
                ..
            }
        ));
        Ok(())
    }

    async fn connect(&mut self) -> (VirtualBleLink, VirtualBleLink) {
        assert_eq!(
            BleBackend::<MAX_PEERS>::dial(&mut self.backends[0], SECOND).await,
            DialOutcome::Started
        );
        let BleEvent::LinkReady { link: first, .. } =
            BleBackend::<MAX_PEERS>::next_event(&mut self.backends[0]).await
        else {
            unreachable!("dialer receives new connection")
        };
        let BleEvent::LinkReady { link: second, .. } =
            BleBackend::<MAX_PEERS>::next_event(&mut self.backends[1]).await
        else {
            unreachable!("listener receives new connection")
        };
        assert_eq!(self.lab.active_connection_count(), 1);
        (first, second)
    }

    async fn rebuild(&mut self, rebuilt: Rebuilt) -> Result<(), Box<dyn Error>> {
        self.replace(rebuilt).await?;
        self.discover().await
    }

    async fn replace(&mut self, rebuilt: Rebuilt) -> Result<(), Box<dyn Error>> {
        let index = rebuilt.index();
        drop(self.backends.remove(index));
        assert_eq!(self.lab.active_connection_count(), 0);
        self.backends
            .insert(index, self.lab.attach_backend(Self::config(index)?)?);
        self.power(index).await?;
        // An empty or old-incarnation sighting cannot authorize a dial to the
        // same address's replacement; the new advertisement is required.
        assert_eq!(
            BleBackend::<MAX_PEERS>::dial(&mut self.backends[0], SECOND).await,
            DialOutcome::UnknownPeer
        );
        Ok(())
    }

    fn finish(self) {
        let Self { lab, backends } = self;
        drop(backends);
        assert_eq!(lab.active_connection_count(), 0);
        let trace = lab.trace();
        assert_eq!(trace.discarded_events, 0);
        let mut attached = Vec::new();
        let mut detached = Vec::new();
        for event in trace.events {
            match event {
                BleSimulationEvent::RadioAttached { radio } => attached.push(radio),
                BleSimulationEvent::RadioDetached { radio } => detached.push(radio),
                BleSimulationEvent::ObservationDropped { .. } => unreachable!("no discovery loss"),
                _ => {}
            }
        }
        attached.sort();
        detached.sort();
        assert_eq!(attached.len(), 3);
        assert_eq!(detached, attached);
    }
}

async fn exchange(sink: &mut VirtualBleSink, source: &mut VirtualBleSource) {
    let mut output = [CANARY; FRAME_BYTES + 1];
    let (sent, received) =
        tokio::join!(sink.send_frame(&NEW_BYTES), source.recv_frame(&mut output));
    assert_eq!(sent, Ok(()));
    assert_eq!(received, Ok(FRAME_BYTES));
    let mut expected = [CANARY; FRAME_BYTES + 1];
    expected[..FRAME_BYTES].copy_from_slice(&NEW_BYTES);
    assert_eq!(output, expected);
}

#[tokio::test(start_paused = true)]
async fn backend_restart_requires_a_fresh_sighting_even_when_an_old_one_is_queued(
) -> Result<(), Box<dyn Error>> {
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut pair = Pair::new().await?;
        let old_links = pair.connect().await;
        let before = pair.backends[0].discovery_snapshot();
        let emitted = pair
            .lab
            .advance_by(SimulationDurationInTicks::from_ticks(1))?;
        assert_eq!(emitted.observations_queued, 1);
        pair.replace(Rebuilt::Listener).await?;
        let fresh_radio = pair
            .lab
            .trace()
            .events
            .iter()
            .rev()
            .find_map(|event| match event {
                BleSimulationEvent::RadioAttached { radio } => Some(*radio),
                _ => None,
            })
            .unwrap_or_else(|| unreachable!("replacement attachment recorded"));
        assert_ne!(before.peers[0].radio, fresh_radio);
        assert_eq!(
            pair.lab
                .set_reachability(FIRST, SECOND, Reachability::Reachable)?,
            TopologyMutation::Applied
        );
        let emitted = pair.lab.advance_to_next_event(pair.lab.now())?;
        assert_eq!(emitted.observations_queued, 1);
        assert!(matches!(
            BleBackend::<MAX_PEERS>::next_event(&mut pair.backends[0]).await,
            BleEvent::Sighting {
                address: SECOND,
                ..
            }
        ));
        assert_eq!(pair.backends[0].discovery_snapshot(), before);
        assert_eq!(
            BleBackend::<MAX_PEERS>::dial(&mut pair.backends[0], SECOND).await,
            DialOutcome::UnknownPeer
        );
        assert!(matches!(
            BleBackend::<MAX_PEERS>::next_event(&mut pair.backends[0]).await,
            BleEvent::Sighting {
                address: SECOND,
                ..
            }
        ));
        assert_eq!(
            pair.backends[0].discovery_snapshot(),
            BleDiscoverySnapshot {
                capacity: NonZeroUsize::MIN,
                peers: vec![BleDiscoveredPeer {
                    address: SECOND,
                    radio: fresh_radio
                }],
                evicted_peers: 0,
            }
        );
        let (first, second) = pair.connect().await;
        drop(old_links);
        assert_eq!(pair.lab.active_connection_count(), 1);
        let (_first_source, mut sink) = first.into_data();
        let (mut source, _second_sink) = second.into_data();
        exchange(&mut sink, &mut source).await;
        pair.finish();
        Ok::<_, Box<dyn Error>>(())
    })
    .await?
}

#[tokio::test(start_paused = true)]
async fn backend_restart_rejects_queued_control_and_wakes_its_blocked_sender(
) -> Result<(), Box<dyn Error>> {
    tokio::time::timeout(Duration::from_secs(1), async {
        for rebuilt in [Rebuilt::Dialer, Rebuilt::Listener] {
            let mut pair = Pair::new().await?;
            let (mut old_first, mut old_second) = pair.connect().await;
            let control = Control::Close {
                reason: CloseReason::Incompatible,
            };
            old_first.control_send(&control).await?;
            let mut blocked = Box::pin(old_first.control_send(&control));
            poll_fn(|cx| {
                assert!(blocked.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            pair.rebuild(rebuilt).await?;
            assert_eq!(blocked.await, Err(VirtualBleError::LinkClosed));
            assert_eq!(
                old_second.control_recv().await,
                Err(VirtualBleError::LinkClosed)
            );
            let (mut first, mut second) = pair.connect().await;
            first.control_send(&control).await?;
            assert_eq!(second.control_recv().await, Ok(control));
            drop((old_first, old_second));
            assert_eq!(pair.lab.active_connection_count(), 1);
            let (_first_source, mut sink) = first.into_data();
            let (mut source, _second_sink) = second.into_data();
            exchange(&mut sink, &mut source).await;
            pair.finish();
        }
        Ok::<_, Box<dyn Error>>(())
    })
    .await?
}

#[tokio::test(start_paused = true)]
async fn backend_restart_discards_a_queued_whole_frame_without_retargeting_it(
) -> Result<(), Box<dyn Error>> {
    tokio::time::timeout(Duration::from_secs(1), async {
        for rebuilt in [Rebuilt::Dialer, Rebuilt::Listener] {
            let mut pair = Pair::new().await?;
            let (first, second) = pair.connect().await;
            let (old_first_source, mut old_sink) = first.into_data();
            let (mut old_source, old_second_sink) = second.into_data();
            old_sink.send_frame(b"queued").await?;
            pair.rebuild(rebuilt).await?;
            let mut untouched = [CANARY; FRAME_BYTES];
            assert_eq!(
                old_source.recv_frame(&mut untouched).await,
                Err(VirtualBleError::LinkClosed)
            );
            assert_eq!(untouched, [CANARY; FRAME_BYTES]);
            assert_eq!(
                old_sink.send_frame(b"stale").await,
                Err(VirtualBleError::LinkClosed)
            );
            let (first, second) = pair.connect().await;
            let (_first_source, mut sink) = first.into_data();
            let (mut source, _second_sink) = second.into_data();
            exchange(&mut sink, &mut source).await;
            drop((old_first_source, old_sink, old_source, old_second_sink));
            assert_eq!(pair.lab.active_connection_count(), 1);
            exchange(&mut sink, &mut source).await;
            pair.finish();
        }
        Ok::<_, Box<dyn Error>>(())
    })
    .await?
}

#[tokio::test(start_paused = true)]
async fn backend_restart_abandons_partial_reassembly_without_poisoning_the_new_connection(
) -> Result<(), Box<dyn Error>> {
    tokio::time::timeout(Duration::from_secs(1), async {
        for rebuilt in [Rebuilt::Dialer, Rebuilt::Listener] {
            let mut pair = Pair::new().await?;
            let (first, second) = pair.connect().await;
            let (old_first_source, mut old_sink) = first.into_data();
            let (mut old_source, old_second_sink) = second.into_data();
            let mut output = [CANARY; FRAME_BYTES];
            let mut send = Box::pin(old_sink.send_frame(&OLD_BYTES));
            let mut receive = Box::pin(old_source.recv_frame(&mut output));
            // One fragment slot forces the sender to stop. Polling the receiver
            // consumes a prefix but cannot publish the incomplete frame.
            for _ in 0..2 {
                poll_fn(|cx| {
                    assert!(send.as_mut().poll(cx).is_pending());
                    assert!(receive.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
            }
            pair.rebuild(rebuilt).await?;
            assert_eq!(send.await, Err(VirtualBleError::LinkClosed));
            assert_eq!(receive.await, Err(VirtualBleError::LinkClosed));
            assert_eq!(output, [CANARY; FRAME_BYTES]);
            let (first, second) = pair.connect().await;
            let (mut first_source, mut first_sink) = first.into_data();
            let (mut second_source, mut second_sink) = second.into_data();
            exchange(&mut first_sink, &mut second_source).await;
            drop((old_first_source, old_sink, old_source, old_second_sink));
            assert_eq!(pair.lab.active_connection_count(), 1);
            exchange(&mut second_sink, &mut first_source).await;
            pair.finish();
        }
        Ok::<_, Box<dyn Error>>(())
    })
    .await?
}
