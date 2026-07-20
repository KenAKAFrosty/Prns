use super::{AndroidBleBridge, RADIO_ADVERTISING, RADIO_ENABLED, RADIO_SCANNING};
use prns_core::interfaces::bluetooth_auto::seam::{AdvertisingMode, RadioMode, ScanningMode};

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
    bridge.link_up(7, [1, 2, 3, 4, 5, 6], None, true);

    for _ in 0..8 {
        assert!(bridge.control_in(7, &[1]));
    }
    assert!(!bridge.control_in(7, &[1]));

    for _ in 0..16 {
        assert!(bridge.data_in(7, &[2]));
    }
    assert!(!bridge.data_in(7, &[2]));
}
