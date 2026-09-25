use std::error::Error;
use std::future::{poll_fn, Future};
use std::task::Poll;

use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, CloseReason, Control,
    DialOutcome, RadioMode, ScanningMode,
};

use super::*;
use crate::{SimulationDurationInTicks, SimulationTick};

const MAX_PEERS: usize = 4;
const SECOND_ADDRESS: BleAddress = BleAddress::new([2; 6]);

struct Pair {
    lab: VirtualBleLab,
    first: VirtualBleBackend,
    second: VirtualBleBackend,
}

impl Pair {
    async fn new(first_capacity: usize, second_capacity: usize) -> Result<Self, Box<dyn Error>> {
        let lab = VirtualBleLab::new(BleMediumConfig::new(2, 4, 4, 32)?);
        let config = |address, capacity| {
            VirtualBleBackendConfig::new(
                BleAddress::new([address; 6]),
                -40,
                BleRoleCapabilities::DualRole,
                SimulationDurationInTicks::from_ticks(1),
                1,
                capacity,
                VirtualBleLinkConfig::new(1, 1, 8)?,
            )
        };
        let mut first = lab.attach_backend(config(1, first_capacity)?)?;
        let mut second = lab.attach_backend(config(2, second_capacity)?)?;
        for backend in [&mut first, &mut second] {
            BleBackend::<MAX_PEERS>::set_radio_mode(backend, RadioMode::On).await?;
            BleBackend::<MAX_PEERS>::set_advertising(backend, AdvertisingMode::On).await?;
        }
        BleBackend::<MAX_PEERS>::set_scanning(&mut first, ScanningMode::On).await?;
        lab.advance_to(SimulationTick::ZERO)?;
        assert!(
            matches!(BleBackend::<MAX_PEERS>::next_event(&mut first).await,
            BleEvent::Sighting { address, .. } if address == SECOND_ADDRESS)
        );
        Ok(Self { lab, first, second })
    }

    async fn dial(&mut self) -> DialOutcome {
        BleBackend::<MAX_PEERS>::dial(&mut self.first, SECOND_ADDRESS).await
    }

    async fn connect(&mut self) -> (VirtualBleLink, VirtualBleLink) {
        assert_eq!(self.dial().await, DialOutcome::Started);
        let BleEvent::LinkReady { link: first, .. } =
            BleBackend::<MAX_PEERS>::next_event(&mut self.first).await
        else {
            unreachable!("dialer receives a link")
        };
        let BleEvent::LinkReady { link: second, .. } =
            BleBackend::<MAX_PEERS>::next_event(&mut self.second).await
        else {
            unreachable!("listener receives a link")
        };
        (first, second)
    }
}

#[tokio::test]
async fn new_connections_require_current_advertising_and_power() -> Result<(), Box<dyn Error>> {
    let mut pair = Pair::new(1, 1).await?;
    BleBackend::<MAX_PEERS>::set_advertising(&mut pair.second, AdvertisingMode::Off).await?;
    assert_eq!(pair.dial().await, DialOutcome::UnknownPeer);
    BleBackend::<MAX_PEERS>::set_advertising(&mut pair.second, AdvertisingMode::On).await?;
    BleBackend::<MAX_PEERS>::set_radio_mode(&mut pair.second, RadioMode::Off).await?;
    assert_eq!(pair.dial().await, DialOutcome::UnknownPeer);
    BleBackend::<MAX_PEERS>::set_radio_mode(&mut pair.second, RadioMode::On).await?;
    let (_first, _second) = pair.connect().await;
    assert_eq!(pair.lab.active_connection_count(), 1);
    Ok(())
}

