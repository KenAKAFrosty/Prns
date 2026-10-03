use super::*;
use RemoteControlInterfaceContinuation::Complete;

#[test]
fn wire_pages_publish_only_complete_inventory_for_the_connected_device() {
    let (_, connection) = connected(1);
    let mut inventory = InterfaceInventory::<8>::new(connection);
    assert_eq!(
        inventory.step(ReadInterfaces),
        InterfaceInventorySnapshot {
            connection,
            status: InterfaceInventoryStatus::NotRequested,
            interfaces: None
        }
    );
    let first = refresh(&mut inventory);
    assert_eq!(first.connection(), connection);
    assert_eq!(
        first.request(),
        RemoteControlRequest::InventoryInterfaces {
            page: RemoteControlInterfacePage::First
        }
    );
    assert_eq!(
        inventory.step(RefreshInterfaces),
        RefreshInterfacesOutcome::Busy { pending: first }
    );
    let ReceiveInterfacePageOutcome::More { request: second } =
        inventory.step(wire_roundtrip(receive(first, &[1, 2], more(2))))
    else {
        panic!("missing continuation");
    };
    assert_eq!(
        second.request(),
        RemoteControlRequest::InventoryInterfaces {
            page: RemoteControlInterfacePage::After(RemoteControlInterfaceCursor::after(
                entry(2).id
            ))
        }
    );
    assert_eq!(
        inventory.step(ReadInterfaces),
        InterfaceInventorySnapshot {
            connection,
            status: InterfaceInventoryStatus::Receiving { pending: second },
            interfaces: None
        }
    );
    assert_eq!(
        inventory.step(receive(first, &[1, 2], more(2))),
        ReceiveInterfacePageOutcome::StaleRequest {
            rejected: receive(first, &[1, 2], more(2))
        }
    );
    assert_eq!(
        inventory.step(wire_roundtrip(receive(second, &[3], Complete))),
        ReceiveInterfacePageOutcome::Complete { count: 3 }
    );
    assert_eq!(
        inventory.step(ReadInterfaces),
        InterfaceInventorySnapshot {
            connection,
            status: InterfaceInventoryStatus::Ready,
            interfaces: Some(heapless::Vec::from_slice(&[entry(1), entry(2), entry(3)]).unwrap())
        }
    );
    assert_eq!(
        inventory.step(receive(second, &[3], Complete)),
        ReceiveInterfacePageOutcome::StaleRequest {
            rejected: receive(second, &[3], Complete)
        }
    );
}

#[test]
fn failed_refreshes_preserve_last_complete_data_and_reject_old_responses() {
    let (_, connection) = connected(1);
    let mut inventory = InterfaceInventory::<4>::new(connection);
    let original = refresh(&mut inventory);
    assert_eq!(
        inventory.step(receive(original, &[7], Complete)),
        ReceiveInterfacePageOutcome::Complete { count: 1 }
    );
    let previous = inventory.step(ReadInterfaces).interfaces;
    for reason in [
        InterfaceRefreshFailure::TimedOut,
        InterfaceRefreshFailure::TransportLost,
        InterfaceRefreshFailure::PermissionDenied,
        InterfaceRefreshFailure::InvalidResponse,
    ] {
        let first = refresh(&mut inventory);
        let ReceiveInterfacePageOutcome::More { request } =
            inventory.step(receive(first, &[1], more(1)))
        else {
            panic!("missing continuation");
        };
        assert_eq!(
            inventory.step(RefreshInterfaces),
            RefreshInterfacesOutcome::Busy { pending: request }
        );
        assert_eq!(
            inventory.step(fail(first, reason)),
            InterfaceRefreshFailedOutcome::StaleRequest {
                rejected: fail(first, reason)
            }
        );
        assert_eq!(
            inventory.step(fail(request, reason)),
            InterfaceRefreshFailedOutcome::Failed { request, reason }
        );
        assert_eq!(
            inventory.step(ReadInterfaces),
            InterfaceInventorySnapshot {
                connection,
                status: InterfaceInventoryStatus::Failed { reason },
                interfaces: previous.clone()
            }
        );
        assert_eq!(
            inventory.step(fail(request, reason)),
            InterfaceRefreshFailedOutcome::StaleRequest {
                rejected: fail(request, reason)
            }
        );
        assert_eq!(
            inventory.step(receive(request, &[2], Complete)),
            ReceiveInterfacePageOutcome::StaleRequest {
                rejected: receive(request, &[2], Complete)
            }
        );
    }
    let fresh = refresh(&mut inventory);
    assert_ne!(original, fresh);
    assert_eq!(
        inventory.step(receive(original, &[7], Complete)),
        ReceiveInterfacePageOutcome::StaleRequest {
            rejected: receive(original, &[7], Complete)
        }
    );
    assert_eq!(
        inventory.step(receive(fresh, &[], Complete)),
        ReceiveInterfacePageOutcome::Complete { count: 0 }
    );
    assert_eq!(
        inventory.step(ReadInterfaces).interfaces,
        Some(heapless::Vec::new())
    );
}

