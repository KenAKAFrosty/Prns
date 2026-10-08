use super::{
    AndroidBleBridge, AndroidBleIngressAdmission, RADIO_ADVERTISING, RADIO_ENABLED, RADIO_SCANNING,
};
use prns_core::interfaces::bluetooth_auto::{AdvertisingMode, RadioMode, ScanningMode};
use std::sync::mpsc;
use std::time::Duration;

use super::{AndroidBleControlOutput, AndroidBleControlTicket, AndroidBleError};
use prns_core::interfaces::bluetooth_auto::{
    BleBackend, BleControl, BleEvent, CloseReason, Control,
};

async fn control_link(bridge: &AndroidBleBridge, conn_id: u32) -> super::AndroidBleLink {
    let mut backend = super::AndroidBleBackend::new(bridge.clone());
    assert!(bridge.link_up(conn_id, [1; 6], None, false));
    let BleEvent::Inbound(link) = backend.next_event().await else {
        panic!("inbound link")
    };
    link
}

#[tokio::test]
async fn liveness_requires_supervisor_readiness_and_this_physical_attempts_capability() {
    use super::LivenessMode;
    for enabled in [false, true] {
        for supported in [false, true] {
            for dialed in [false, true] {
                let bridge = AndroidBleBridge::new();
                let mut backend = super::AndroidBleBackend::new(bridge.clone());
                assert!(bridge.liveness_capability().is_empty());
                backend.set_session_liveness(enabled).await.unwrap();
                assert_eq!(!bridge.liveness_capability().is_empty(), enabled,);
                assert!(bridge.link_up_with_liveness(1, [1; 6], None, dialed, supported));
                let link = match backend.next_event().await {
                    BleEvent::Inbound(link) | BleEvent::LinkReady { link, .. } => link,
                    _ => panic!("native link"),
                };
                let expected = match (enabled && supported, dialed) {
                    (false, _) => LivenessMode::Disabled,
                    (true, false) => LivenessMode::Listener,
                    (true, true) => LivenessMode::Initiator,
                };
                assert_eq!(link.control.liveness_mode(), expected);
                // Readiness changes cannot rewrite an already admitted physical session.
                backend.set_session_liveness(!enabled).await.unwrap();
                assert_eq!(link.control.liveness_mode(), expected);
            }
        }
    }
}

#[tokio::test]
async fn legacy_admission_and_same_address_replacement_never_inherit_liveness() {
    use super::LivenessMode;
    let bridge = AndroidBleBridge::new();
    bridge.set_session_liveness(true);
    let mut backend = super::AndroidBleBackend::new(bridge.clone());
    assert_eq!(
        bridge.liveness_capability(),
        &super::LIVENESS_CAPABILITY_BYTES
    );
    assert!(bridge.link_up_with_liveness(1, [1; 6], None, true, true));
    let BleEvent::LinkReady { link: old, .. } = backend.next_event().await else {
        panic!("capable link")
    };
    assert_eq!(old.control.liveness_mode(), LivenessMode::Initiator);
    bridge.disconnected(1);
    assert!(bridge.link_up(1, [1; 6], None, false));
    let BleEvent::Inbound(replacement) = backend.next_event().await else {
        panic!("legacy replacement")
    };
    assert_eq!(replacement.control.liveness_mode(), LivenessMode::Disabled);
    drop(old);
    assert_eq!(
        bridge.control_in(1, &[0]),
        AndroidBleIngressAdmission::Accepted
    );
}

#[tokio::test]
async fn columba_cannot_gain_a_liveness_control_from_local_readiness() {
    use prns_core::interfaces::bluetooth_auto::BleLink;
    let bridge = AndroidBleBridge::new();
    bridge.set_session_liveness(true);
    let mut backend = super::AndroidBleBackend::new(bridge.clone());
    assert!(bridge.columba_link_up(1, [1; 6], None, false, [2; 16]));
    let BleEvent::Inbound(link) = backend.next_event().await else {
        panic!("Columba link")
    };
    assert_eq!(link.control.liveness_mode(), super::LivenessMode::Disabled);
    assert!(link.into_parts().control.is_none());
}

