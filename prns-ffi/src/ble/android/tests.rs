use super::{AndroidBleBridge, RADIO_ADVERTISING, RADIO_ENABLED, RADIO_SCANNING};

#[test]
fn disabled_radio_exposes_no_android_ble_work() {
    let bridge = AndroidBleBridge::new();

    bridge.set_radio_enabled(true);
    bridge.set_advertising(true);
    bridge.set_scanning(true);
    bridge.set_psm(0x0080);
    assert_eq!(
        bridge.radio_state(),
        RADIO_ENABLED | RADIO_ADVERTISING | RADIO_SCANNING
    );

    bridge.set_radio_enabled(false);

    assert_eq!(bridge.radio_state(), 0);
    assert!(bridge.shared.psm.lock().unwrap().is_none());
    assert!(bridge.shared.links.lock().unwrap().is_empty());
    assert!(bridge.shared.events.lock().unwrap().is_empty());
    assert!(bridge.shared.dial_requests.lock().unwrap().is_empty());
    assert!(bridge.shared.l2cap_opens.lock().unwrap().is_empty());
}

#[test]
fn advertising_or_scanning_without_enabled_stays_invisible() {
    let bridge = AndroidBleBridge::new();

    bridge.set_advertising(true);
    bridge.set_scanning(true);

    assert_eq!(bridge.radio_state(), 0);
}
