use std::collections::HashMap;

use tokio::sync::{mpsc, oneshot};

use prns_core::interfaces::bluetooth_auto::{Control, LivenessMode, PeerProtocol};

use super::data_plane::{DataPlane, PendingL2cap};
use super::gatt_link::{gatt_inbound_channel, GattInboundReceiver};
use super::peripheral::{
    can_arm_l2cap, can_open_inbound, capability_read_allowed, l2cap_delivery_admission,
    listener_liveness_mode, reap_closed_sessions, reap_stale_pending_l2cap, L2capDeliveryAdmission,
};
use super::peripheral_notify::SessionPhase;
use super::peripheral_write::WriteSession;
use super::CoreBluetoothPeerId;

fn peer_id(prefix: u8, suffix: u8) -> CoreBluetoothPeerId {
    let mut bytes = [prefix; 16];
    bytes[15] = suffix;
    CoreBluetoothPeerId(bytes)
}

fn native_session() -> (
    WriteSession<()>,
    mpsc::Receiver<Control>,
    GattInboundReceiver,
) {
    let (control_tx, control_rx) = mpsc::channel(8);
    let (data_tx, data_rx) = gatt_inbound_channel();
    (
        WriteSession {
            central: (),
            protocol: PeerProtocol::Native,
            control_tx,
            data_tx,
        },
        control_rx,
        data_rx,
    )
}

#[test]
fn capability_publication_requires_runtime_and_current_native_service() {
    for enabled in [false, true] {
        for published in [false, true] {
            for protocol in [PeerProtocol::Native, PeerProtocol::Columba] {
                assert_eq!(
                    listener_liveness_mode(enabled, published, protocol),
                    if enabled && published && protocol == PeerProtocol::Native {
                        LivenessMode::Listener
                    } else {
                        LivenessMode::Disabled
                    },
                );
            }
            assert_eq!(
                capability_read_allowed::<()>(enabled, published, None),
                enabled && published,
            );
        }
    }
}

#[test]
fn enabling_runtime_never_advertises_support_to_an_existing_legacy_owner() {
    let (legacy, _control, _data) = native_session();
    assert!(!capability_read_allowed(false, true, Some(&legacy)));
    assert!(!capability_read_allowed(true, true, Some(&legacy)));
    let (mut capable, _control, _data) = native_session();
    capable.data_tx.notifications().enable_liveness_listener();
    assert!(capability_read_allowed(true, true, Some(&capable)));
    assert!(
        !capability_read_allowed(true, false, Some(&capable)),
        "restored old services have no e9"
    );
    capable.protocol = PeerProtocol::Columba;
    assert!(!capability_read_allowed(true, true, Some(&capable)));
    capable.protocol = PeerProtocol::Native;
    drop(capable.data_tx.notifications().settled_owner());
    assert!(!capability_read_allowed(true, true, Some(&capable)));
    assert!(!capability_read_allowed(true, true, Some(&legacy)));
}

#[tokio::test]
async fn retired_control_reaps_its_live_data_receiver_and_pending_upgrade_only() {
    let retired_peer = peer_id(0x5a, 1);
    let colliding_peer = peer_id(0x5a, 2);
    assert_eq!(retired_peer.address(), colliding_peer.address());
    let (retired, mut retired_control, mut retired_data) = native_session();
    let owner = retired.data_tx.notifications().settled_owner();
    let (live, mut live_control, _live_data) = native_session();
    let _live_owner = live.data_tx.notifications().settled_owner();
    let (retired_tx, mut retired_upgrade) = oneshot::channel::<DataPlane>();
    let mut retired_pending = PendingL2cap::default();
    assert!(retired_pending.arm(retired_tx));
    let (live_tx, mut live_upgrade) = oneshot::channel::<DataPlane>();
    let mut live_pending = PendingL2cap::default();
    assert!(live_pending.arm(live_tx));
    assert!(!retired.data_receiver_closed());
    drop(owner);
    assert!(
        !retired.data_tx.is_closed(),
        "the old data pump has not exited"
    );
    assert!(
        retired.data_receiver_closed(),
        "retirement is independently terminal"
    );
    let mut sessions = HashMap::from([(retired_peer, retired), (colliding_peer, live)]);
    let mut pending = HashMap::from([
        (retired_peer, retired_pending),
        (colliding_peer, live_pending),
    ]);
    assert_eq!(
        reap_closed_sessions(
            &mut sessions,
            &mut pending,
            Some(retired_peer.address()),
            WriteSession::data_receiver_closed
        ),
        1
    );
    assert!(!sessions.contains_key(&retired_peer));
    assert!(!pending.contains_key(&retired_peer));
    assert!(retired_data.recv().await.is_none());
    assert!(matches!(
        retired_control.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    assert!(matches!(
        retired_upgrade.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(sessions.contains_key(&colliding_peer));
    assert!(pending.contains_key(&colliding_peer));
    assert!(matches!(
        live_control.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        live_upgrade.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
}

#[test]
fn delayed_old_control_drop_cannot_reap_a_fresh_exact_peer_replacement() {
    let peer = peer_id(0x5a, 1);
    let (old, _old_control, _old_data) = native_session();
    let old_session = old.data_tx.notifications().clone();
    let old_owner = old_session.settled_owner();
    let mut sessions = HashMap::from([(peer, old)]);
    let (replacement, mut replacement_control, _replacement_data) = native_session();
    let replacement_session = replacement.data_tx.notifications().clone();
    let _replacement_owner = replacement_session.settled_owner();
    sessions.insert(peer, replacement);
    let (tx, mut upgrade) = oneshot::channel::<DataPlane>();
    let mut waiting = PendingL2cap::default();
    assert!(waiting.arm(tx));
    let mut pending = HashMap::from([(peer, waiting)]);
    drop(old_owner);
    assert_eq!(old_session.phase(), SessionPhase::Retired);
    assert_eq!(replacement_session.phase(), SessionPhase::Settled);
    assert_eq!(
        reap_closed_sessions(
            &mut sessions,
            &mut pending,
            Some(peer.address()),
            WriteSession::data_receiver_closed
        ),
        0
    );
    assert!(sessions.contains_key(&peer));
    assert!(pending.contains_key(&peer));
    assert!(matches!(
        replacement_control.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        upgrade.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
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