fn control_ticket(bridge: &AndroidBleBridge, conn_id: u32) -> AndroidBleControlTicket {
    let mut bytes = [0; super::CONTROL_BUFFER_LEN];
    let AndroidBleControlOutput::Ready { ticket, .. } = bridge.control_out(conn_id, &mut bytes)
    else {
        panic!("pending control")
    };
    ticket
}

async fn start_control(control: &mut super::AndroidBleControl, message: &Control) {
    // Poll once, then cancel just the waiting future. The operation remains owned.
    tokio::select! {
        biased;
        result = control.send(message) => panic!("admission is not completion: {result:?}"),
        () = tokio::task::yield_now() => {},
    }
}

#[tokio::test]
async fn canceled_control_send_rejoins_only_its_exact_message_and_receipt() {
    let bridge = AndroidBleBridge::new();
    let mut link = control_link(&bridge, 1).await;
    let message = Control::Close {
        reason: CloseReason::DuplicateLink,
    };
    start_control(&mut link.control, &message).await;
    let ticket = control_ticket(&bridge, 1);
    start_control(&mut link.control, &message).await;
    assert_eq!(control_ticket(&bridge, 1), ticket);
    assert_eq!(
        link.control
            .send(&Control::Close {
                reason: CloseReason::Incompatible
            })
            .await,
        Err(AndroidBleError::ControlBusy)
    );
    assert!(!bridge.complete_control_out(
        1,
        AndroidBleControlTicket {
            operation: ticket.operation + 1,
            ..ticket
        },
        true
    ));
    assert_eq!(control_ticket(&bridge, 1), ticket);
    assert!(bridge.complete_control_out(1, ticket, true));
    assert!(!bridge.complete_control_out(1, ticket, true));
    assert_eq!(link.control.send(&message).await, Ok(()));
    start_control(&mut link.control, &message).await;
    let next = control_ticket(&bridge, 1);
    assert_eq!(ticket.session, next.session);
    assert_ne!(ticket.operation, next.operation);
    assert!(!bridge.complete_control_out(1, ticket, false));
    assert!(bridge.complete_control_out(1, next, false));
    assert_eq!(
        link.control.send(&message).await,
        Err(AndroidBleError::ControlWriteFailed)
    );
    assert_eq!(bridge.next_close(), Some(1));
}

#[tokio::test(start_paused = true)]
async fn canceled_control_keeps_original_deadline_and_retires_the_physical_owner() {
    let bridge = AndroidBleBridge::new();
    let mut link = control_link(&bridge, 1).await;
    let message = Control::Close {
        reason: CloseReason::DuplicateLink,
    };
    start_control(&mut link.control, &message).await;
    let ticket = control_ticket(&bridge, 1);
    tokio::time::advance(Duration::from_secs(20)).await;
    start_control(&mut link.control, &message).await;
    assert_eq!(control_ticket(&bridge, 1), ticket);
    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(bridge.next_close(), Some(1));
    assert_eq!(
        link.control.send(&message).await,
        Err(AndroidBleError::ControlTimedOut)
    );
    assert!(!bridge.complete_control_out(1, ticket, true));
}

#[tokio::test]
async fn late_control_callback_cannot_complete_a_reused_connection_id() {
    let bridge = AndroidBleBridge::new();
    let mut old = control_link(&bridge, 7).await;
    let message = Control::Close {
        reason: CloseReason::DuplicateLink,
    };
    start_control(&mut old.control, &message).await;
    let stale = control_ticket(&bridge, 7);
    bridge.set_radio_mode(RadioMode::Off);
    let mut replacement = control_link(&bridge, 7).await;
    start_control(&mut replacement.control, &message).await;
    let current = control_ticket(&bridge, 7);
    assert_ne!(stale.session, current.session);
    assert!(!bridge.complete_control_out(7, stale, true));
    assert!(!bridge.complete_control_out(7, stale, false));
    assert_eq!(
        old.control.send(&message).await,
        Err(AndroidBleError::Closed)
    );
    drop(old);
    assert_eq!(control_ticket(&bridge, 7), current);
    assert_eq!(bridge.next_close(), None);
    assert!(bridge.complete_control_out(7, current, true));
    assert_eq!(replacement.control.send(&message).await, Ok(()));
}