#[test]
fn duplicate_or_backwards_pages_and_capacity_overflow_never_publish_partial_data() {
    let (_, connection) = connected(1);
    let mut inventory = InterfaceInventory::<2>::new(connection);
    for (id, reason) in [
        (1, InterfaceRefreshFailure::OutOfOrder),
        (2, InterfaceRefreshFailure::OutOfOrder),
        (3, InterfaceRefreshFailure::CapacityExceeded { maximum: 2 }),
    ] {
        let first = refresh(&mut inventory);
        let ReceiveInterfacePageOutcome::More { request } =
            inventory.step(receive(first, &[1, 2], more(2)))
        else {
            panic!("missing continuation");
        };
        assert_eq!(
            inventory.step(receive(first, &[1, 2], more(2))),
            ReceiveInterfacePageOutcome::StaleRequest {
                rejected: receive(first, &[1, 2], more(2))
            }
        );
        assert_eq!(
            inventory.step(receive(request, &[id], Complete)),
            if id == 3 {
                ReceiveInterfacePageOutcome::CapacityExceeded {
                    request,
                    maximum: 2,
                }
            } else {
                ReceiveInterfacePageOutcome::OutOfOrder { request }
            }
        );
        assert_eq!(
            inventory.step(ReadInterfaces),
            InterfaceInventorySnapshot {
                connection,
                status: InterfaceInventoryStatus::Failed { reason },
                interfaces: None
            }
        );
    }
    let request = refresh(&mut inventory);
    assert_eq!(
        inventory.step(receive(request, &[1, 2], Complete)),
        ReceiveInterfacePageOutcome::Complete { count: 2 }
    );
    let request = refresh(&mut inventory);
    assert_eq!(
        inventory.step(receive(request, &[3, 4, 5], Complete)),
        ReceiveInterfacePageOutcome::CapacityExceeded {
            request,
            maximum: 2
        }
    );
    assert_eq!(
        inventory.step(ReadInterfaces).interfaces,
        Some(heapless::Vec::from_slice(&[entry(1), entry(2)]).unwrap())
    );
    let mut zero = InterfaceInventory::<0>::new(connection);
    let request = refresh(&mut zero);
    assert_eq!(
        zero.step(receive(request, &[], Complete)),
        ReceiveInterfacePageOutcome::Complete { count: 0 }
    );
    let request = refresh(&mut zero);
    assert_eq!(
        zero.step(receive(request, &[1], Complete)),
        ReceiveInterfacePageOutcome::CapacityExceeded {
            request,
            maximum: 0
        }
    );
}

