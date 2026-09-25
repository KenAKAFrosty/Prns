use std::future::{poll_fn, Future};
use std::num::NonZeroUsize;
use std::pin::{pin, Pin};
use std::task::Poll;
use std::time::Duration;

use personal_rns::engine::InstantMillis;
use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, AppleHost, BleAddress, BleBackend, BleEvent, BleIdentity, BleLink,
    BleRoleCapabilities, CloseReason, Control, DiscoveryGroupId, DiscoveryGroupSet, Endpoint,
    Handshake, HandshakeOutcome, HandshakeReaction, HandshakeRole, LinkCapabilities, LocalPeer,
    Origin, RadioMode, BLE_HW_MTU, CONTROL_MAX_LEN, GROUP_MISMATCH_RETRY_TTL_MS,
};
use personal_rns::interfaces::{ConnectionState, InterfaceStatus};
use personal_rns::manifold::tokio::TokioClock;
use personal_rns::runtime::{Fleet, InterfaceSupervisor};
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::ble::{
    BleMediumConfig, VirtualBleBackend, VirtualBleBackendConfig, VirtualBleBackendLimits,
    VirtualBleError, VirtualBleLab, VirtualBleLink, VirtualBleLinkConfig, VirtualGattConfig,
};
use prns_simulation::{SimulationDurationInTicks, SimulationTick, TopologyConfig};

const MAX_PEERS: usize = 4;
const ONE_MILLISECOND: Duration = Duration::from_millis(1);
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);
const REFUSAL_AT_MILLIS: u64 = 37_000;

struct Scenario {
    lab: VirtualBleLab,
    supervisor: BluetoothAuto<VirtualBleBackend, MAX_PEERS>,
    remote: VirtualBleBackend,
}

fn backend_config(address: u8) -> VirtualBleBackendConfig {
    let gatt = VirtualGattConfig::new(CONTROL_MAX_LEN, 20)
        .unwrap_or_else(|error| unreachable!("valid GATT configuration: {error}"));
    let link = VirtualBleLinkConfig::new(4, 4, BLE_HW_MTU, gatt)
        .unwrap_or_else(|error| unreachable!("valid link configuration: {error}"));
    VirtualBleBackendConfig::new(
        BleAddress::new([address; 6]),
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
    .unwrap_or_else(|error| unreachable!("valid backend configuration: {error}"))
}

async fn scenario() -> Scenario {
    let medium = BleMediumConfig::new(TopologyConfig::FullyConnected, 2, 4, 8, 64)
        .unwrap_or_else(|error| unreachable!("valid medium configuration: {error}"));
    let lab = VirtualBleLab::new(medium);
    let backend = lab
        .attach_backend(backend_config(1))
        .unwrap_or_else(|error| unreachable!("supervisor attaches: {error}"));
    let mut remote = lab
        .attach_backend(backend_config(2))
        .unwrap_or_else(|error| unreachable!("remote attaches: {error}"));
    assert!(
        BleBackend::<MAX_PEERS>::set_radio_mode(&mut remote, RadioMode::On)
            .await
            .is_ok()
    );
    assert!(
        BleBackend::<MAX_PEERS>::set_advertising(&mut remote, AdvertisingMode::On)
            .await
            .is_ok()
    );
    let supervisor = BluetoothAuto::new(
        backend,
        BleIdentity::new([1; 16]),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
    );
    Scenario {
        lab,
        supervisor,
        remote,
    }
}

// One explicit poll keeps Tokio from auto-advancing time while a negative assertion waits.
async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|context| Poll::Ready(future.as_mut().poll(context))).await
}

fn emit_sighting(lab: &VirtualBleLab) {
    let report = lab
        .advance_to_next_event(SimulationTick::from_ticks(u64::MAX))
        .unwrap_or_else(|error| unreachable!("one discovery step fits: {error}"));
    assert!(report.observations_queued > 0);
}

async fn accepted_link(remote: &mut VirtualBleBackend) -> VirtualBleLink {
    match poll_once(pin!(BleBackend::<MAX_PEERS>::next_event(remote))).await {
        Poll::Ready(BleEvent::LinkReady {
            link,
            origin: Origin::Accepted,
            ..
        }) => link,
        _ => unreachable!("the supervisor must have dialed the advertising remote"),
    }
}

