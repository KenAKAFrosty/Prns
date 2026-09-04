use std::collections::HashMap;
use std::time::{Duration, Instant};

use objc2_core_bluetooth::CBCharacteristicProperties;
use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleIdentity, Control, ScanningMode,
};
use tokio::sync::{mpsc, oneshot};

use super::backend::{
    dial_admission, manager_readiness, scan_lease, scan_op, DialAdmission, ScanLease, ScanOp,
};
use super::central::{
    CentralPeerSession, RestoredCallbackBuffer, CENTRAL_CONTROL_INBOUND_CAPACITY,
};
use super::discovery::{
    candidate_strength, discover_disposition, CandidateStrength, DiscoverDisposition,
    DiscoveryGuard, PeripheralLinkState, SessionPresence, StaleCancellation, StaleLinkRecovery,
};
use super::gatt_link::{
    gatt_inbound_channel, gatt_inbound_channel_with_budget, GattInboundSendError,
    GATT_INBOUND_BUDGET_BYTES,
};
use super::gatt_write::{write_admission, GattWriteAdmission, GattWriteMode, GattWritePlan};
use super::legacy_restoration_identifiers;
use super::peripheral::{advertising_op, has_session_for_peer, AdvertisingOp};
use super::MacosBleError;
use super::{
    manager_signal_channel, try_bounded_ingress, BoundedIngress, CoreBluetoothPeerId,
    MacosBleBackend, Sighting,
};
use super::{CoreBluetoothRestorationIdentifiers, CoreBluetoothRestorationIdentifiersError};

fn peer_id(value: u16) -> CoreBluetoothPeerId {
    let mut bytes = [0; 16];
    bytes[..2].copy_from_slice(&value.to_le_bytes());
    CoreBluetoothPeerId(bytes)
}

#[test]
fn restoration_identifiers_are_nonempty_and_distinct(
) -> Result<(), CoreBluetoothRestorationIdentifiersError> {
    assert_eq!(
        CoreBluetoothRestorationIdentifiers::new("", "peripheral"),
        Err(CoreBluetoothRestorationIdentifiersError::EmptyCentral)
    );
    assert_eq!(
        CoreBluetoothRestorationIdentifiers::new("central", ""),
        Err(CoreBluetoothRestorationIdentifiersError::EmptyPeripheral)
    );
    assert_eq!(
        CoreBluetoothRestorationIdentifiers::new("shared", "shared"),
        Err(CoreBluetoothRestorationIdentifiersError::Duplicate)
    );

    let identifiers = CoreBluetoothRestorationIdentifiers::new("central", "peripheral")?;
    assert_eq!(identifiers.central(), "central");
    assert_eq!(identifiers.peripheral(), "peripheral");
    Ok(())
}

#[test]
fn legacy_preparation_keeps_personal_hopspot_identifiers() {
    let identifiers = legacy_restoration_identifiers();
    assert_eq!(identifiers.central(), "com.personal.prns.ble.central");
    assert_eq!(identifiers.peripheral(), "com.personal.prns.ble.peripheral");
}

#[test]
fn discovery_recovery_distinguishes_owned_stale_and_transitioning_links() {
    assert_eq!(
        discover_disposition(
            PeripheralLinkState::Disconnected,
            SessionPresence::Absent,
            StaleCancellation::Idle,
            StaleLinkRecovery::Enabled,
        ),
        DiscoverDisposition::Adopt
    );
    for state in [
        PeripheralLinkState::Connecting,
        PeripheralLinkState::Connected,
    ] {
        assert_eq!(
            discover_disposition(
                state,
                SessionPresence::Present,
                StaleCancellation::Idle,
                StaleLinkRecovery::Enabled,
            ),
            DiscoverDisposition::IgnoreOwned
        );
        assert_eq!(
            discover_disposition(
                state,
                SessionPresence::Absent,
                StaleCancellation::Idle,
                StaleLinkRecovery::Enabled,
            ),
            DiscoverDisposition::CancelStale
        );
        assert_eq!(
            discover_disposition(
                state,
                SessionPresence::Absent,
                StaleCancellation::InFlight,
                StaleLinkRecovery::Enabled,
            ),
            DiscoverDisposition::WaitForDisconnect
        );
        assert_eq!(
            discover_disposition(
                state,
                SessionPresence::Absent,
                StaleCancellation::Idle,
                StaleLinkRecovery::Disabled,
            ),
            DiscoverDisposition::WaitForDisconnect
        );
    }
    for state in [
        PeripheralLinkState::Disconnecting,
        PeripheralLinkState::Unknown,
    ] {
        assert_eq!(
            discover_disposition(
                state,
                SessionPresence::Absent,
                StaleCancellation::Idle,
                StaleLinkRecovery::Enabled,
            ),
            DiscoverDisposition::WaitForDisconnect
        );
    }
}

