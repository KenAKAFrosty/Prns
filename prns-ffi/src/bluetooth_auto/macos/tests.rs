use std::time::{Duration, Instant};

use objc2_core_bluetooth::CBCharacteristicProperties;
use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleIdentity, Control, ScanningMode,
};
use tokio::sync::{mpsc, oneshot};

use super::backend::{dial_admission, DialAdmission, StartupReadiness};
use super::central::CentralPeerSession;
use super::discovery::{
    candidate_strength, discover_disposition, CandidateStrength, DiscoverDisposition,
    DiscoveryGuard, PeripheralLinkState, SessionPresence, StaleCancellation, StaleLinkRecovery,
};
use super::gatt_link::{
    gatt_inbound_channel, gatt_inbound_channel_with_budget, GattInboundSendError,
};
use super::gatt_write::{write_admission, GattWriteAdmission, GattWriteMode, GattWritePlan};
use super::MacosBleError;
use super::{CoreBluetoothPeerId, MacosBleBackend};

fn peer_id(value: u16) -> CoreBluetoothPeerId {
    let mut bytes = [0; 16];
    bytes[..2].copy_from_slice(&value.to_le_bytes());
    CoreBluetoothPeerId(bytes)
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
    let mut readiness = StartupReadiness::default();
    readiness.note_l2cap_published(0x0081).unwrap();
    assert_eq!(readiness.ready_psm(), None);

    readiness.note_central_powered();
    assert_eq!(readiness.ready_psm(), None);

    readiness.note_gatt_service_published();
    assert_eq!(readiness.ready_psm().map(|psm| psm.get()), Some(0x0081));
}

#[test]
fn dial_yields_only_when_peer_is_already_system_connected() {
    assert_eq!(dial_admission(true), DialAdmission::YieldToSystemConnection);
    assert_eq!(dial_admission(false), DialAdmission::AttachCentralSession);
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
