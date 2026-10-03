use super::*;

#[test]
fn only_confirmed_connections_start_inventory_and_repeated_synchronization_preserves_work() {
    let mut registry = registry();
    let device = create(&mut registry);
    let mut interfaces = DeviceInterfaces::<4>::new(device);
    assert_eq!(
        interfaces.step(ReadDeviceInterfaces),
        ReadDeviceInterfacesOutcome::Unavailable { device }
    );
    assert_eq!(
        interfaces.step(RefreshInterfaces),
        Ok(RefreshInterfacesOutcome::Closed)
    );
    assert_eq!(
        interfaces.step(CloseInterfaceInventory),
        CloseInterfaceInventoryOutcome::AlreadyClosed
    );
    assert_eq!(
        interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unavailable { cancelled: None }
    );
    pair(&mut registry, device, 1);
    let connection = begin(&mut registry, device);
    assert_eq!(
        interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unavailable { cancelled: None }
    );
    let rejected = ConfirmConnection {
        connection,
        target: target(2),
        link: FIRST_LINK,
    };
    assert_eq!(
        registry.step(ConfirmConnection {
            connection,
            target: target(2),
            link: FIRST_LINK
        }),
        ConfirmConnectionOutcome::TargetMismatch { rejected }
    );
    assert_eq!(
        interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unavailable { cancelled: None }
    );
    confirm(&mut registry, connection, FIRST_LINK);
    let before_registry = registry.step(ReadDevice { device });
    let first = start(&mut interfaces, &mut registry, connection, FIRST_LINK, None);
    assert_eq!(first.generation.get(), 1);
    assert_eq!(registry.step(ReadDevice { device }), before_registry);
    let before = interfaces.step(ReadDeviceInterfaces);
    assert_eq!(
        interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unchanged {
            connection,
            link: FIRST_LINK
        }
    );
    assert_eq!(interfaces.step(ReadDeviceInterfaces), before);
    assert_eq!(
        interfaces.step(RefreshInterfaces),
        Ok(RefreshInterfacesOutcome::Busy { pending: first })
    );
    let ReceiveInterfacePageOutcome::More { request: next } = interfaces.step(response(
        first,
        &[1],
        RemoteControlInterfaceContinuation::More(RemoteControlInterfaceCursor::after(entry(1).id)),
    )) else {
        panic!("missing continuation");
    };
    assert_eq!(
        interfaces.step(response(
            next,
            &[2],
            RemoteControlInterfaceContinuation::Complete
        )),
        ReceiveInterfacePageOutcome::Complete { count: 2 }
    );
    let complete = interfaces.step(ReadDeviceInterfaces);
    assert_eq!(
        complete,
        ReadDeviceInterfacesOutcome::Found {
            device,
            inventory: InterfaceInventorySnapshot {
                connection,
                status: InterfaceInventoryStatus::Ready,
                interfaces: Some(heapless::Vec::from_slice(&[entry(1), entry(2)]).unwrap())
            },
        }
    );
    assert_eq!(
        interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unchanged {
            connection,
            link: FIRST_LINK
        }
    );
    assert_eq!(interfaces.step(ReadDeviceInterfaces), complete);
    let Ok(RefreshInterfacesOutcome::Requested { request }) = interfaces.step(RefreshInterfaces)
    else {
        panic!("refresh refused");
    };
    assert_eq!(request.generation.get(), 2);
    assert_eq!(
        interfaces.step(failure(request)),
        InterfaceRefreshFailedOutcome::Failed {
            request,
            reason: InterfaceRefreshFailure::TransportLost
        }
    );
    assert_eq!(
        interfaces.step(ReadDeviceInterfaces),
        ReadDeviceInterfacesOutcome::Found {
            device,
            inventory: InterfaceInventorySnapshot {
                connection,
                status: InterfaceInventoryStatus::Failed {
                    reason: InterfaceRefreshFailure::TransportLost
                },
                interfaces: Some(heapless::Vec::from_slice(&[entry(1), entry(2)]).unwrap())
            },
        }
    );
}

