use prns_core::interfaces::bluetooth_auto::{
    AppleHost, BleIdentity, Control, Endpoint, LinkCapabilities, PeerProtocol,
};
use tokio::sync::{mpsc, oneshot};

use super::central::{
    CentralDialCandidate, CentralPeerRegistry, CentralPeerSession, DialCompletion,
    RestorationDisconnectAction, RestorationProfileAction, RestoredAdmission,
    RestoredCallbackBuffer, CENTRAL_CONTROL_INBOUND_CAPACITY,
};
use super::gatt_link::{gatt_inbound_channel, GattInboundReceiver};
use super::CoreBluetoothPeerId;

struct Fixture {
    session: CentralPeerSession,
    control: mpsc::Receiver<Control>,
    completion: oneshot::Receiver<DialCompletion>,
    data: GattInboundReceiver,
}

fn peer() -> CoreBluetoothPeerId {
    CoreBluetoothPeerId([0x42; 16])
}

fn fixture(ios: bool, initially_connected: bool) -> Fixture {
    let (control_tx, control) = mpsc::channel(CENTRAL_CONTROL_INBOUND_CAPACITY);
    let (completion_tx, completion) = oneshot::channel();
    let (data_tx, data) = gatt_inbound_channel();
    let mut session = CentralPeerSession::new(peer().address(), control_tx, completion_tx, data_tx);
    session.configure_restoration_recovery(ios, initially_connected);
    Fixture {
        session,
        control,
        completion,
        data,
    }
}

fn greetings() -> [Control; 2] {
    let identity = BleIdentity::new([0x5a; 16]);
    let endpoint = Endpoint::CoreBluetooth(AppleHost::Ios);
    let capabilities = LinkCapabilities {
        l2cap: None,
        link_mtu: 512,
    };
    [
        Control::Hello {
            identity,
            endpoint,
            capabilities,
            peer_rssi: None,
        },
        Control::Welcome {
            identity,
            endpoint,
            capabilities,
            peer_rssi: Some(-47),
        },
    ]
}

fn fresh_native_discovery(session: &mut CentralPeerSession) {
    discard_old_traffic(session);
    assert!(session.restoration_connected());
    discard_old_traffic(session);
    assert!(session.restoration_services_discovered());
    discard_old_traffic(session);
    assert!(session.restoration_characteristics_expected());
    assert_eq!(
        session.restoration_profile(PeerProtocol::Native),
        Ok(RestorationProfileAction::Continue)
    );
    discard_old_traffic(session);
    assert!(session.restoration_native_subscribed());
}

fn discard_old_traffic(session: &mut CentralPeerSession) {
    session.enqueue_control(greetings()[1]).unwrap();
    session.enqueue_data(Box::from(&[0xee][..])).unwrap();
}

async fn receive_data(data: &mut GattInboundReceiver) -> Box<[u8]> {
    tokio::time::timeout(std::time::Duration::from_secs(1), data.recv())
        .await
        .expect("the already-queued data must be readable")
        .expect("the live session must retain its data sender")
}

#[test]
fn only_ios_previously_connected_native_profile_requests_a_reset() {
    for (ios, initially_connected, protocol, expected) in [
        (
            true,
            true,
            PeerProtocol::Native,
            RestorationProfileAction::Disconnect,
        ),
        (
            true,
            true,
            PeerProtocol::Columba,
            RestorationProfileAction::Continue,
        ),
        (
            false,
            true,
            PeerProtocol::Native,
            RestorationProfileAction::Continue,
        ),
        (
            false,
            true,
            PeerProtocol::Columba,
            RestorationProfileAction::Continue,
        ),
        (
            true,
            false,
            PeerProtocol::Native,
            RestorationProfileAction::Continue,
        ),
        (
            false,
            false,
            PeerProtocol::Native,
            RestorationProfileAction::Continue,
        ),
    ] {
        let mut fixture = fixture(ios, initially_connected);
        assert_eq!(fixture.session.restoration_profile(protocol), Ok(expected));
    }
}

