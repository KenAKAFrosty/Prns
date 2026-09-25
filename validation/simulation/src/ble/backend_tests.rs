use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, AppleHost, BleBackend, BleEvent, BleIdentity, BleLink, BleSink, BleSource,
    BlueZHost, DialOutcome, DiscoveryGroupSet, Endpoint, Handshake, HandshakeOutcome,
    HandshakeRole, LinkCapabilities, LocalPeer, Origin, RadioMode, ScanningMode, BLE_HW_MTU,
    CONTROL_MAX_LEN,
};

use super::*;
use crate::{SimulationDurationInTicks, SimulationTick, TopologyConfig};

const MAX_PEERS: usize = 4;

fn gatt_config() -> VirtualGattConfig {
    VirtualGattConfig::new(CONTROL_MAX_LEN, 20)
        .unwrap_or_else(|error| unreachable!("test GATT limits are valid: {error}"))
}

fn lab(radios: usize) -> VirtualBleLab {
    let config = BleMediumConfig::new(TopologyConfig::FullyConnected, radios, 8, 32, 128)
        .unwrap_or_else(|error| unreachable!("test medium is valid: {error}"));
    VirtualBleLab::new(config)
}

fn backend_config(
    address_byte: u8,
    received_signal_strength_dbm: i8,
    maximum_frame_length: usize,
) -> VirtualBleBackendConfig {
    let link = VirtualBleLinkConfig::new(4, 4, maximum_frame_length, gatt_config())
        .unwrap_or_else(|error| unreachable!("test link is valid: {error}"));
    VirtualBleBackendConfig::new(
        BleAddress::new([address_byte; 6]),
        received_signal_strength_dbm,
        BleRoleCapabilities::DualRole,
        SimulationDurationInTicks::from_ticks(2),
        2,
        MAX_PEERS,
        link,
    )
    .unwrap_or_else(|error| unreachable!("test backend is valid: {error}"))
}

async fn set_radio(backend: &mut VirtualBleBackend, mode: RadioMode) {
    BleBackend::<MAX_PEERS>::set_radio_mode(backend, mode)
        .await
        .unwrap_or_else(|error| unreachable!("radio transition succeeds: {error}"));
}

async fn set_advertising(backend: &mut VirtualBleBackend, mode: AdvertisingMode) {
    BleBackend::<MAX_PEERS>::set_advertising(backend, mode)
        .await
        .unwrap_or_else(|error| unreachable!("advertising transition succeeds: {error}"));
}

async fn set_scanning(backend: &mut VirtualBleBackend, mode: ScanningMode) {
    BleBackend::<MAX_PEERS>::set_scanning(backend, mode)
        .await
        .unwrap_or_else(|error| unreachable!("scanning transition succeeds: {error}"));
}

#[test]
fn backend_configuration_rejects_each_zero_capacity_as_a_whole_value() {
    for (capacities, expected) in [
        ((0, 1, 1), VirtualBleBackendConfigError::ZeroControlCapacity),
        (
            (1, 0, 1),
            VirtualBleBackendConfigError::ZeroDataFragmentCapacity,
        ),
        (
            (1, 1, 0),
            VirtualBleBackendConfigError::ZeroMaximumFrameLength,
        ),
    ] {
        assert_eq!(
            VirtualBleLinkConfig::new(capacities.0, capacities.1, capacities.2, gatt_config()),
            Err(expected),
        );
    }
    assert_eq!(
        VirtualBleLinkConfig::new(1, 1, BLE_HW_MTU + 1, gatt_config()),
        Err(VirtualBleBackendConfigError::FrameLimitTooLarge {
            requested: BLE_HW_MTU + 1,
            maximum: BLE_HW_MTU
        })
    );
    let link = VirtualBleLinkConfig::new(1, 1, 1, gatt_config())
        .unwrap_or_else(|error| unreachable!("test link is valid: {error}"));
    assert_eq!(
        VirtualBleBackendConfig::new(
            BleAddress::new([1; 6]),
            -40,
            BleRoleCapabilities::DualRole,
            SimulationDurationInTicks::from_ticks(1),
            0,
            MAX_PEERS,
            link,
        ),
        Err(VirtualBleBackendConfigError::ZeroInboundLinkCapacity),
    );
    assert_eq!(
        VirtualBleBackendConfig::new(
            BleAddress::new([1; 6]),
            -40,
            BleRoleCapabilities::DualRole,
            SimulationDurationInTicks::ZERO,
            1,
            MAX_PEERS,
            link,
        ),
        Err(VirtualBleBackendConfigError::ZeroAdvertisingInterval),
    );
    assert_eq!(
        VirtualBleBackendConfig::new(
            BleAddress::new([1; 6]),
            -40,
            BleRoleCapabilities::DualRole,
            SimulationDurationInTicks::from_ticks(1),
            1,
            0,
            link
        ),
        Err(VirtualBleBackendConfigError::ZeroConnectionCapacity),
    );
}

