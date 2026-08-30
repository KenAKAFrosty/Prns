use super::{
    AndroidBleBridge, AndroidBleIngressAdmission, RADIO_ADVERTISING, RADIO_ENABLED, RADIO_SCANNING,
};
use prns_core::interfaces::bluetooth_auto::{AdvertisingMode, RadioMode, ScanningMode};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn disabled_radio_exposes_no_android_ble_work() {
    let bridge = AndroidBleBridge::new();

    bridge.set_radio_mode(RadioMode::On);
    bridge.set_advertising(AdvertisingMode::On);
    bridge.set_scanning(ScanningMode::On);
    bridge.set_psm(0x0080);
    assert_eq!(
        bridge.radio_state(),
        RADIO_ENABLED | RADIO_ADVERTISING | RADIO_SCANNING
    );

    bridge.set_radio_mode(RadioMode::Off);

    assert_eq!(bridge.radio_state(), 0);
    assert!(bridge.shared.psm.lock().unwrap().is_none());
    assert!(bridge.shared.links.lock().unwrap().is_empty());
    assert!(bridge.shared.events.lock().unwrap().is_empty());
    assert!(bridge.shared.dial_requests.lock().unwrap().is_empty());
    assert!(bridge.shared.close_requests.lock().unwrap().is_empty());
    assert!(bridge.shared.l2cap_opens.lock().unwrap().is_empty());
}

#[test]
fn set_local_group_tag_wakes_radio_work() {
    let bridge = AndroidBleBridge::new();
    let before = bridge.work_generation();
    bridge.set_local_group_tag([0x11, 0x22, 0x33, 0x44]);
    assert_ne!(bridge.work_generation(), before);
    let after = bridge.work_generation();
    bridge.set_local_group_tag([0x11, 0x22, 0x33, 0x44]);
    assert_eq!(
        bridge.work_generation(),
        after,
        "unchanged discovery group must not bounce advertising"
    );
}

#[test]
fn advertising_or_scanning_without_enabled_stays_invisible() {
    let bridge = AndroidBleBridge::new();

    bridge.set_advertising(AdvertisingMode::On);
    bridge.set_scanning(ScanningMode::On);

    assert_eq!(bridge.radio_state(), 0);
}

#[test]
fn inbound_link_queues_are_bounded() {
    let bridge = AndroidBleBridge::new();
    assert!(bridge.link_up(7, [1, 2, 3, 4, 5, 6], None, true));

    for _ in 0..8 {
        assert_eq!(
            bridge.control_in(7, &[1]),
            AndroidBleIngressAdmission::Accepted
        );
    }
    assert_eq!(bridge.control_in(7, &[1]), AndroidBleIngressAdmission::Full);

    for _ in 0..16 {
        assert_eq!(
            bridge.data_in(7, &[2]),
            AndroidBleIngressAdmission::Accepted
        );
    }
    assert_eq!(bridge.data_in(7, &[2]), AndroidBleIngressAdmission::Full);
    assert_eq!(bridge.ingress_pressure_events(), 2);
}

#[tokio::test]
async fn outbound_messages_remain_owned_until_commit() {
    let bridge = AndroidBleBridge::new();
    assert!(bridge.link_up(9, [1, 2, 3, 4, 5, 7], None, true));
    let (control, data) = {
        let links = bridge.shared.links.lock().unwrap();
        let endpoints = links.get(&9).unwrap().active().unwrap();
        (endpoints.control_out.clone(), endpoints.data_out.clone())
    };
    control.push(vec![vec![1, 2], vec![3, 4, 5]]).await.unwrap();
    data.push(vec![vec![6, 7], vec![8, 9, 10]]).await.unwrap();

    let mut out = [0u8; 8];
    assert_eq!(bridge.control_out(9, &mut out), 2);
    assert_eq!(&out[..2], &[1, 2]);
    assert_eq!(bridge.control_out(9, &mut out), 2);
    assert_eq!(&out[..2], &[1, 2]);
    assert!(bridge.commit_control_out(9));
    assert_eq!(bridge.control_out(9, &mut out), 3);
    assert_eq!(&out[..3], &[3, 4, 5]);

    assert_eq!(bridge.data_out(9, &mut out), 2);
    assert_eq!(&out[..2], &[6, 7]);
    assert_eq!(bridge.data_out(9, &mut out), 2);
    assert_eq!(&out[..2], &[6, 7]);
    assert!(bridge.commit_data_out(9));
    assert_eq!(bridge.data_out(9, &mut out), 3);
    assert_eq!(&out[..3], &[8, 9, 10]);
}

