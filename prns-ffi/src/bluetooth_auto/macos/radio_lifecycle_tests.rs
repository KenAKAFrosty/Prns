use super::backend::BoundedRecentSet;
use super::central::{CentralDialCandidate, CentralPeerRegistry, RestoredAdmission};
use super::{manager_signal_channel, CoreBluetoothPeerId, CoreBluetoothRadioStatus};
use objc2_core_bluetooth::CBManagerState;
use prns_core::interfaces::bluetooth_auto::BluetoothRadioState;

#[test]
fn radio_status_requires_both_managers_and_tracks_power_loss_without_new_owners() {
    let (signals, current) = manager_signal_channel();
    let status = CoreBluetoothRadioStatus(current);
    let observer = status.clone();
    assert_eq!(status.state(), BluetoothRadioState::Unknown);

    signals.central_state_changed(CBManagerState::PoweredOn);
    assert_eq!(status.state(), BluetoothRadioState::Unknown);
    signals.peripheral_state_changed(CBManagerState::PoweredOn);
    assert_eq!(observer.state(), BluetoothRadioState::PoweredOn);

    signals.central_state_changed(CBManagerState::PoweredOff);
    assert_eq!(status.state(), BluetoothRadioState::PoweredOff);
    signals.central_state_changed(CBManagerState::PoweredOn);
    signals.peripheral_state_changed(CBManagerState::PoweredOff);
    assert_eq!(observer.state(), BluetoothRadioState::PoweredOff);
    signals.peripheral_state_changed(CBManagerState::PoweredOn);
    assert_eq!(status.state(), BluetoothRadioState::PoweredOn);

    drop(signals);
    assert_eq!(observer.state(), BluetoothRadioState::Unknown);
}

#[test]
fn radio_status_preserves_authoritative_unavailable_and_transition_states() {
    for (native, expected) in [
        (CBManagerState::Unknown, BluetoothRadioState::Unknown),
        (CBManagerState::Resetting, BluetoothRadioState::Resetting),
        (
            CBManagerState::Unsupported,
            BluetoothRadioState::Unsupported,
        ),
        (
            CBManagerState::Unauthorized,
            BluetoothRadioState::Unauthorized,
        ),
        (CBManagerState::PoweredOff, BluetoothRadioState::PoweredOff),
    ] {
        let (signals, current) = manager_signal_channel();
        let status = CoreBluetoothRadioStatus(current);
        signals.central_state_changed(native);
        assert_eq!(status.state(), expected);
        signals.central_state_changed(CBManagerState::PoweredOn);
        signals.peripheral_state_changed(native);
        assert_eq!(status.state(), expected);
    }
}

#[test]
fn startup_readiness_does_not_reuse_a_prior_powered_on_observation() {
    let (signals, current) = manager_signal_channel();
    signals.central_state_changed(CBManagerState::PoweredOn);
    signals.peripheral_state_changed(CBManagerState::PoweredOn);
    signals.gatt_service_published();
    signals.l2cap_published(0x0081);
    assert!(super::backend::manager_readiness(*current.borrow())
        .unwrap()
        .is_some());
    signals.central_state_changed(CBManagerState::PoweredOff);
    assert_eq!(
        super::backend::manager_readiness(*current.borrow()).unwrap(),
        None
    );
}

fn peer_id(prefix: u8, suffix: u8) -> CoreBluetoothPeerId {
    let mut bytes = [prefix; 16];
    bytes[15] = suffix;
    CoreBluetoothPeerId(bytes)
}

fn claim(registry: &mut CentralPeerRegistry<u8>, peer_id: CoreBluetoothPeerId, expected: u8) {
    match registry.claim(peer_id.address()) {
        CentralDialCandidate::Ready {
            peer_id: claimed,
            peripheral,
            restored,
            ..
        } => {
            assert!(claimed == peer_id);
            assert_eq!(peripheral, expected);
            assert!(!restored);
        }
        CentralDialCandidate::Busy | CentralDialCandidate::Missing => {
            panic!("expected a ready dial candidate")
        }
    }
}

#[test]
fn radio_shutdown_drains_only_connections_owned_by_the_central_registry() {
    let observed = peer_id(0x10, 1);
    let dialing = peer_id(0x20, 1);
    let active = peer_id(0x30, 1);
    let restored = peer_id(0x40, 1);
    let mut registry = CentralPeerRegistry::new(4, 1);

    assert!(registry.observe(observed, 10, None));
    assert!(registry.observe(dialing, 20, None));
    claim(&mut registry, dialing, 20);
    assert!(registry.observe(active, 30, None));
    claim(&mut registry, active, 30);
    assert!(matches!(registry.begin_session(active), Ok(None)));
    assert_eq!(
        registry.admit_restored(restored, 40),
        RestoredAdmission::Admitted
    );

    let mut owned = registry.drain_owned();
    owned.sort_unstable();
    assert_eq!(owned, vec![20, 30, 40]);
    assert_eq!(registry.peripheral_len(), 0);
    assert_eq!(registry.restored_len(), 0);
}

#[test]
fn repeated_radio_shutdown_reclaims_registry_and_recent_set_capacity() {
    let mut registry = CentralPeerRegistry::new(1, 1);
    let mut seen = BoundedRecentSet::new(1);

    for cycle in 0..8 {
        let peer_id = peer_id(cycle, cycle);
        assert_eq!(
            registry.admit_restored(peer_id, cycle),
            RestoredAdmission::Admitted
        );
        assert!(seen.insert(peer_id.address()));

        assert_eq!(registry.drain_owned(), vec![cycle]);
        seen.clear();
        assert_eq!(registry.peripheral_len(), 0);
        assert_eq!(registry.restored_len(), 0);
        assert_eq!(seen.len(), 0);
    }
}