#[test]
fn candidate_strength_accepts_prns_name_or_manufacturer_marker() {
    assert_eq!(candidate_strength(true, None), CandidateStrength::Strong);
    assert_eq!(
        candidate_strength(false, Some(&[0xff, 0xff, 0x03, 0x00])),
        CandidateStrength::Strong
    );
    assert_eq!(
        candidate_strength(false, Some(&[0x4c, 0x00, 0x03, 0x00])),
        CandidateStrength::Weak
    );
    assert_eq!(candidate_strength(false, None), CandidateStrength::Weak);
}

#[test]
fn service_miss_suppresses_only_weak_candidates_until_expiry() {
    let now = Instant::now();
    let peer = peer_id(1);
    let mut guard = DiscoveryGuard::default();

    assert!(guard.admit_candidate(peer, CandidateStrength::Weak, now));
    guard.record_service_miss(peer, now);
    assert!(!guard.admit_candidate(
        peer,
        CandidateStrength::Weak,
        now + Duration::from_secs(299)
    ));
    assert!(guard.admit_candidate(
        peer,
        CandidateStrength::Strong,
        now + Duration::from_secs(299)
    ));

    guard.record_service_miss(peer, now);
    assert!(guard.admit_candidate(
        peer,
        CandidateStrength::Weak,
        now + Duration::from_secs(300)
    ));
}

#[test]
fn discovery_guard_bounds_service_misses_and_stale_cancellation_retries() {
    let now = Instant::now();
    let mut guard = DiscoveryGuard::default();
    for value in 0..=255 {
        guard.record_service_miss(peer_id(value), now);
    }
    assert_eq!(guard.suppressed_len(), 256);
    guard.record_service_miss(peer_id(256), now + Duration::from_secs(1));
    assert_eq!(guard.suppressed_len(), 256);

    let peer = peer_id(500);
    assert!(!guard.cancellation_recent(peer, now));
    guard.record_stale_cancellation(peer, now);
    assert!(guard.cancellation_recent(peer, now + Duration::from_secs(29)));
    assert!(!guard.cancellation_recent(peer, now + Duration::from_secs(30)));
}

#[test]
fn startup_requires_central_gatt_and_l2cap_readiness() {
    let (signals, current) = manager_signal_channel();
    signals.l2cap_published(0x0081);
    assert_eq!(manager_readiness(*current.borrow()).unwrap(), None);

    signals.central_powered();
    assert_eq!(manager_readiness(*current.borrow()).unwrap(), None);

    signals.gatt_service_published();
    assert_eq!(
        manager_readiness(*current.borrow())
            .unwrap()
            .map(|psm| psm.get()),
        Some(0x0081)
    );
}

#[test]
fn bounded_ingress_separates_inbound_and_sighting_pressure() {
    let (inbound_tx, mut inbound_rx) = mpsc::channel(1);
    let (sighting_tx, mut sighting_rx) = mpsc::channel(1);
    let (manager_signals, manager_current) = manager_signal_channel();
    let address = prns_core::interfaces::bluetooth_auto::BleAddress::new([1; 6]);
    let sighting = Sighting {
        address,
        rssi: Some(-62),
    };

    assert_eq!(
        try_bounded_ingress(&inbound_tx, 1_u8),
        BoundedIngress::Accepted
    );
    assert_eq!(
        try_bounded_ingress(&sighting_tx, sighting),
        BoundedIngress::Accepted
    );
    assert_eq!(
        try_bounded_ingress(&inbound_tx, 2_u8),
        BoundedIngress::Full(2)
    );
    assert_eq!(
        try_bounded_ingress(
            &sighting_tx,
            Sighting {
                address,
                rssi: Some(-50),
            }
        ),
        BoundedIngress::Full(Sighting {
            address,
            rssi: Some(-50),
        })
    );
    manager_signals.central_powered();
    manager_signals.gatt_service_published();
    manager_signals.l2cap_published(0x0081);
    assert_eq!(
        manager_readiness(*manager_current.borrow())
            .unwrap()
            .map(|psm| psm.get()),
        Some(0x0081)
    );
    assert_eq!(inbound_rx.try_recv(), Ok(1));
    assert_eq!(sighting_rx.try_recv(), Ok(sighting));
}