#[test]
fn disconnect_and_forget_cancel_pending_pages_without_touching_another_device() {
    let mut registry = registry();
    let first = create(&mut registry);
    let second = create(&mut registry);
    pair(&mut registry, first, 1);
    pair(&mut registry, second, 2);
    let first_connection = begin(&mut registry, first);
    let second_connection = begin(&mut registry, second);
    confirm(&mut registry, first_connection, FIRST_LINK);
    confirm(&mut registry, second_connection, SECOND_LINK);
    let mut first_interfaces = DeviceInterfaces::<4>::new(first);
    let mut second_interfaces = DeviceInterfaces::<4>::new(second);
    let first_request = start(
        &mut first_interfaces,
        &mut registry,
        first_connection,
        FIRST_LINK,
        None,
    );
    let second_request = start(
        &mut second_interfaces,
        &mut registry,
        second_connection,
        SECOND_LINK,
        None,
    );
    let second_before = second_interfaces.step(ReadDeviceInterfaces);
    assert_stale(&mut first_interfaces, second_request);
    assert_stale(&mut second_interfaces, first_request);
    end(&mut registry, first_connection, FIRST_LINK);
    assert_eq!(
        first_interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unavailable {
            cancelled: Some(first_request)
        }
    );
    assert_eq!(
        first_interfaces.step(ReadDeviceInterfaces),
        ReadDeviceInterfacesOutcome::Unavailable { device: first }
    );
    assert_stale(&mut first_interfaces, first_request);
    assert_eq!(
        first_interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unavailable { cancelled: None }
    );
    let connection = begin(&mut registry, first);
    confirm(&mut registry, connection, FIRST_LINK);
    let request = start(
        &mut first_interfaces,
        &mut registry,
        connection,
        FIRST_LINK,
        None,
    );
    assert_stale(&mut first_interfaces, first_request);
    assert_eq!(
        registry.step(ForgetDevice { device: first }),
        ForgetDeviceOutcome::Forgotten {
            device: first,
            enrollment: EnrollmentState::Paired {
                target: *target(1).public_keys()
            },
            connection: ConnectionState::Connected {
                connection,
                link: FIRST_LINK
            },
        }
    );
    assert_eq!(
        first_interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unavailable {
            cancelled: Some(request)
        }
    );
    assert_stale(&mut first_interfaces, request);
    let replacement = create(&mut registry);
    assert_ne!(replacement, first);
    pair(&mut registry, replacement, 1);
    let replacement_connection = begin(&mut registry, replacement);
    confirm(&mut registry, replacement_connection, FIRST_LINK);
    assert_eq!(
        first_interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unavailable { cancelled: None }
    );
    assert_eq!(
        second_interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unchanged {
            connection: second_connection,
            link: SECOND_LINK
        }
    );
    assert_eq!(second_interfaces.step(ReadDeviceInterfaces), second_before);
}

#[test]
fn explicit_close_is_idempotent_and_does_not_recreate_tokens_for_the_same_connection() {
    let mut registry = registry();
    let device = create(&mut registry);
    pair(&mut registry, device, 1);
    let connection = begin(&mut registry, device);
    confirm(&mut registry, connection, FIRST_LINK);
    let mut interfaces = DeviceInterfaces::<4>::new(device);
    let request = start(&mut interfaces, &mut registry, connection, FIRST_LINK, None);
    assert_eq!(
        interfaces.step(CloseInterfaceInventory),
        CloseInterfaceInventoryOutcome::Closed {
            pending: Some(request)
        }
    );
    assert_eq!(
        interfaces.step(CloseInterfaceInventory),
        CloseInterfaceInventoryOutcome::AlreadyClosed
    );
    assert_eq!(
        interfaces.step(RefreshInterfaces),
        Ok(RefreshInterfacesOutcome::Closed)
    );
    assert_stale(&mut interfaces, request);
    assert_eq!(
        interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unchanged {
            connection,
            link: FIRST_LINK
        }
    );
    assert_eq!(
        interfaces.step(ReadDeviceInterfaces),
        ReadDeviceInterfacesOutcome::Found {
            device,
            inventory: InterfaceInventorySnapshot {
                connection,
                status: InterfaceInventoryStatus::Closed,
                interfaces: None
            }
        }
    );
    end(&mut registry, connection, FIRST_LINK);
    assert_eq!(
        interfaces.step(SynchronizeDeviceInterfaces {
            registry: &mut registry
        }),
        SynchronizeDeviceInterfacesOutcome::Unavailable { cancelled: None }
    );
    assert_eq!(
        interfaces.step(ReadDeviceInterfaces),
        ReadDeviceInterfacesOutcome::Unavailable { device }
    );
}