#[tokio::test]
async fn restored_native_reset_discards_old_traffic_and_retains_one_registry_owner() {
    let mut registry = CentralPeerRegistry::new(1, 1);
    assert_eq!(
        registry.admit_restored(peer(), 7_u8),
        RestoredAdmission::Admitted
    );
    for control in greetings() {
        assert!(matches!(
            registry.buffer_restored_control(peer(), control),
            super::central::RestoredBufferResult::Buffered
        ));
    }
    assert!(matches!(
        registry.buffer_restored_data(peer(), Box::from(&[1, 2][..])),
        super::central::RestoredBufferResult::Buffered
    ));
    assert!(registry.offer_restored() == Some(peer()));
    assert!(matches!(
        registry.claim(peer().address()),
        CentralDialCandidate::Ready { restored: true, .. }
    ));
    let callbacks = registry.begin_session(peer()).unwrap().unwrap();
    let mut fixture = fixture(true, true);
    assert!(fixture.session.restore_callbacks(callbacks));
    assert!(fixture.session.enqueue_control(greetings()[0]).is_ok());
    assert!(fixture.session.enqueue_data(Box::from(&[3][..])).is_ok());
    assert_eq!(
        fixture.control.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    );

    assert_eq!(
        fixture.session.restoration_profile(PeerProtocol::Native),
        Ok(RestorationProfileAction::Disconnect)
    );
    assert!(fixture.session.cancellation_pending());
    assert!(fixture.session.enqueue_control(greetings()[1]).is_ok());
    assert!(fixture.session.enqueue_data(Box::from(&[4][..])).is_ok());

    // Keep the exact active owner: no synthetic sighting, replacement, or second claim.
    assert!(registry.offer_restored().is_none());
    assert_eq!(
        registry.admit_restored(peer(), 8),
        RestoredAdmission::AlreadyOwned
    );
    assert!(!registry.observe(peer(), 9, None));
    assert!(matches!(
        registry.claim(peer().address()),
        CentralDialCandidate::Busy
    ));
    assert!(registry.pop_sighting().0.is_none());
    assert_eq!(registry.peripheral_len(), 1);

    assert_eq!(
        fixture.session.restoration_disconnected(),
        RestorationDisconnectAction::Reconnect
    );
    fresh_native_discovery(&mut fixture.session);
    assert_eq!(
        fixture.control.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    );
    assert!(matches!(
        fixture.completion.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let fresh_control = greetings()[1];
    fixture.session.enqueue_control(fresh_control).unwrap();
    fixture.session.enqueue_data(Box::from(&[9][..])).unwrap();
    assert_eq!(fixture.control.try_recv(), Ok(fresh_control));
    assert_eq!(&*receive_data(&mut fixture.data).await, &[9]);
    assert_eq!(registry.drain_owned(), vec![7]);
    assert!(registry.drain_owned().is_empty());
}

#[tokio::test]
async fn columba_and_unaffected_native_paths_preserve_buffered_input() {
    for (ios, initially_connected, protocol) in [
        (true, true, PeerProtocol::Columba),
        (false, true, PeerProtocol::Native),
        (true, false, PeerProtocol::Native),
    ] {
        let mut fixture = fixture(ios, initially_connected);
        let mut callbacks = RestoredCallbackBuffer::default();
        let control = greetings()[0];
        assert!(callbacks.buffer_control(control));
        assert!(callbacks.buffer_data(Box::from(&[1, 2, 3][..])));
        assert!(fixture.session.restore_callbacks(callbacks));
        assert_eq!(
            fixture.session.restoration_profile(protocol),
            Ok(RestorationProfileAction::Continue)
        );
        assert_eq!(fixture.control.try_recv(), Ok(control));
        assert_eq!(&*receive_data(&mut fixture.data).await, &[1, 2, 3]);
        assert!(!fixture.session.cancellation_pending());
        assert_eq!(
            fixture.session.restoration_disconnected(),
            RestorationDisconnectAction::Close
        );
    }
}

#[test]
fn reset_progress_requires_ordered_events_and_reconnects_once() {
    let mut fixture = fixture(true, true);
    assert!(!fixture.session.restoration_native_subscribed());
    assert_eq!(
        fixture.session.restoration_profile(PeerProtocol::Native),
        Ok(RestorationProfileAction::Disconnect)
    );
    assert!(!fixture.session.restoration_connected());
    assert!(!fixture.session.restoration_services_discovered());
    assert!(!fixture.session.restoration_characteristics_expected());
    assert!(!fixture.session.restoration_native_subscribed());
    assert_eq!(
        fixture.session.restoration_profile(PeerProtocol::Native),
        Ok(RestorationProfileAction::Ignore)
    );
    assert_eq!(
        fixture.session.restoration_disconnected(),
        RestorationDisconnectAction::Reconnect
    );
    assert_eq!(
        fixture.session.restoration_disconnected(),
        RestorationDisconnectAction::Ignore
    );
    assert!(!fixture.session.restoration_services_discovered());
    assert!(!fixture.session.restoration_native_subscribed());
    assert!(fixture.session.restoration_connected());
    assert!(!fixture.session.restoration_connected());
    assert!(!fixture.session.restoration_characteristics_expected());
    assert!(fixture.session.restoration_services_discovered());
    assert!(!fixture.session.restoration_services_discovered());
    assert!(fixture.session.restoration_characteristics_expected());
    assert_eq!(
        fixture.session.restoration_profile(PeerProtocol::Native),
        Ok(RestorationProfileAction::Continue)
    );
    assert!(!fixture.session.restoration_characteristics_expected());
    assert!(fixture.session.restoration_native_subscribed());
    assert!(!fixture.session.restoration_native_subscribed());
    assert_eq!(
        fixture.session.restoration_profile(PeerProtocol::Native),
        Ok(RestorationProfileAction::Ignore)
    );
    assert_eq!(
        fixture.session.restoration_disconnected(),
        RestorationDisconnectAction::Close
    );
}

#[test]
fn abandoned_dial_or_data_owner_cannot_reconnect() {
    for (close_completion, disconnected) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let Fixture {
            mut session,
            control: _control,
            mut completion,
            data,
        } = fixture(true, true);
        assert_eq!(
            session.restoration_profile(PeerProtocol::Native),
            Ok(RestorationProfileAction::Disconnect)
        );
        if disconnected {
            assert_eq!(
                session.restoration_disconnected(),
                RestorationDisconnectAction::Reconnect
            );
        }
        if close_completion {
            // The original dial's deadline/abort closes its completion receiver.
            completion.close();
        } else {
            // Stop/owner teardown drops the retained data receiver.
            drop(data);
        }
        assert_eq!(
            session.restoration_disconnected(),
            RestorationDisconnectAction::Close
        );
        assert!(!session.restoration_connected());
        assert!(!session.restoration_native_subscribed());
    }
}

#[test]
fn retiring_recovery_fails_once_and_never_duplicates_an_in_flight_cancel() {
    for phase in 0..3 {
        let mut registry = CentralPeerRegistry::new(1, 1);
        assert_eq!(
            registry.admit_restored(peer(), 7_u8),
            RestoredAdmission::Admitted
        );
        assert!(registry.offer_restored() == Some(peer()));
        assert!(matches!(
            registry.claim(peer().address()),
            CentralDialCandidate::Ready { .. }
        ));
        assert!(registry.begin_session(peer()).is_ok());
        let mut fixture = fixture(true, true);
        let mut old_connection_cancels = 0;
        if phase > 0 {
            assert_eq!(
                fixture.session.restoration_profile(PeerProtocol::Native),
                Ok(RestorationProfileAction::Disconnect)
            );
            old_connection_cancels += 1;
        }
        if phase > 1 {
            assert_eq!(
                fixture.session.restoration_disconnected(),
                RestorationDisconnectAction::Reconnect
            );
        }

        // The same retirement method backs failure/timeout/Stop. Once the owner is
        // removed, repeated cleanup and a late disconnect find no session to restart.
        let cancel_required = fixture.session.retire();
        let removed = registry.remove_peer(peer());
        assert_eq!(removed, Some(7));
        assert_eq!(cancel_required, phase != 1);
        if phase == 0 {
            assert_eq!(old_connection_cancels + usize::from(cancel_required), 1);
        } else {
            assert_eq!(old_connection_cancels, 1);
            // After didDisconnect, this cancellation belongs to the newly requested
            // connection; before it, retirement must not repeat the pending cancel.
            assert_eq!(usize::from(cancel_required), usize::from(phase == 2));
        }
        assert!(matches!(
            fixture.completion.try_recv(),
            Ok(DialCompletion::Failed)
        ));
        assert!(matches!(
            fixture.completion.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
        assert!(registry.remove_peer(peer()).is_none());
        assert!(registry.drain_owned().is_empty());
        assert!(registry.offer_restored().is_none());
        assert!(matches!(
            registry.claim(peer().address()),
            CentralDialCandidate::Missing
        ));
    }
}