#[tokio::test]
async fn production_backend_modes_drive_wakeable_discovery() {
    let lab = lab(2);
    let mut advertiser = lab
        .attach_backend(backend_config(1, -41, 8))
        .unwrap_or_else(|error| unreachable!("advertiser attaches: {error}"));
    let mut scanner = lab
        .attach_backend(backend_config(2, -52, 8))
        .unwrap_or_else(|error| unreachable!("scanner attaches: {error}"));
    set_radio(&mut advertiser, RadioMode::On).await;
    set_radio(&mut scanner, RadioMode::On).await;
    set_advertising(&mut advertiser, AdvertisingMode::On).await;
    set_scanning(&mut scanner, ScanningMode::On).await;

    let (event, report) = tokio::join!(BleBackend::<MAX_PEERS>::next_event(&mut scanner), async {
        tokio::task::yield_now().await;
        lab.advance_to(SimulationTick::ZERO)
            .unwrap_or_else(|error| unreachable!("one emission fits: {error}"))
    },);
    assert_eq!(report.advertisements_emitted, 1);
    assert_eq!(report.observations_queued, 1);
    assert!(matches!(
        event,
        BleEvent::Sighting {
            address,
            rssi: Some(-41),
        } if address == BleAddress::new([1; 6])
    ));
}

#[tokio::test]
async fn discovery_dial_control_and_data_use_the_production_traits() {
    let lab = lab(2);
    let mut first = lab
        .attach_backend(backend_config(1, -41, 8))
        .unwrap_or_else(|error| unreachable!("first backend attaches: {error}"));
    let mut second = lab
        .attach_backend(backend_config(2, -52, 16))
        .unwrap_or_else(|error| unreachable!("second backend attaches: {error}"));
    for backend in [&mut first, &mut second] {
        set_radio(backend, RadioMode::On).await;
        set_advertising(backend, AdvertisingMode::On).await;
        set_scanning(backend, ScanningMode::On).await;
    }
    let report = lab
        .advance_to(SimulationTick::ZERO)
        .unwrap_or_else(|error| unreachable!("two emissions fit: {error}"));
    assert_eq!(report.observations_queued, 2);
    assert!(matches!(
        BleBackend::<MAX_PEERS>::next_event(&mut first).await,
        BleEvent::Sighting { address, .. } if address == BleAddress::new([2; 6])
    ));
    assert!(matches!(
        BleBackend::<MAX_PEERS>::next_event(&mut second).await,
        BleEvent::Sighting { address, .. } if address == BleAddress::new([1; 6])
    ));

    assert_eq!(
        BleBackend::<MAX_PEERS>::dial(&mut first, BleAddress::new([2; 6])).await,
        DialOutcome::Started,
    );
    assert_eq!(
        BleBackend::<MAX_PEERS>::dial(&mut first, BleAddress::new([2; 6])).await,
        DialOutcome::Busy,
    );
    let BleEvent::LinkReady {
        mut link,
        origin: Origin::Dialed,
        peer_rssi: Some(-52),
    } = BleBackend::<MAX_PEERS>::next_event(&mut first).await
    else {
        unreachable!("dialer receives its ready link")
    };
    let BleEvent::LinkReady {
        link: mut peer_link,
        origin: Origin::Accepted,
        peer_rssi: Some(-41),
    } = BleBackend::<MAX_PEERS>::next_event(&mut second).await
    else {
        unreachable!("listener receives its ready link")
    };
    assert_eq!(link.address(), BleAddress::new([2; 6]));
    assert_eq!(peer_link.address(), BleAddress::new([1; 6]));

    let first_peer = LocalPeer {
        identity: BleIdentity::new([1; 16]),
        endpoint: Endpoint::CoreBluetooth(AppleHost::MacOs),
        capabilities: LinkCapabilities {
            l2cap: None,
            link_mtu: 8,
        },
        discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
    };
    let second_peer = LocalPeer {
        identity: BleIdentity::new([2; 16]),
        endpoint: Endpoint::BlueZ(BlueZHost::Linux),
        capabilities: LinkCapabilities {
            l2cap: None,
            link_mtu: 8,
        },
        discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
    };
    let (mut dialer, opening) = Handshake::begin(HandshakeRole::Dialer, first_peer, Some(-52));
    let (mut listener, no_opening) =
        Handshake::begin(HandshakeRole::Listener, second_peer, Some(-41));
    assert_eq!(no_opening, None);
    let Some(opening) = opening else {
        unreachable!("dialer produces a greeting")
    };
    link.control_send(&opening)
        .await
        .unwrap_or_else(|error| unreachable!("control send succeeds: {error}"));
    let hello = peer_link
        .control_recv()
        .await
        .unwrap_or_else(|error| unreachable!("listener receives greeting: {error}"));
    let listener_reaction = listener.absorb(second_peer, hello);
    let Some(welcome) = listener_reaction.reply else {
        unreachable!("listener replies to a valid greeting")
    };
    assert!(matches!(
        listener_reaction.outcome,
        HandshakeOutcome::Settled(established)
            if established.identity == first_peer.identity
                && established.peer_rssi == Some(-52)
    ));
    peer_link
        .control_send(&welcome)
        .await
        .unwrap_or_else(|error| unreachable!("welcome send succeeds: {error}"));
    let welcome = link
        .control_recv()
        .await
        .unwrap_or_else(|error| unreachable!("dialer receives welcome: {error}"));
    assert!(matches!(
        dialer.absorb(first_peer, welcome).outcome,
        HandshakeOutcome::Settled(established)
            if established.identity == second_peer.identity
                && established.peer_rssi == Some(-41)
    ));

    let (_first_source, mut first_sink) = link.into_data();
    let (mut second_source, _second_sink) = peer_link.into_data();
    first_sink
        .send_frame(b"personal")
        .await
        .unwrap_or_else(|error| unreachable!("bounded data send succeeds: {error}"));
    let mut received = [0; 8];
    assert_eq!(second_source.recv_frame(&mut received).await, Ok(8));
    assert_eq!(&received, b"personal");
    assert_eq!(
        first_sink.send_frame(&[0; 9]).await,
        Err(VirtualBleError::FrameTooLong {
            length: 9,
            maximum: 8,
        }),
    );
    first_sink
        .send_frame(&[1, 2, 3])
        .await
        .unwrap_or_else(|error| unreachable!("bounded data send succeeds: {error}"));
    assert_eq!(
        second_source.recv_frame(&mut [0; 2]).await,
        Err(VirtualBleError::ReceiveBufferTooSmall {
            frame: 3,
            buffer: 2,
        }),
    );
    assert_eq!(lab.active_connection_count(), 1);
    let mut after_close = [0; 8];
    let (received, disconnected) =
        tokio::join!(second_source.recv_frame(&mut after_close), async {
            tokio::task::yield_now().await;
            lab.disconnect_between(BleAddress::new([1; 6]), BleAddress::new([2; 6]))
        },);
    assert_eq!(
        disconnected,
        VirtualBleDisconnectReport {
            connections_closed: 1,
        },
    );
    assert_eq!(received, Err(VirtualBleError::LinkClosed));
    assert_eq!(
        first_sink.send_frame(&[4]).await,
        Err(VirtualBleError::LinkClosed),
    );
    assert_eq!(lab.active_connection_count(), 0);
    assert_eq!(
        lab.disconnect_between(BleAddress::new([1; 6]), BleAddress::new([2; 6])),
        VirtualBleDisconnectReport {
            connections_closed: 0,
        },
    );
}