#[tokio::test(start_paused = true)]
async fn native_completion_after_deadline_retires_even_if_watchdog_has_not_started() {
    let bridge = AndroidBleBridge::new();
    let link = control_link(&bridge, 1).await;
    let queue = link.control.outbound.clone();
    let (ticket, _, _) = queue
        .begin(
            &[3, 2],
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(bridge.complete_control_out(1, ticket, true));
    assert_eq!(
        queue.result(ticket).await,
        Err(AndroidBleError::ControlTimedOut)
    );
    assert_eq!(bridge.next_close(), Some(1));
}

#[tokio::test]
async fn waiting_watchdog_does_not_hold_the_physical_lease_alive() {
    let bridge = AndroidBleBridge::new();
    let mut link = control_link(&bridge, 1).await;
    let queue = link.control.outbound.clone();
    let message = Control::Close {
        reason: CloseReason::DuplicateLink,
    };
    start_control(&mut link.control, &message).await;
    drop(link);
    assert_eq!(bridge.next_close(), Some(1));
    tokio::task::yield_now().await;
    assert_eq!(std::sync::Arc::strong_count(&queue), 1);
}

#[tokio::test]
async fn control_output_reports_required_capacity_without_consuming_the_operation() {
    use prns_core::interfaces::bluetooth_auto::{
        AndroidHost, BleIdentity, Endpoint, LinkCapabilities, PeerDiscoveryGroups,
    };
    use prns_core::interfaces::{DiscoveryGroupHash, DiscoveryGroupHashSet};
    for count in [2, 4] {
        let bridge = AndroidBleBridge::new();
        let mut link = control_link(&bridge, 1).await;
        let hashes: Vec<_> = (1..=count)
            .map(|value| DiscoveryGroupHash::from_bytes([value; 32]))
            .collect();
        let hello = Control::Hello {
            identity: BleIdentity::new([1; 16]),
            endpoint: Endpoint::Android(AndroidHost::Android),
            capabilities: LinkCapabilities {
                l2cap: None,
                link_mtu: 247,
            },
            peer_rssi: None,
            discovery_groups: PeerDiscoveryGroups::Explicit(
                DiscoveryGroupHashSet::try_from_wire_ordered(&hashes).unwrap(),
            ),
        };
        start_control(&mut link.control, &hello).await;
        let mut wire = [0; super::CONTROL_BUFFER_LEN];
        let len = hello.encode(&mut wire).unwrap();
        assert_eq!(len, 25 + usize::from(count) * 32);
        assert_eq!(
            bridge.control_out(1, &mut [0; 64]),
            AndroidBleControlOutput::BufferTooSmall { required: len }
        );
        let mut received = [0; super::CONTROL_BUFFER_LEN];
        let AndroidBleControlOutput::Ready {
            ticket,
            len: actual,
        } = bridge.control_out(1, &mut received)
        else {
            panic!("full control output")
        };
        assert_eq!(actual, len);
        assert_eq!(&received[..len], &wire[..len]);
        assert_eq!(control_ticket(&bridge, 1), ticket);
        assert!(bridge.complete_control_out(1, ticket, true));
        assert_eq!(link.control.send(&hello).await, Ok(()));
    }
}

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
    let data = {
        let links = bridge.shared.links.lock().unwrap();
        let endpoints = links.get(&9).unwrap().active().unwrap();
        endpoints.data_out.clone()
    };
    data.push(vec![vec![6, 7], vec![8, 9, 10]]).await.unwrap();

    let mut out = [0u8; 8];

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

    let owners = {
        let links = bridge.shared.links.lock().unwrap();
        conn_ids
            .clone()
            .map(|id| {
                (
                    id,
                    links.get(&id).unwrap().active().unwrap().data_out.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    for (id, owner) in &owners {
        assert!(bridge.close_owned_connection(*id, owner));
    }
    assert_eq!(
        bridge.shared.close_requests.lock().unwrap().len(),
        super::bridge::PEER_CAPACITY
    );
    assert_eq!(
        bridge.shared.links.lock().unwrap().len(),
        super::bridge::PEER_CAPACITY,
        "closing links must retain their bridge slots"
    );

    for (id, owner) in &owners {
        assert!(bridge.close_owned_connection(*id, owner));
    }
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
    let owner = bridge
        .shared
        .links
        .lock()
        .unwrap()
        .get(&7)
        .unwrap()
        .active()
        .unwrap()
        .data_out
        .clone();
    assert!(bridge.close_owned_connection(7, &owner));

    assert!(!bridge.link_up(7, [6, 5, 4, 3, 2, 1], None, true));
    bridge.disconnected(7);
    assert!(bridge.link_up(7, [6, 5, 4, 3, 2, 1], None, true));
    assert!(bridge.close_owned_connection(7, &owner));
    assert!(bridge
        .shared
        .links
        .lock()
        .unwrap()
        .get(&7)
        .unwrap()
        .active()
        .is_some());
    assert_eq!(
        bridge.next_close(),
        None,
        "the old owner cannot close a reused connection ID"
    );
}

#[test]
fn radio_reset_discards_pending_physical_closes() {
    let bridge = AndroidBleBridge::new();
    let address = [1, 2, 3, 4, 5, 6];
    assert!(bridge.link_up(7, address, None, false));
    let owner = bridge
        .shared
        .links
        .lock()
        .unwrap()
        .get(&7)
        .unwrap()
        .active()
        .unwrap()
        .data_out
        .clone();
    assert!(bridge.close_owned_connection(7, &owner));
    assert_eq!(bridge.shared.close_requests.lock().unwrap().len(), 1);

    bridge.set_radio_mode(RadioMode::Off);

    assert!(bridge.shared.links.lock().unwrap().is_empty());
    assert!(bridge.shared.close_requests.lock().unwrap().is_empty());
    assert_eq!(bridge.next_close(), None);
}

#[tokio::test]
async fn dropping_a_challenger_closes_only_its_physical_connection() {
    use prns_core::interfaces::bluetooth_auto::{
        BleBackend, BleControl, BleEvent, BleLink, BleSink, CloseReason, Control, CONTROL_MAX_LEN,
    };
    let bridge = AndroidBleBridge::new();
    let mut backend = super::AndroidBleBackend::new(bridge.clone());
    let address = [1, 2, 3, 4, 5, 6];
    assert!(bridge.link_up(1, address, None, false));
    let BleEvent::Inbound(keeper) = backend.next_event().await else {
        panic!("keeper")
    };
    let prns_core::interfaces::bluetooth_auto::BleLinkParts {
        source,
        mut sink,
        mut control,
    } = keeper.into_parts();
    assert_eq!(
        bridge.next_close(),
        None,
        "settlement retains physical ownership"
    );

    assert!(bridge.link_up(2, address, None, false));
    let BleEvent::Inbound(challenger) = backend.next_event().await else {
        panic!("challenger")
    };
    drop(challenger);
    backend
        .on_link_closed(prns_core::interfaces::bluetooth_auto::BleAddress::new(
            address,
        ))
        .await;
    assert_eq!(bridge.next_close(), Some(2));
    assert_eq!(bridge.next_close(), None);
    sink.send_frame(&[1, 2, 3]).await.unwrap();
    let mut frame = [0; 32];
    assert!(
        bridge.data_out(1, &mut frame) > 0,
        "the keeper is still usable"
    );
    let message = Control::Close {
        reason: CloseReason::DuplicateLink,
    };
    let mut bytes = [0; CONTROL_MAX_LEN];
    let len = message.encode(&mut bytes).unwrap();
    assert_eq!(
        bridge.control_in(1, &bytes[..len]),
        AndroidBleIngressAdmission::Accepted
    );
    assert_eq!(control.as_mut().unwrap().recv().await.unwrap(), message);

    drop(source);
    assert_eq!(
        bridge.next_close(),
        None,
        "the other half still owns the connection"
    );
    drop(sink);
    assert_eq!(
        bridge.next_close(),
        None,
        "control still owns the connection"
    );
    drop(control);
    assert_eq!(bridge.next_close(), Some(1));
    assert_eq!(bridge.next_close(), None);
}

#[tokio::test]
async fn settled_control_retains_delivery_and_disconnect_observation_on_both_data_planes() {
    use prns_core::interfaces::bluetooth_auto::{
        BleBackend, BleControl, BleEvent, BleLink, CloseReason, Control, CONTROL_MAX_LEN,
    };
    for l2cap in [false, true] {
        let bridge = AndroidBleBridge::new();
        let mut backend = super::AndroidBleBackend::new(bridge.clone());
        assert!(bridge.link_up(1, [1; 6], None, false));
        let BleEvent::Inbound(mut link) = backend.next_event().await else {
            panic!("link")
        };
        let message = Control::Close {
            reason: CloseReason::DuplicateLink,
        };
        let mut bytes = [0; CONTROL_MAX_LEN];
        let len = message.encode(&mut bytes).unwrap();
        assert_eq!(
            bridge.control_in(1, &bytes[..len]),
            AndroidBleIngressAdmission::Accepted
        );
        assert_eq!(link.control_recv().await.unwrap(), message);
        if l2cap {
            bridge.l2cap_up(1);
        }
        let mut parts = link.into_parts();
        let control = parts.control.as_mut().expect("native control");
        assert_eq!(
            bridge.control_in(1, &bytes[..len]),
            AndroidBleIngressAdmission::Accepted
        );
        assert_eq!(control.recv().await.unwrap(), message);
        {
            let send = control.send(&message);
            tokio::pin!(send);
            tokio::select! {
                biased;
                result = &mut send => panic!("queue admission completed send: {result:?}"),
                () = tokio::task::yield_now() => {},
            }
            let mut written = [0; CONTROL_MAX_LEN];
            let super::AndroidBleControlOutput::Ready {
                len: written_len,
                ticket,
            } = bridge.control_out(1, &mut written)
            else {
                panic!("pending control")
            };
            assert_eq!(written_len, len);
            assert_eq!(&written[..len], &bytes[..len]);
            assert!(bridge.complete_control_out(1, ticket, true));
            assert_eq!(send.await, Ok(()));
        }
        bridge.disconnected(1);
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), control.recv())
                .await
                .unwrap(),
            Err(super::AndroidBleError::Closed)
        ));
    }
}

