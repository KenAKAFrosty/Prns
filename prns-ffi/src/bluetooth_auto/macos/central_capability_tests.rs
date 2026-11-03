use super::*;
use objc2_core_bluetooth::{CBAttributePermissions, CBMutableCharacteristic};
use prns_core::interfaces::bluetooth_auto::LIVENESS_CAPABILITY_BYTES;

fn characteristic() -> SendCharacteristicRef {
    // SAFETY: this test creates an ordinary retained, read-only characteristic without a radio.
    let mutable = unsafe {
        CBMutableCharacteristic::initWithType_properties_value_permissions(
            CBMutableCharacteristic::alloc(),
            &liveness_uuid(),
            CBCharacteristicProperties::Read,
            None,
            CBAttributePermissions::Readable,
        )
    };
    let characteristic: &CBCharacteristic = &mutable;
    SendCharacteristicRef(characteristic.retain())
}

struct Fixture {
    session: CentralPeerSession,
    completion: oneshot::Receiver<DialCompletion>,
    _control: tokio_mpsc::Receiver<Control>,
    _data: super::super::gatt_link::GattInboundReceiver,
}

fn fixture(enabled: bool) -> Fixture {
    let (control_tx, control) = tokio_mpsc::channel(8);
    let (completion_tx, completion) = oneshot::channel();
    let (data_tx, data) = super::super::gatt_link::gatt_inbound_channel();
    let mut session =
        CentralPeerSession::new(BleAddress::new([7; 6]), control_tx, completion_tx, data_tx);
    session.enable_liveness(enabled);
    Fixture {
        session,
        completion,
        _control: control,
        _data: data,
    }
}

fn completed_mode(fixture: &mut Fixture) -> LivenessMode {
    let Ok(DialCompletion::Ready(chars)) = fixture.completion.try_recv() else {
        panic!("the current native session should be ready");
    };
    assert_eq!(chars.peer_protocol, PeerProtocol::Native);
    chars.liveness
}

#[test]
fn absent_capability_and_unready_runtime_never_enable_liveness() {
    for (enabled, present) in [(true, false), (false, true), (false, false)] {
        let mut fixture = fixture(enabled);
        let control = characteristic();
        assert!(fixture
            .session
            .select_native(
                SendCharacteristicRef(control.0.clone()),
                None,
                present.then(characteristic),
            )
            .is_none());
        assert!(fixture.session.native_subscriptions().is_some());
        fixture.session.native_ready(control);
        assert_eq!(completed_mode(&mut fixture), LivenessMode::Disabled);
    }
}

