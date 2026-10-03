use std::collections::HashMap;

use tokio::sync::oneshot;

use super::data_plane::{DataPlane, PendingL2cap};
use super::peripheral::{
    can_arm_l2cap, can_open_inbound, l2cap_delivery_admission, reap_closed_sessions,
    reap_stale_pending_l2cap, L2capDeliveryAdmission,
};
use super::CoreBluetoothPeerId;

fn peer_id(prefix: u8, suffix: u8) -> CoreBluetoothPeerId {
    let mut bytes = [prefix; 16];
    bytes[15] = suffix;
    CoreBluetoothPeerId(bytes)
}

#[test]
fn listener_cleanup_removes_only_closed_exact_peer_ids() {
    let closed = peer_id(0x5a, 1);
    let colliding_live = peer_id(0x5a, 2);
    let unrelated_closed = peer_id(0x6b, 1);
    assert_eq!(closed.address(), colliding_live.address());

    let mut sessions = HashMap::from([
        (closed, true),
        (colliding_live, false),
        (unrelated_closed, true),
    ]);
    let mut pending = HashMap::from([
        (closed, "closed"),
        (colliding_live, "live"),
        (unrelated_closed, "unrelated"),
    ]);

    assert_eq!(
        reap_closed_sessions(
            &mut sessions,
            &mut pending,
            Some(closed.address()),
            |closed| *closed,
        ),
        1
    );
    assert!(!sessions.contains_key(&closed));
    assert!(!pending.contains_key(&closed));
    assert!(sessions.contains_key(&colliding_live));
    assert!(pending.contains_key(&colliding_live));
    assert!(sessions.contains_key(&unrelated_closed));
    assert!(pending.contains_key(&unrelated_closed));
}

#[test]
fn global_reap_reclaims_session_capacity_across_disable_cycles() {
    let mut sessions = HashMap::new();
    let mut pending = HashMap::new();

    for suffix in 0..8 {
        let peer_id = peer_id(suffix, suffix);
        sessions.insert(peer_id, true);
        pending.insert(peer_id, ());
        assert!(!can_open_inbound(sessions.len(), 1));
        assert_eq!(
            reap_closed_sessions(&mut sessions, &mut pending, None, |closed| *closed),
            1
        );
        assert!(can_open_inbound(sessions.len(), 1));
        assert!(pending.is_empty());
    }
}

#[test]
fn stale_l2cap_waiters_are_reaped_before_map_admission() {
    let stale_peer = peer_id(1, 1);
    let next_peer = peer_id(2, 2);
    let (tx, rx) = oneshot::channel::<DataPlane>();
    let mut stale = PendingL2cap::default();
    assert!(stale.arm(tx));
    drop(rx);

    let mut pending = HashMap::from([(stale_peer, stale)]);
    assert!(!can_arm_l2cap(&pending, next_peer, 1));
    assert_eq!(reap_stale_pending_l2cap(&mut pending), 1);
    assert!(can_arm_l2cap(&pending, next_peer, 1));
}

#[test]
fn closed_waiters_reclaim_exact_peer_waiter_capacity() {
    let mut pending = PendingL2cap::default();
    let mut receivers = Vec::new();
    for _ in 0..4 {
        let (tx, rx) = oneshot::channel::<DataPlane>();
        assert!(pending.arm(tx));
        receivers.push(rx);
    }
    let (overflow_tx, overflow_rx) = oneshot::channel::<DataPlane>();
    assert!(!pending.arm(overflow_tx));
    assert!(overflow_rx.blocking_recv().is_err());

    drop(receivers);
    let (replacement_tx, _replacement_rx) = oneshot::channel::<DataPlane>();
    assert!(pending.arm(replacement_tx));
    assert_eq!(pending.waiter_len(), 1);
}

#[test]
fn l2cap_admission_uses_full_peer_id_not_synthetic_address() {
    let listener = peer_id(0x5a, 1);
    let armed_collision = peer_id(0x5a, 2);
    let unknown_collision = peer_id(0x5a, 3);
    assert_eq!(listener.address(), armed_collision.address());
    assert_eq!(listener.address(), unknown_collision.address());

    let sessions = HashMap::from([(listener, ())]);
    let pending = HashMap::from([(armed_collision, ())]);
    assert_eq!(
        l2cap_delivery_admission(&sessions, &pending, armed_collision, 1),
        L2capDeliveryAdmission::Existing
    );
    assert_eq!(
        l2cap_delivery_admission(&sessions, &pending, listener, 1),
        L2capDeliveryAdmission::Full
    );
    assert_eq!(
        l2cap_delivery_admission(&sessions, &pending, unknown_collision, 2),
        L2capDeliveryAdmission::Unknown
    );
}