#[tokio::test]
async fn outbound_pressure_waits_for_commit_and_link_closure_wakes_waiters() {
    let bridge = AndroidBleBridge::new();
    assert!(bridge.link_up(10, [1, 2, 3, 4, 5, 8], None, true));
    let data = {
        let links = bridge.shared.links.lock().unwrap();
        links.get(&10).unwrap().active().unwrap().data_out.clone()
    };
    data.push((0..16).map(|byte| vec![byte]).collect())
        .await
        .unwrap();

    let waiting_data = data.clone();
    let mut waiting = tokio::spawn(async move { waiting_data.push(vec![vec![17]]).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiting)
            .await
            .is_err()
    );

    let mut out = [0u8; 8];
    assert_eq!(bridge.data_out(10, &mut out), 1);
    assert!(bridge.commit_data_out(10));
    assert!(tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap()
        .is_ok());

    let waiting_data = data.clone();
    let mut waiting = tokio::spawn(async move { waiting_data.push(vec![vec![18]]).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiting)
            .await
            .is_err()
    );
    bridge.disconnected(10);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap(),
        Err(super::outbound::OutboundQueueError::Closed)
    ));
}

#[tokio::test]
async fn outbound_stream_pressure_waits_for_drain() {
    let queue = std::sync::Arc::new(super::outbound::BoundedByteQueue::new(2));
    queue.push(&[1, 2]).await.unwrap();

    let waiting_queue = queue.clone();
    let mut waiting = tokio::spawn(async move { waiting_queue.push(&[3]).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiting)
            .await
            .is_err()
    );

    let mut out = [0u8; 1];
    assert_eq!(queue.drain(&mut out), 1);
    assert_eq!(out, [1]);
    assert!(tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
    assert_eq!(queue.drain(&mut out), 1);
    assert_eq!(out, [2]);
    assert_eq!(queue.drain(&mut out), 1);
    assert_eq!(out, [3]);
}

#[test]
fn lifecycle_and_dial_storage_are_coalesced_and_bounded() {
    let bridge = AndroidBleBridge::new();
    for rssi in -90..=-40 {
        bridge.sighting([1, 2, 3, 4, 5, 6], Some(rssi));
    }
    assert_eq!(bridge.shared.events.lock().unwrap().len(), 1);

    for suffix in 0..super::bridge::PEER_CAPACITY {
        assert!(bridge.push_dial([1, 2, 3, 4, 5, suffix as u8]));
    }
    assert!(bridge.push_dial([1, 2, 3, 4, 5, 0]));
    assert!(!bridge.push_dial([1, 2, 3, 4, 5, 99]));
    assert_eq!(
        bridge.shared.dial_requests.lock().unwrap().len(),
        super::bridge::PEER_CAPACITY
    );
}

#[test]
fn lifecycle_overflow_rejects_a_link_explicitly() {
    let bridge = AndroidBleBridge::new();
    for suffix in 0..21 {
        assert!(bridge.dial_failed([1, 2, 3, 4, suffix, 0]));
    }

    assert!(!bridge.link_up(77, [6, 5, 4, 3, 2, 1], None, false));
    assert!(!bridge.shared.links.lock().unwrap().contains_key(&77));
}

#[test]
fn policy_closes_stay_bounded_and_owned_until_kotlin_acknowledges() {
    let bridge = AndroidBleBridge::new();
    let address = [1, 2, 3, 4, 5, 6];
    let conn_ids = 1..=super::bridge::PEER_CAPACITY as u32;
    for conn_id in conn_ids.clone() {
        assert!(bridge.link_up(conn_id, address, None, false));
    }

    assert!(bridge.close_by_address(address));
    assert_eq!(
        bridge.shared.close_requests.lock().unwrap().len(),
        super::bridge::PEER_CAPACITY
    );
    assert_eq!(
        bridge.shared.links.lock().unwrap().len(),
        super::bridge::PEER_CAPACITY,
        "closing links must retain their bridge slots"
    );

    assert!(bridge.close_by_address(address));
    assert_eq!(
        bridge.shared.close_requests.lock().unwrap().len(),
        super::bridge::PEER_CAPACITY,
        "repeated policy cleanup must coalesce"
    );
    assert!(!bridge.link_up(99, [6, 5, 4, 3, 2, 1], None, false));

    let mut requested = std::vec::Vec::new();
    while let Some(conn_id) = bridge.next_close() {
        requested.push(conn_id);
    }
    requested.sort_unstable();
    assert_eq!(requested, conn_ids.clone().collect::<std::vec::Vec<_>>());
    assert_eq!(bridge.next_close(), None);
    assert_eq!(
        bridge.shared.links.lock().unwrap().len(),
        super::bridge::PEER_CAPACITY
    );

    for conn_id in conn_ids {
        bridge.disconnected(conn_id);
    }
    assert!(bridge.shared.links.lock().unwrap().is_empty());
    assert!(bridge.link_up(99, [6, 5, 4, 3, 2, 1], None, false));
}

#[test]
fn connection_ids_cannot_be_reused_before_disconnect_acknowledgement() {
    let bridge = AndroidBleBridge::new();
    let address = [1, 2, 3, 4, 5, 6];
    assert!(bridge.link_up(7, address, None, false));
    assert!(bridge.close_by_address(address));

    assert!(!bridge.link_up(7, [6, 5, 4, 3, 2, 1], None, true));
    bridge.disconnected(7);
    assert!(bridge.link_up(7, [6, 5, 4, 3, 2, 1], None, true));
}

#[test]
fn radio_reset_discards_pending_physical_closes() {
    let bridge = AndroidBleBridge::new();
    let address = [1, 2, 3, 4, 5, 6];
    assert!(bridge.link_up(7, address, None, false));
    assert!(bridge.close_by_address(address));
    assert_eq!(bridge.shared.close_requests.lock().unwrap().len(), 1);

    bridge.set_radio_mode(RadioMode::Off);

    assert!(bridge.shared.links.lock().unwrap().is_empty());
    assert!(bridge.shared.close_requests.lock().unwrap().is_empty());
    assert_eq!(bridge.next_close(), None);
}

#[test]
fn l2cap_ingress_blocks_until_bounded_capacity_returns() {
    let bridge = AndroidBleBridge::new();
    assert!(bridge.link_up(11, [1, 2, 3, 4, 5, 8], None, false));
    let mut inbound = {
        let mut events = bridge.shared.events.lock().unwrap();
        match events.pop_front().unwrap() {
            super::bridge::Event::Link(pending) => pending.l2cap_in,
            _ => panic!("expected link event"),
        }
    };
    for _ in 0..16 {
        assert!(bridge.l2cap_in(11, &[1]));
    }

    let (done_tx, done_rx) = mpsc::channel();
    let blocked_bridge = bridge.clone();
    let blocked = std::thread::spawn(move || {
        let _ = done_tx.send(blocked_bridge.l2cap_in(11, &[2]));
    });
    assert!(done_rx.recv_timeout(Duration::from_millis(20)).is_err());
    assert_eq!(inbound.try_recv().unwrap(), vec![1]);
    assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
    blocked.join().unwrap();
}