#[test]
fn closing_cancels_pending_work_erases_data_and_rejects_reuse() {
    let (mut registry, connection) = connected(1);
    let mut inventory = InterfaceInventory::<4>::new(connection);
    let first = refresh(&mut inventory);
    assert_eq!(
        inventory.step(receive(first, &[1], Complete)),
        ReceiveInterfacePageOutcome::Complete { count: 1 }
    );
    let pending = refresh(&mut inventory);
    assert_eq!(
        registry.step(EndConnection {
            connection,
            reason: DisconnectionReason::TransportLost
        }),
        EndConnectionOutcome::SessionEnded {
            connection,
            link: LinkId::new([1; 16]),
            reason: DisconnectionReason::TransportLost
        }
    );
    assert_eq!(
        inventory.step(CloseInterfaceInventory),
        CloseInterfaceInventoryOutcome::Closed {
            pending: Some(pending)
        }
    );
    assert_eq!(
        inventory.step(ReadInterfaces),
        InterfaceInventorySnapshot {
            connection,
            status: InterfaceInventoryStatus::Closed,
            interfaces: None
        }
    );
    assert_eq!(
        inventory.step(CloseInterfaceInventory),
        CloseInterfaceInventoryOutcome::AlreadyClosed
    );
    assert_eq!(
        inventory.step(RefreshInterfaces),
        RefreshInterfacesOutcome::Closed
    );
    assert_eq!(
        inventory.step(receive(pending, &[2], Complete)),
        ReceiveInterfacePageOutcome::StaleRequest {
            rejected: receive(pending, &[2], Complete)
        }
    );
    let BeginConnectionOutcome::Connect {
        connection: new_connection,
    } = registry.step(BeginConnection {
        device: connection.device(),
    })
    else {
        panic!("reconnect refused");
    };
    assert_eq!(
        registry.step(ConfirmConnection {
            connection: new_connection,
            target: new_connection.target(),
            link: LinkId::new([2; 16])
        }),
        ConfirmConnectionOutcome::Connected {
            connection: new_connection,
            link: LinkId::new([2; 16])
        }
    );
    let mut replacement = InterfaceInventory::<4>::new(new_connection);
    let current = refresh(&mut replacement);
    assert_eq!(
        replacement.step(receive(pending, &[2], Complete)),
        ReceiveInterfacePageOutcome::StaleRequest {
            rejected: receive(pending, &[2], Complete)
        }
    );
    assert_eq!(
        replacement.step(ReadInterfaces).status,
        InterfaceInventoryStatus::Receiving { pending: current }
    );
    let mut idle = InterfaceInventory::<4>::new(connection);
    assert_eq!(
        idle.step(CloseInterfaceInventory),
        CloseInterfaceInventoryOutcome::Closed { pending: None }
    );
}

#[test]
fn refresh_generations_exhaust_without_wrapping_or_losing_published_data() {
    let (_, connection) = connected(1);
    let mut inventory = InterfaceInventory::<4>::new(connection);
    inventory.next_refresh = Some(NonZeroU64::MAX);
    let request = refresh(&mut inventory);
    assert_eq!(request.generation, NonZeroU64::MAX);
    assert_eq!(
        inventory.step(receive(request, &[1], Complete)),
        ReceiveInterfacePageOutcome::Complete { count: 1 }
    );
    let previous = inventory.step(ReadInterfaces);
    assert_eq!(
        inventory.step(RefreshInterfaces),
        RefreshInterfacesOutcome::IdentifiersExhausted
    );
    assert_eq!(inventory.step(ReadInterfaces), previous);
}

#[test]
fn an_empty_terminal_page_completes_a_full_buffer() {
    let (_, connection) = connected(1);
    let mut inventory = InterfaceInventory::<1>::new(connection);
    let first = refresh(&mut inventory);
    let ReceiveInterfacePageOutcome::More { request } =
        inventory.step(receive(first, &[1], more(1)))
    else {
        panic!("missing continuation");
    };
    assert_eq!(
        inventory.step(receive(request, &[], Complete)),
        ReceiveInterfacePageOutcome::Complete { count: 1 }
    );
    assert_eq!(
        inventory.step(ReadInterfaces).interfaces,
        Some(heapless::Vec::from_slice(&[entry(1)]).unwrap())
    );
    assert_eq!(
        inventory.step(CloseInterfaceInventory),
        CloseInterfaceInventoryOutcome::Closed { pending: None }
    );
    assert_eq!(inventory.step(ReadInterfaces).interfaces, None);
}