#[tokio::test]
async fn dialing_requires_power_and_a_real_sighting() {
    let lab = lab(2);
    let mut first = lab
        .attach_backend(backend_config(1, -41, 8))
        .unwrap_or_else(|error| unreachable!("first backend attaches: {error}"));
    let second_address = BleAddress::new([2; 6]);
    let _second = lab
        .attach_backend(backend_config(2, -52, 8))
        .unwrap_or_else(|error| unreachable!("second backend attaches: {error}"));
    assert_eq!(
        BleBackend::<MAX_PEERS>::local_capabilities(
            &mut first,
            LinkCapabilities {
                l2cap: personal_rns::interfaces::bluetooth_auto::Psm::new(0x80),
                link_mtu: BLE_HW_MTU as u16,
            }
        )
        .await,
        Ok(LinkCapabilities {
            l2cap: None,
            link_mtu: 8
        })
    );
    assert_eq!(
        BleBackend::<MAX_PEERS>::dial(&mut first, second_address).await,
        DialOutcome::RadioOff,
    );
    set_radio(&mut first, RadioMode::On).await;
    assert_eq!(
        BleBackend::<MAX_PEERS>::dial(&mut first, second_address).await,
        DialOutcome::UnknownPeer,
    );
    assert_eq!(
        BleBackend::<MAX_PEERS>::dial(&mut first, BleAddress::new([1; 6])).await,
        DialOutcome::InvariantViolation,
    );
}

#[test]
fn dropping_a_backend_releases_its_address() {
    let lab = lab(1);
    let config = backend_config(1, -41, 8);
    let first = lab
        .attach_backend(config)
        .unwrap_or_else(|error| unreachable!("first backend attaches: {error}"));
    drop(first);
    assert!(lab.attach_backend(config).is_ok());
}