#[test]
fn bounded_ingress_returns_rejected_payload_when_receiver_is_closed() {
    let (sender, receiver) = mpsc::channel(1);
    drop(receiver);
    assert_eq!(
        try_bounded_ingress(&sender, 7_u8),
        BoundedIngress::Closed(7)
    );
}

#[test]
fn manager_failures_are_sticky_and_fatal() {
    let (gatt_signals, gatt_current) = manager_signal_channel();
    gatt_signals.gatt_service_publish_failed();
    gatt_signals.gatt_service_published();
    assert!(matches!(
        manager_readiness(*gatt_current.borrow()),
        Err(MacosBleError::PublishFailed)
    ));

    let (l2cap_signals, l2cap_current) = manager_signal_channel();
    l2cap_signals.l2cap_publish_failed();
    l2cap_signals.l2cap_published(0x0081);
    assert!(matches!(
        manager_readiness(*l2cap_current.borrow()),
        Err(MacosBleError::PublishFailed)
    ));
}

#[test]
fn dial_admission_is_scoped_to_the_target_peer() {
    let inbound_peer = peer_id(1);
    let unrelated_peer = peer_id(2);
    let inbound_sessions = HashMap::from([(inbound_peer, ())]);

    assert_eq!(
        dial_admission(true, false, false),
        DialAdmission::YieldToSystemConnection
    );
    assert_eq!(
        dial_admission(
            false,
            has_session_for_peer(&inbound_sessions, inbound_peer),
            false,
        ),
        DialAdmission::YieldToInboundSession
    );
    assert_eq!(
        dial_admission(
            false,
            has_session_for_peer(&inbound_sessions, unrelated_peer),
            false,
        ),
        DialAdmission::AttachCentralSession
    );
}

#[test]
fn dial_admission_resumes_only_restored_system_connections() {
    assert_eq!(
        dial_admission(true, false, true),
        DialAdmission::ResumeRestoredSession
    );
    assert_eq!(
        dial_admission(false, false, true),
        DialAdmission::AttachCentralSession
    );
    assert_eq!(
        dial_admission(true, true, true),
        DialAdmission::YieldToInboundSession
    );
    assert_eq!(
        dial_admission(true, true, false),
        DialAdmission::YieldToInboundSession
    );
}

#[test]
fn scan_op_starts_restarts_and_stops_without_spurious_work() {
    assert_eq!(scan_op(true, false, false), ScanOp::Start);
    assert_eq!(scan_op(true, false, true), ScanOp::Start);
    assert_eq!(scan_op(true, true, false), ScanOp::None);
    assert_eq!(scan_op(true, true, true), ScanOp::Restart);
    assert_eq!(scan_op(false, true, false), ScanOp::Stop);
    assert_eq!(scan_op(false, true, true), ScanOp::Stop);
    assert_eq!(scan_op(false, false, true), ScanOp::None);
}

#[test]
fn scan_lease_restarts_only_an_enabled_scan_without_observed_activity() {
    assert_eq!(scan_lease(false, false), ScanLease::Inactive);
    assert_eq!(scan_lease(false, true), ScanLease::Inactive);
    assert_eq!(scan_lease(true, true), ScanLease::Renewed);
    assert_eq!(scan_lease(true, false), ScanLease::Expired);
}