#[test]
fn exact_supported_read_is_required_before_subscriptions_and_readiness() {
    let mut fixture = fixture(true);
    let control = characteristic();
    let capability = characteristic();
    let stale = characteristic();
    assert!(fixture
        .session
        .select_native(
            SendCharacteristicRef(control.0.clone()),
            None,
            Some(SendCharacteristicRef(capability.0.clone())),
        )
        .is_some());
    assert!(fixture.session.capability_read_pending());
    assert!(fixture.session.native_subscriptions().is_none());
    fixture
        .session
        .native_ready(SendCharacteristicRef(control.0.clone()));
    assert!(matches!(
        fixture.completion.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(!fixture
        .session
        .finish_capability_read(&stale.0, &LIVENESS_CAPABILITY_BYTES));
    assert!(fixture.session.capability_read_pending());
    assert!(fixture
        .session
        .finish_capability_read(&capability.0, &LIVENESS_CAPABILITY_BYTES));
    assert!(!fixture.session.finish_capability_read(&capability.0, &[]));
    assert!(fixture.session.native_subscriptions().is_some());
    fixture.session.native_ready(control);
    assert_eq!(completed_mode(&mut fixture), LivenessMode::Initiator);
}

#[test]
fn failed_empty_unknown_or_extended_capability_reads_remain_legacy() {
    for value in [
        &[][..],
        b"PRNS\x02\x01",
        b"PRNS\x01\x00",
        b"PRNS\x01\x01\x00",
    ] {
        let mut fixture = fixture(true);
        let control = characteristic();
        let capability = characteristic();
        fixture.session.select_native(
            SendCharacteristicRef(control.0.clone()),
            None,
            Some(SendCharacteristicRef(capability.0.clone())),
        );
        assert!(fixture.session.finish_capability_read(&capability.0, value));
        fixture.session.native_ready(control);
        assert_eq!(completed_mode(&mut fixture), LivenessMode::Disabled);
    }
}

#[test]
fn accepted_read_fences_writes_and_expired_dial_cannot_be_revived() {
    let mut fixture = fixture(true);
    let control = characteristic();
    let capability = characteristic();
    fixture.session.select_native(
        SendCharacteristicRef(control.0.clone()),
        None,
        Some(SendCharacteristicRef(capability.0.clone())),
    );
    let (tx, _rx) = oneshot::channel();
    let write = GattWriteRequest::new(
        SendCharacteristicRef(control.0.clone()),
        Box::from([1]),
        GattWriteMode::WithResponse,
        tx,
    );
    assert!(fixture
        .session
        .begin_acknowledged_write(write.into_acknowledged())
        .is_err());
    let (tx, _rx) = oneshot::channel();
    let write = GattWriteRequest::new(control, Box::from([1]), GattWriteMode::WithoutResponse, tx);
    assert!(fixture.session.hold_unacknowledged_write(write).is_err());
    drop(fixture.completion);
    assert!(!fixture
        .session
        .finish_capability_read(&capability.0, &LIVENESS_CAPABILITY_BYTES));
    assert!(fixture.session.native_subscriptions().is_none());
    assert!(
        fixture.session.retire(),
        "dial expiry must still cancel the physical attempt"
    );
}

#[test]
fn fresh_restoration_discovery_replaces_capability_ownership() {
    let mut fixture = fixture(true);
    let control = characteristic();
    let old = characteristic();
    let new = characteristic();
    fixture.session.select_native(
        SendCharacteristicRef(control.0.clone()),
        None,
        Some(SendCharacteristicRef(old.0.clone())),
    );
    assert!(fixture
        .session
        .finish_capability_read(&old.0, &LIVENESS_CAPABILITY_BYTES));
    fixture.session.configure_restoration_recovery(true, true);
    assert_eq!(
        fixture.session.restoration_profile(PeerProtocol::Native),
        Ok(RestorationProfileAction::Disconnect)
    );
    assert_eq!(
        fixture.session.restoration_disconnected(),
        RestorationDisconnectAction::Reconnect
    );
    assert!(fixture.session.restoration_connected());
    assert!(fixture.session.restoration_services_discovered());
    assert_eq!(
        fixture.session.restoration_profile(PeerProtocol::Native),
        Ok(RestorationProfileAction::Continue)
    );
    fixture.session.select_native(
        SendCharacteristicRef(control.0.clone()),
        None,
        Some(SendCharacteristicRef(new.0.clone())),
    );
    assert!(!fixture
        .session
        .finish_capability_read(&old.0, &LIVENESS_CAPABILITY_BYTES));
    assert!(fixture.session.finish_capability_read(&new.0, &[]));
    fixture.session.native_ready(control);
    assert_eq!(completed_mode(&mut fixture), LivenessMode::Disabled);
}

#[tokio::test(start_paused = true)]
async fn late_capability_or_subscription_cannot_beat_the_original_deadline() {
    for read_completed_before_deadline in [false, true] {
        let mut fixture = fixture(true);
        let original_deadline = fixture.session.dial_deadline();
        let control = characteristic();
        let capability = characteristic();
        fixture.session.select_native(
            SendCharacteristicRef(control.0.clone()),
            None,
            Some(SendCharacteristicRef(capability.0.clone())),
        );
        if read_completed_before_deadline {
            assert!(fixture
                .session
                .finish_capability_read(&capability.0, &LIVENESS_CAPABILITY_BYTES));
        }
        tokio::time::advance(super::super::backend::DIAL_TIMEOUT).await;
        assert!(!fixture
            .session
            .finish_capability_read(&capability.0, &LIVENESS_CAPABILITY_BYTES));
        assert!(fixture.session.native_subscriptions().is_none());
        fixture.session.native_ready(control);
        assert!(matches!(
            fixture.completion.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(fixture.session.dial_deadline(), original_deadline);
        assert!(fixture.session.retire());
        assert!(matches!(
            fixture.completion.try_recv(),
            Ok(DialCompletion::Failed)
        ));
    }
}

#[test]
fn stale_queued_write_or_startup_cleanup_cannot_target_a_replacement_session() {
    let (signals, _state) = super::super::manager_signal_channel();
    let (sightings, _receiver) = tokio_mpsc::channel(1);
    let delegate = CentralDelegate::new(
        signals,
        sightings,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(true)),
        2,
        2,
    );
    let old = fixture(true);
    let old_gate = old.session.write_gate.clone();
    let mut replacement = fixture(true);
    let new_gate = replacement.session.write_gate.clone();
    assert!(replacement.session.owns_live_write_gate(&new_gate));
    assert!(!replacement.session.owns_live_write_gate(&old_gate));
    let peer = CoreBluetoothPeerId([7; 16]);
    delegate
        .ivars()
        .sessions
        .borrow_mut()
        .insert(peer, replacement.session);
    delegate.fail_write_owner(peer, &old_gate);
    assert!(delegate.has_session(peer));
    assert!(matches!(
        replacement.completion.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    delegate.fail_write_owner(peer, &new_gate);
    assert!(!delegate.has_session(peer));
    assert!(matches!(
        replacement.completion.try_recv(),
        Ok(DialCompletion::Failed)
    ));
}
