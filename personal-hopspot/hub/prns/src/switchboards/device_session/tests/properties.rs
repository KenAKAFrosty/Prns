use super::*;
use proptest::prelude::*;

async fn drive(
    board: &mut DeviceSessionSwitchboard<4>,
    registry: &mut DeviceRegistry,
    fitting: &mut PrnsDeviceFitting<Mock>,
    message: DeviceSessionMessage,
) -> DeviceSessionRoute<4> {
    let mut routed = route(board, registry, message);
    let mut commands = alloc::collections::VecDeque::new();
    loop {
        commands.extend(routed.commands.iter_mut().filter_map(Option::take));
        let Some(command) = commands.pop_front() else {
            return routed;
        };
        let output = perform(fitting, command).await;
        routed = route(board, registry, DeviceSessionMessage::Prns(output));
    }
}

proptest! {
    #[test]
    fn arbitrary_sessions_match_connection_ownership_and_reject_retired_callbacks(operations in proptest::collection::vec(0u8..4, 0..32)) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(check_sessions(operations));
    }
}

async fn check_sessions(operations: alloc::vec::Vec<u8>) {
    let (mut registry, initial, backend) = fixture();
    let _ended = registry.step(EndConnection {
        connection: initial,
        reason: DisconnectionReason::Cancelled,
    });
    let other = paired(&mut registry, target(99));
    let other_snapshot = registry.step(ReadDevice { device: other });
    let mut board = DeviceSessionSwitchboard::<4>::new(initial.device());
    let shared = Arc::clone(&backend.shared);
    let mut fitting = PrnsDeviceFitting::new(initial.device(), backend);
    let mut active = None;
    let mut retired = alloc::vec![initial];
    let mut expected = alloc::vec::Vec::new();
    for operation in operations {
        let message = match operation {
            0 => DeviceSessionMessage::Connect,
            1 => DeviceSessionMessage::Refresh,
            2 => DeviceSessionMessage::Disconnect,
            _ => DeviceSessionMessage::Inspect,
        };
        let routed = drive(&mut board, &mut registry, &mut fitting, message).await;
        if operation == 0 && active.is_none() {
            let ReadDeviceOutcome::Found { device } = &routed.snapshot.device else {
                panic!("missing device")
            };
            let ConnectionState::Connected {
                connection,
                link: LINK,
            } = device.connection
            else {
                panic!("not connected")
            };
            active = Some(connection);
            expected.extend([
                Call::Resolve(connection.target().identity_hash()),
                Call::Establish(connection.target().endpoint().destination_hash()),
                Call::Identify(LINK, controller().identity_hash()),
                Call::Inventory(LINK, RemoteControlInterfacePage::First),
                Call::Inventory(
                    LINK,
                    RemoteControlInterfacePage::After(RemoteControlInterfaceCursor::after(
                        InterfaceId::new([1; 8]),
                    )),
                ),
            ]);
        } else if operation == 1 && active.is_some() {
            expected.extend([
                Call::Inventory(LINK, RemoteControlInterfacePage::First),
                Call::Inventory(
                    LINK,
                    RemoteControlInterfacePage::After(RemoteControlInterfaceCursor::after(
                        InterfaceId::new([1; 8]),
                    )),
                ),
            ]);
        } else if operation == 2
            && let Some(connection) = active.take()
        {
            retired.push(connection);
            expected.push(Call::Close(LINK));
        }
        if let Some(connection) = active {
            let ReadDeviceInterfacesOutcome::Found { inventory, .. } = &routed.snapshot.interfaces
            else {
                panic!("inventory missing")
            };
            assert_eq!(inventory.connection, connection);
            assert_eq!(inventory.status, InterfaceInventoryStatus::Ready);
            assert_eq!(inventory.interfaces.as_ref().unwrap().len(), 2);
        } else {
            assert_eq!(
                routed.snapshot.interfaces,
                ReadDeviceInterfacesOutcome::Unavailable {
                    device: initial.device()
                }
            );
        }
        for &connection in &retired {
            let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
            let stale = drive(
                &mut board,
                &mut registry,
                &mut fitting,
                DeviceSessionMessage::Prns(page(request)),
            )
            .await;
            assert_eq!(stale.snapshot, routed.snapshot);
            let stale = drive(
                &mut board,
                &mut registry,
                &mut fitting,
                DeviceSessionMessage::Prns(PrnsDeviceOut::Connected {
                    confirmation: ConfirmConnection {
                        connection,
                        target: connection.target(),
                        link: LINK,
                    },
                }),
            )
            .await;
            assert_eq!(stale.snapshot, routed.snapshot);
        }
        assert_eq!(*shared.calls.lock().unwrap(), expected);
        assert_eq!(registry.step(ReadDevice { device: other }), other_snapshot);
    }
    drop(fitting);
    if active.is_some() {
        expected.push(Call::Close(LINK));
    }
    assert_eq!(*shared.calls.lock().unwrap(), expected);
}