#[test]
fn advertising_reconciliation_never_bounces_a_healthy_advertisement() {
    assert_eq!(advertising_op(true, false), AdvertisingOp::Start);
    assert_eq!(advertising_op(true, true), AdvertisingOp::None);
    assert_eq!(advertising_op(false, true), AdvertisingOp::Stop);
    assert_eq!(advertising_op(false, false), AdvertisingOp::None);
}

#[test]
fn role_cleanup_only_selects_a_session_after_its_data_receiver_closes() {
    let (control_tx, _control_rx) = mpsc::channel::<Control>(1);
    let (completion_tx, _completion_rx) = oneshot::channel();
    let (data_tx, data_rx) = gatt_inbound_channel();
    let session = CentralPeerSession::new(
        prns_core::interfaces::bluetooth_auto::BleAddress::new([1; 6]),
        control_tx,
        completion_tx,
        data_tx,
    );

    assert!(!session.data_receiver_closed());
    drop(data_rx);
    assert!(session.data_receiver_closed());
}

#[tokio::test]
async fn restored_value_callbacks_are_handed_to_the_admitted_session() {
    let control = Control::decode(&[0x03, 0x01]).expect("valid close control");
    let mut callbacks = RestoredCallbackBuffer::default();
    assert!(callbacks.buffer_control(control));
    assert!(callbacks.buffer_data(Box::from(&[1, 2, 3][..])));

    let (control_tx, mut control_rx) = mpsc::channel(CENTRAL_CONTROL_INBOUND_CAPACITY);
    let (completion_tx, _completion_rx) = oneshot::channel();
    let (data_tx, mut data_rx) = gatt_inbound_channel();
    let mut session = CentralPeerSession::new(
        prns_core::interfaces::bluetooth_auto::BleAddress::new([1; 6]),
        control_tx,
        completion_tx,
        data_tx,
    );

    assert!(session.restore_callbacks(callbacks));
    assert_eq!(control_rx.try_recv(), Ok(control));
    assert_eq!(&*data_rx.recv().await.unwrap(), &[1, 2, 3]);
}

#[test]
fn restored_callback_overflow_fails_instead_of_skipping_protocol_input() {
    let control = Control::decode(&[0x03, 0x01]).expect("valid close control");
    let mut controls = RestoredCallbackBuffer::default();
    for _ in 0..CENTRAL_CONTROL_INBOUND_CAPACITY {
        assert!(controls.buffer_control(control));
    }
    assert!(!controls.buffer_control(control));
    let (control_tx, _control_rx) = mpsc::channel(CENTRAL_CONTROL_INBOUND_CAPACITY);
    let (completion_tx, _completion_rx) = oneshot::channel();
    let (data_tx, _data_rx) = gatt_inbound_channel();
    let mut session = CentralPeerSession::new(
        prns_core::interfaces::bluetooth_auto::BleAddress::new([1; 6]),
        control_tx,
        completion_tx,
        data_tx,
    );
    assert!(!session.restore_callbacks(controls));

    let mut data = RestoredCallbackBuffer::default();
    assert!(!data.buffer_data(vec![0; GATT_INBOUND_BUDGET_BYTES + 1].into_boxed_slice()));
}

#[tokio::test]
async fn gatt_callback_inbox_preserves_bursts_up_to_its_byte_budget() {
    let (data_tx, mut data_rx) = gatt_inbound_channel_with_budget(5);

    data_tx
        .try_send(Box::from(&[1, 2, 3][..]))
        .expect("the first fragment should fit");
    data_tx
        .try_send(Box::from(&[4, 5][..]))
        .expect("the burst should fill the byte budget exactly");
    assert_eq!(
        data_tx.try_send(Box::from(&[6][..])),
        Err(GattInboundSendError::BudgetExceeded)
    );

    assert_eq!(&*data_rx.recv().await.unwrap(), &[1, 2, 3]);
    data_tx
        .try_send(Box::from(&[6, 7, 8][..]))
        .expect("receiving a fragment should release its exact capacity");
    assert_eq!(&*data_rx.recv().await.unwrap(), &[4, 5]);
    assert_eq!(&*data_rx.recv().await.unwrap(), &[6, 7, 8]);
}