#[tokio::test]
async fn a_settled_control_owner_cannot_close_a_reused_connection_id() {
    use prns_core::interfaces::bluetooth_auto::{BleBackend, BleEvent, BleLink};
    let bridge = AndroidBleBridge::new();
    let mut backend = super::AndroidBleBackend::new(bridge.clone());
    assert!(bridge.link_up(7, [7; 6], None, false));
    let BleEvent::Inbound(link) = backend.next_event().await else {
        panic!("link")
    };
    let parts = link.into_parts();
    drop(parts.source);
    drop(parts.sink);
    assert_eq!(bridge.next_close(), None);
    bridge.set_radio_mode(RadioMode::Off);
    assert!(bridge.link_up(7, [8; 6], None, false));
    drop(parts.control);
    assert_eq!(bridge.next_close(), None);
    assert!(bridge.shared.links.lock().unwrap()[&7].active().is_some());
}

#[tokio::test]
async fn columba_settlement_has_no_native_control_owner() {
    use prns_core::interfaces::bluetooth_auto::{BleBackend, BleEvent, BleLink};
    let bridge = AndroidBleBridge::new();
    let mut backend = super::AndroidBleBackend::new(bridge.clone());
    assert!(bridge.columba_link_up(7, [7; 6], None, false, [9; 16]));
    let BleEvent::Inbound(link) = backend.next_event().await else {
        panic!("link")
    };
    let parts = link.into_parts();
    assert!(parts.control.is_none());
    drop(parts.source);
    assert_eq!(bridge.next_close(), None);
    drop(parts.sink);
    assert_eq!(bridge.next_close(), Some(7));
}

#[tokio::test]
async fn delayed_link_drop_after_radio_reset_cannot_close_a_reused_id() {
    use prns_core::interfaces::bluetooth_auto::{BleBackend, BleEvent};
    let bridge = AndroidBleBridge::new();
    let mut backend = super::AndroidBleBackend::new(bridge.clone());
    let address = [1, 2, 3, 4, 5, 6];
    assert!(bridge.link_up(1, address, None, false));
    let BleEvent::Inbound(old) = backend.next_event().await else {
        panic!("old link")
    };
    bridge.set_radio_mode(RadioMode::Off);
    assert!(bridge.link_up(1, address, None, false));
    let BleEvent::Inbound(current) = backend.next_event().await else {
        panic!("current link")
    };
    drop(old);
    assert_eq!(bridge.next_close(), None);
    assert!(bridge
        .shared
        .links
        .lock()
        .unwrap()
        .get(&1)
        .unwrap()
        .active()
        .is_some());
    drop(current);
    assert_eq!(bridge.next_close(), Some(1));
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
