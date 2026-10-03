use super::*;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, max_shrink_time: 1_000, ..ProptestConfig::default() })]
    #[test]
    fn arbitrary_intent_histories_match_physical_connection_and_inventory_ownership(actions in prop::collection::vec(0u8..4, 0..30)) {
        within_runtime(async move {
            let (mut registry, connection, backend) = fixture();
            let _ended = registry.step(EndConnection { connection, reason: DisconnectionReason::Cancelled });
            let shared = backend.shared.clone();
            let (updates, mut observed) = tokio::sync::mpsc::unbounded_channel();
            let session = prepare_device_session::<4>(registry, connection.device(), backend, move |update| { updates.send(update).unwrap(); }).unwrap();
            let handle = session.handle;
            let exercise = async {
                let mut connected = false;
                let mut expected = alloc::vec::Vec::new();
                for action in actions {
                    let (intent, inventory) = match action {
                        0 => {
                            let inventory = !connected;
                            if !connected {
                                expected.extend([Call::Resolve(connection.target().identity_hash()), Call::Establish(connection.target().endpoint().destination_hash()), Call::Identify(LINK, controller().identity_hash())]);
                                connected = true;
                            }
                            (DeviceSessionIntent::Connect, inventory)
                        },
                        1 => (DeviceSessionIntent::Refresh, connected),
                        2 => {
                            if connected { expected.push(Call::Close(LINK)); connected = false; }
                            (DeviceSessionIntent::Disconnect, false)
                        },
                        _ => (DeviceSessionIntent::Inspect, false),
                    };
                    assert_eq!(handle.submit(intent).unwrap(), DeviceSessionSubmission::Submitted);
                    loop {
                        let event = observed.recv().await.unwrap().event;
                        let settled = match action {
                            0 => matches!(event, DeviceSessionEvent::ConnectionRequested { .. }),
                            1 => matches!(event, DeviceSessionEvent::RefreshRequested { .. } | DeviceSessionEvent::Unavailable),
                            2 => matches!(event, DeviceSessionEvent::Disconnected { .. } | DeviceSessionEvent::Unavailable),
                            _ => matches!(event, DeviceSessionEvent::Observed),
                        };
                        if settled { break; }
                    }
                    if inventory {
                        expected.extend([Call::Inventory(LINK, RemoteControlInterfacePage::First), Call::Inventory(LINK, RemoteControlInterfacePage::After(RemoteControlInterfaceCursor::after(InterfaceId::new([1; 8]))))]);
                        loop {
                            let update = observed.recv().await.unwrap();
                            if matches!(update.event, DeviceSessionEvent::InterfacesReceived { outcome: ReceiveInterfacePageOutcome::Complete { count: 2 }, .. }) { break; }
                        }
                    }
                }
                if connected { expected.push(Call::Close(LINK)); }
                drop(handle);
                expected
            };
            let (exit, expected) = tokio::join!(session.run, exercise);
            let mut exit = exit.unwrap();
            assert!(exit.settlement.is_ok());
            let ReadDeviceOutcome::Found { device } = exit.registry.step(ReadDevice { device: connection.device() }) else { panic!("remembered device lost") };
            assert!(matches!(device.connection, ConnectionState::Disconnected { .. }));
            assert_eq!(*shared.calls.lock().unwrap(), expected);
        });
    }
}