#[tokio::test]
async fn gatt_callback_inbox_bounds_empty_callbacks_and_reports_closure() {
    let (data_tx, data_rx) = gatt_inbound_channel_with_budget(1);

    data_tx
        .try_send(Box::from(&[][..]))
        .expect("an empty callback consumes one unit of capacity");
    assert_eq!(
        data_tx.try_send(Box::from(&[][..])),
        Err(GattInboundSendError::BudgetExceeded)
    );

    drop(data_rx);
    assert_eq!(
        data_tx.try_send(Box::from(&[1][..])),
        Err(GattInboundSendError::Closed)
    );
}

#[test]
fn native_write_selects_acknowledged_atomic_fragments_when_both_modes_are_advertised() {
    let plan = GattWritePlan::from_discovery(
        GattWriteMode::WithResponse,
        CBCharacteristicProperties::Write | CBCharacteristicProperties::WriteWithoutResponse,
        512,
        244,
    )
    .unwrap();

    assert_eq!(plan.mode(), GattWriteMode::WithResponse);
    assert_eq!(plan.fragment_mtu(), 180);
}

#[test]
fn columba_write_selects_unacknowledged_fragments_when_advertised() {
    let plan = GattWritePlan::from_discovery(
        GattWriteMode::WithoutResponse,
        CBCharacteristicProperties::Write | CBCharacteristicProperties::WriteWithoutResponse,
        512,
        120,
    )
    .unwrap();

    assert_eq!(plan.mode(), GattWriteMode::WithoutResponse);
    assert_eq!(plan.fragment_mtu(), 120);
}

#[test]
fn unsupported_or_non_fragmentable_characteristics_are_rejected() {
    assert!(matches!(
        GattWritePlan::from_discovery(
            GattWriteMode::WithResponse,
            CBCharacteristicProperties::Notify,
            512,
            244
        ),
        Err(MacosBleError::UnsupportedWriteMode)
    ));
    assert!(matches!(
        GattWritePlan::from_discovery(
            GattWriteMode::WithResponse,
            CBCharacteristicProperties::Write,
            512,
            5
        ),
        Err(MacosBleError::InvalidWriteMtu)
    ));
    assert!(matches!(
        GattWritePlan::from_discovery(
            GattWriteMode::WithResponse,
            CBCharacteristicProperties::WriteWithoutResponse,
            512,
            244
        ),
        Err(MacosBleError::UnsupportedWriteMode)
    ));
    assert!(matches!(
        GattWritePlan::from_discovery(
            GattWriteMode::WithoutResponse,
            CBCharacteristicProperties::Write,
            512,
            244
        ),
        Err(MacosBleError::UnsupportedWriteMode)
    ));
}

#[test]
fn write_admission_serializes_acks_and_waits_for_unacknowledged_capacity() {
    assert_eq!(
        write_admission(GattWriteMode::WithResponse, false, false, false),
        GattWriteAdmission::Issue
    );
    assert_eq!(
        write_admission(GattWriteMode::WithResponse, true, false, false),
        GattWriteAdmission::Busy
    );
    assert_eq!(
        write_admission(GattWriteMode::WithoutResponse, false, false, false),
        GattWriteAdmission::WaitForCapacity
    );
    assert_eq!(
        write_admission(GattWriteMode::WithoutResponse, false, false, true),
        GattWriteAdmission::Issue
    );
    assert_eq!(
        write_admission(GattWriteMode::WithoutResponse, false, true, true),
        GattWriteAdmission::Busy
    );
}

#[tokio::test]
#[ignore = "needs a real Bluetooth radio + Bluetooth permission; run with `--ignored` on a Mac"]
async fn the_node_publishes_then_accepts_explicit_radio_modes() {
    let mut backend = MacosBleBackend::new(BleIdentity::new([0; 16]))
        .await
        .expect("bluetooth should power on and publish both listeners");
    <MacosBleBackend as BleBackend<{ MacosBleBackend::MAX_PEERS }>>::set_advertising(
        &mut backend,
        AdvertisingMode::On,
    )
    .await
    .expect("advertising should start");
    <MacosBleBackend as BleBackend<{ MacosBleBackend::MAX_PEERS }>>::set_scanning(
        &mut backend,
        ScanningMode::On,
    )
    .await
    .expect("scanning should start");
}
