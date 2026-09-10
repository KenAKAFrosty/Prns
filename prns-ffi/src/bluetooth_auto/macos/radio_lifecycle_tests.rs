use super::backend::BoundedRecentSet;
use super::central::{CentralDialCandidate, CentralPeerRegistry, RestoredAdmission};
use super::CoreBluetoothPeerId;

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