#[tokio::test(start_paused = true)]
async fn incompatible_group_cooldown_expires_at_the_controlled_runtime_deadline() {
    let Scenario {
        lab,
        supervisor,
        mut remote,
    } = scenario().await;
    let status = supervisor.status();
    let (fleet, _detached) = Fleet::detached(status.id());
    let mut running = pin!(supervisor.run(fleet));
    let clock = TokioClock::start_at(InstantMillis(17));
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    tokio::time::advance(Duration::from_millis(REFUSAL_AT_MILLIS)).await;
    emit_sighting(&lab);
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    let mut link = accepted_link(&mut remote).await;
    let Poll::Ready(Ok(hello)) = poll_once(pin!(link.control_recv())).await else {
        unreachable!("the production supervisor sends its greeting")
    };
    let local = LocalPeer {
        identity: BleIdentity::new([2; 16]),
        endpoint: Endpoint::CoreBluetooth(AppleHost::MacOs),
        capabilities: LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
        discovery_groups: DiscoveryGroupSet::from_singleton(
            DiscoveryGroupId::parse("elsewhere")
                .unwrap_or_else(|error| unreachable!("valid disjoint group: {error:?}")),
        )
        .hashes(),
    };
    let (mut handshake, _) = Handshake::begin(HandshakeRole::Listener, local, None);
    let reaction = handshake.absorb(local, hello);
    assert_eq!(
        reaction,
        HandshakeReaction {
            reply: Some(Control::Close {
                reason: CloseReason::Incompatible
            }),
            outcome: HandshakeOutcome::Aborted(CloseReason::Incompatible),
        }
    );
    let reply = reaction
        .reply
        .unwrap_or_else(|| unreachable!("explicit refusal"));
    assert_eq!(
        poll_once(pin!(link.control_send(&reply))).await,
        Poll::Ready(Ok(()))
    );
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    assert_eq!(
        (lab.active_connection_count(), status.connection()),
        (0, ConnectionState::Disconnected)
    );
    drop(link);

    tokio::time::advance(Duration::from_millis(GROUP_MISMATCH_RETRY_TTL_MS - 1)).await;
    emit_sighting(&lab);
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    assert!(
        poll_once(pin!(BleBackend::<MAX_PEERS>::next_event(&mut remote)))
            .await
            .is_pending()
    );
    assert_eq!(lab.active_connection_count(), 0);
    assert_eq!(
        clock.now(),
        InstantMillis(17 + REFUSAL_AT_MILLIS + GROUP_MISMATCH_RETRY_TTL_MS - 1)
    );

    tokio::time::advance(ONE_MILLISECOND).await;
    emit_sighting(&lab);
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    let retried = accepted_link(&mut remote).await;
    assert_eq!(lab.active_connection_count(), 1);
    assert_eq!(
        clock.now(),
        InstantMillis(17 + REFUSAL_AT_MILLIS + GROUP_MISMATCH_RETRY_TTL_MS)
    );
    drop(retried);
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    assert_eq!(lab.active_connection_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn stalled_handshake_releases_capacity_at_ten_seconds_without_medium_time_passing() {
    let Scenario {
        lab,
        supervisor,
        mut remote,
    } = scenario().await;
    let status = supervisor.status();
    let (fleet, _detached) = Fleet::detached(status.id());
    let mut running = pin!(supervisor.run(fleet));
    let clock = TokioClock::new();
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    emit_sighting(&lab);
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    let mut silent = accepted_link(&mut remote).await;

    tokio::time::advance(HANDSHAKE_DEADLINE - ONE_MILLISECOND).await;
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    assert_eq!(lab.active_connection_count(), 1);

    tokio::time::advance(ONE_MILLISECOND).await;
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    assert_eq!(
        (
            lab.active_connection_count(),
            status.connection(),
            lab.now(),
            clock.now()
        ),
        (
            0,
            ConnectionState::Disconnected,
            SimulationTick::ZERO,
            InstantMillis(10_000)
        )
    );
    assert_eq!(
        poll_once(pin!(silent.control_recv())).await,
        Poll::Ready(Err(VirtualBleError::LinkClosed))
    );
    drop(silent);

    emit_sighting(&lab);
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    let replacement = accepted_link(&mut remote).await;
    assert_eq!(lab.active_connection_count(), 1);
    drop(replacement);
    assert_eq!(poll_once(running.as_mut()).await, Poll::Pending);
    assert_eq!(lab.active_connection_count(), 0);
}