#[tokio::test]
async fn both_radios_enforce_capacity_and_reclaim_departed_links() -> Result<(), Box<dyn Error>> {
    for (first_capacity, second_capacity) in [(1, 2), (2, 1)] {
        let mut pair = Pair::new(first_capacity, second_capacity).await?;
        let mut retired = Vec::new();
        for _ in 0..32 {
            let (mut first, second) = pair.connect().await;
            assert_eq!(pair.dial().await, DialOutcome::Busy);
            assert_eq!(pair.lab.active_connection_count(), 1);
            drop(second);
            assert_eq!(pair.lab.active_connection_count(), 0);
            assert_eq!(first.control_recv().await, Err(VirtualBleError::LinkClosed));
            retired.push(first);
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_full_inbound_queue_does_not_reserve_an_extra_connection() -> Result<(), Box<dyn Error>> {
    let mut pair = Pair::new(3, 3).await?;
    assert_eq!(pair.dial().await, DialOutcome::Started);
    let first = BleBackend::<MAX_PEERS>::next_event(&mut pair.first).await;
    assert_eq!(pair.dial().await, DialOutcome::Busy);
    assert_eq!(pair.lab.active_connection_count(), 1);
    let second = BleBackend::<MAX_PEERS>::next_event(&mut pair.second).await;
    let (_next_first, _next_second) = pair.connect().await;
    assert_eq!(pair.lab.active_connection_count(), 2);
    drop((first, second));
    assert_eq!(pair.lab.active_connection_count(), 1);
    Ok(())
}

#[tokio::test]
async fn last_data_half_drop_closes_the_peer_and_wakes_a_blocked_send() -> Result<(), Box<dyn Error>>
{
    let mut pair = Pair::new(1, 1).await?;
    let (first, second) = pair.connect().await;
    let (_first_source, mut first_sink) = first.into_data();
    let (second_source, second_sink) = second.into_data();
    drop(second_sink);
    assert_eq!(pair.lab.active_connection_count(), 1);
    first_sink.send_frame(b"queued").await?;
    let mut blocked = std::pin::pin!(first_sink.send_frame(b"blocked"));
    poll_fn(|cx| {
        assert!(blocked.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(second_source);
    assert_eq!(blocked.await, Err(VirtualBleError::LinkClosed));
    assert_eq!(pair.lab.active_connection_count(), 0);
    Ok(())
}

#[tokio::test]
async fn forced_disconnect_wakes_a_full_control_queue() -> Result<(), Box<dyn Error>> {
    let mut pair = Pair::new(1, 1).await?;
    let (mut first, _second) = pair.connect().await;
    let message = Control::Close {
        reason: CloseReason::Incompatible,
    };
    first.control_send(&message).await?;
    let mut blocked = std::pin::pin!(first.control_send(&message));
    poll_fn(|cx| {
        assert!(blocked.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(
        pair.lab.disconnect_radio(SECOND_ADDRESS),
        VirtualBleDisconnectReport {
            connections_closed: 1
        }
    );
    assert_eq!(blocked.await, Err(VirtualBleError::LinkClosed));
    Ok(())
}

#[tokio::test]
async fn disconnect_before_link_ready_reports_dial_failure() -> Result<(), Box<dyn Error>> {
    let mut pair = Pair::new(1, 1).await?;
    assert_eq!(pair.dial().await, DialOutcome::Started);
    let _ = pair.lab.disconnect_radio(SECOND_ADDRESS);
    assert!(
        matches!(BleBackend::<MAX_PEERS>::next_event(&mut pair.first).await,
        BleEvent::DialFailed { address } if address == SECOND_ADDRESS)
    );
    assert_eq!(pair.lab.active_connection_count(), 0);
    Ok(())
}

#[tokio::test]
async fn radio_shutdown_wakes_control_receive_and_discards_queued_data(
) -> Result<(), Box<dyn Error>> {
    let mut pair = Pair::new(1, 1).await?;
    let (mut first, second) = pair.connect().await;
    let (received, stopped) = tokio::join!(first.control_recv(), async {
        tokio::task::yield_now().await;
        BleBackend::<MAX_PEERS>::set_radio_mode(&mut pair.second, RadioMode::Off).await
    });
    stopped?;
    assert_eq!(received, Err(VirtualBleError::LinkClosed));
    assert_eq!(pair.lab.active_connection_count(), 0);
    drop((first, second));
    BleBackend::<MAX_PEERS>::set_radio_mode(&mut pair.second, RadioMode::On).await?;
    let (first, second) = pair.connect().await;
    let (_source, mut sink) = first.into_data();
    let (mut source, _sink) = second.into_data();
    sink.send_frame(b"stale").await?;
    assert_eq!(
        pair.lab.disconnect_radio(SECOND_ADDRESS),
        VirtualBleDisconnectReport {
            connections_closed: 1
        }
    );
    assert_eq!(
        source.recv_frame(&mut [0; 8]).await,
        Err(VirtualBleError::LinkClosed)
    );
    assert_eq!(
        pair.lab.disconnect_radio(SECOND_ADDRESS),
        VirtualBleDisconnectReport {
            connections_closed: 0
        }
    );
    Ok(())
}
