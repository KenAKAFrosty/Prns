#![allow(clippy::unwrap_used, clippy::panic)]

mod properties;

use super::*;
use crate::tests::*;
use pipecircuit::Switchboard;

fn route<const N: usize>(
    board: &mut DeviceSessionSwitchboard<N>,
    registry: &mut DeviceRegistry,
    message: DeviceSessionMessage,
) -> DeviceSessionRoute<N> {
    board
        .route(DeviceSessionInput { registry, message })
        .unwrap()
}

fn connected<const N: usize>(
    board: &mut DeviceSessionSwitchboard<N>,
    registry: &mut DeviceRegistry,
    connection: Connection,
) -> InterfacePageRequest {
    let routed = route(
        board,
        registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::Connected {
            confirmation: ConfirmConnection {
                connection,
                target: connection.target(),
                link: LINK,
            },
        }),
    );
    assert_eq!(
        routed.event,
        DeviceSessionEvent::ConnectionConfirmed {
            outcome: ConfirmConnectionOutcome::Connected {
                connection,
                link: LINK
            }
        }
    );
    let [None, Some(PrnsDeviceIn::Inventory { request })] = routed.commands else {
        panic!("initial inventory missing")
    };
    assert_eq!(request.connection(), connection);
    assert_eq!(routed.cancelled, [None, None]);
    request
}

fn page(request: InterfacePageRequest) -> PrnsDeviceOut {
    PrnsDeviceOut::InterfacesReceived {
        response: ReceiveInterfacePage {
            request,
            page: RemoteControlInterfaceInventory::empty(),
        },
        rtt: RttMillis::new(42),
    }
}

#[tokio::test]
async fn connect_drives_pages_to_a_complete_snapshot_and_refresh_reuses_the_link() {
    let (mut registry, connection, backend) = fixture();
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    let shared = Arc::clone(&backend.shared);
    let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
    let first = route(&mut board, &mut registry, DeviceSessionMessage::Connect);
    assert_eq!(
        first.event,
        DeviceSessionEvent::ConnectionRequested {
            outcome: BeginConnectionOutcome::AlreadyConnecting { connection }
        }
    );
    assert_eq!(first.commands, [None, None]);
    let confirmation = connect(&mut fitting, connection).await;
    let mut routed = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::Connected { confirmation }),
    );
    for _ in 0..2 {
        let command = routed.commands.into_iter().flatten().next().unwrap();
        let output = perform(&mut fitting, command).await;
        routed = route(
            &mut board,
            &mut registry,
            DeviceSessionMessage::Prns(output),
        );
    }
    assert_eq!(
        routed.event,
        DeviceSessionEvent::InterfacesReceived {
            outcome: ReceiveInterfacePageOutcome::Complete { count: 2 },
            rtt: RttMillis::new(42)
        }
    );
    assert_eq!(routed.commands, [None, None]);
    let ReadDeviceInterfacesOutcome::Found { inventory, .. } = routed.snapshot.interfaces else {
        panic!("inventory missing")
    };
    assert_eq!(inventory.status, InterfaceInventoryStatus::Ready);
    assert_eq!(inventory.interfaces.unwrap().len(), 2);
    let routed = route(&mut board, &mut registry, DeviceSessionMessage::Connect);
    assert_eq!(
        routed.event,
        DeviceSessionEvent::ConnectionRequested {
            outcome: BeginConnectionOutcome::AlreadyConnected {
                connection,
                link: LINK
            }
        }
    );
    assert_eq!(routed.commands, [None, None]);
    let refresh = route(&mut board, &mut registry, DeviceSessionMessage::Refresh);
    let [Some(PrnsDeviceIn::Inventory { request }), None] = refresh.commands else {
        panic!("refresh not dispatched")
    };
    assert_eq!(
        refresh.event,
        DeviceSessionEvent::RefreshRequested {
            outcome: RefreshInterfacesOutcome::Requested { request }
        }
    );
    let busy = route(&mut board, &mut registry, DeviceSessionMessage::Refresh);
    assert_eq!(
        busy.event,
        DeviceSessionEvent::RefreshRequested {
            outcome: RefreshInterfacesOutcome::Busy { pending: request }
        }
    );
    assert_eq!(busy.commands, [None, None]);
    let disconnected = route(&mut board, &mut registry, DeviceSessionMessage::Disconnect);
    assert_eq!(disconnected.cancelled, [None, Some(request)]);
    assert_eq!(
        disconnected.commands,
        [Some(PrnsDeviceIn::Close { connection }), None]
    );
    assert_eq!(
        disconnected.event,
        DeviceSessionEvent::Disconnected {
            outcome: EndConnectionOutcome::SessionEnded {
                connection,
                link: LINK,
                reason: DisconnectionReason::Cancelled
            }
        }
    );
    let closed = perform(&mut fitting, PrnsDeviceIn::Close { connection }).await;
    let settled = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(closed),
    );
    assert_eq!(settled.commands, [None, None]);
    assert_eq!(
        settled.snapshot.interfaces,
        ReadDeviceInterfacesOutcome::Unavailable {
            device: connection.device()
        }
    );
    assert_eq!(
        shared
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| matches!(call, Call::Establish(_)))
            .count(),
        1
    );
    assert_eq!(
        shared
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| matches!(call, Call::Close(_)))
            .count(),
        1
    );
}

#[test]
fn missing_unpaired_and_unconnected_devices_never_dispatch_transport_work() {
    let mut registry = DeviceRegistry::try_new(NonZeroU32::new(1).unwrap()).unwrap();
    let CreateDeviceOutcome::Created { device } = registry
        .step(CreateDevice {
            label: DeviceLabel::new("MCU").unwrap(),
        })
        .unwrap()
    else {
        panic!("create failed")
    };
    let mut board = DeviceSessionSwitchboard::<4>::new(device);
    assert_eq!(
        route(&mut board, &mut registry, DeviceSessionMessage::Connect).event,
        DeviceSessionEvent::ConnectionRequested {
            outcome: BeginConnectionOutcome::NotPaired { device }
        }
    );
    for message in [
        DeviceSessionMessage::Refresh,
        DeviceSessionMessage::Disconnect,
        DeviceSessionMessage::Inspect,
    ] {
        let routed = route(&mut board, &mut registry, message);
        assert_eq!(routed.commands, [None, None]);
        assert_eq!(routed.cancelled, [None, None]);
        assert_eq!(
            routed.snapshot.interfaces,
            ReadDeviceInterfacesOutcome::Unavailable { device }
        );
    }
    let _forgotten = registry.step(ForgetDevice { device });
    let missing = route(&mut board, &mut registry, DeviceSessionMessage::Connect);
    assert_eq!(
        missing.event,
        DeviceSessionEvent::ConnectionRequested {
            outcome: BeginConnectionOutcome::MissingDevice { device }
        }
    );
    assert_eq!(
        missing.snapshot.device,
        ReadDeviceOutcome::MissingDevice { device }
    );
    assert_eq!(
        route(&mut board, &mut registry, DeviceSessionMessage::Disconnect).event,
        DeviceSessionEvent::Unavailable
    );
}

#[test]
fn connect_and_disconnect_attempts_preserve_tokens_and_duplicate_confirmations_preserve_inventory()
{
    let (mut registry, connection, _) = fixture();
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    let ended = route(&mut board, &mut registry, DeviceSessionMessage::Disconnect);
    assert_eq!(
        ended.event,
        DeviceSessionEvent::Disconnected {
            outcome: EndConnectionOutcome::AttemptEnded {
                connection,
                reason: DisconnectionReason::Cancelled
            }
        }
    );
    assert_eq!(
        ended.commands,
        [Some(PrnsDeviceIn::Close { connection }), None]
    );
    let start = route(&mut board, &mut registry, DeviceSessionMessage::Connect);
    let [Some(PrnsDeviceIn::Connect { connection: newer }), None] = start.commands else {
        panic!("connect missing")
    };
    assert_ne!(newer, connection);
    assert_eq!(
        start.event,
        DeviceSessionEvent::ConnectionRequested {
            outcome: BeginConnectionOutcome::Connect { connection: newer }
        }
    );
    let request = connected(&mut board, &mut registry, newer);
    let complete = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(page(request)),
    );
    let duplicate = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::AlreadyConnected {
            connection: newer,
            link: LINK,
        }),
    );
    assert_eq!(duplicate.commands, [None, None]);
    assert_eq!(duplicate.snapshot, complete.snapshot);
    assert_eq!(
        duplicate.event,
        DeviceSessionEvent::TransportSettled {
            output: PrnsDeviceOut::Connected {
                confirmation: ConfirmConnection {
                    connection: newer,
                    target: newer.target(),
                    link: LINK
                }
            },
            ended: None
        }
    );
    let inspect = route(&mut board, &mut registry, DeviceSessionMessage::Inspect);
    assert_eq!(inspect.event, DeviceSessionEvent::Observed);
    assert_eq!(inspect.snapshot, complete.snapshot);
    let disconnected = route(&mut board, &mut registry, DeviceSessionMessage::Disconnect);
    assert_eq!(disconnected.cancelled, [None, None]);
    assert_eq!(
        route(&mut board, &mut registry, DeviceSessionMessage::Disconnect).event,
        DeviceSessionEvent::Unavailable
    );
}

#[test]
fn late_confirmations_close_only_the_old_token_and_schedule_new_inventory_after_cleanup() {
    let (mut registry, old, _) = fixture();
    let mut board = DeviceSessionSwitchboard::<4>::new(old.device());
    let old_request = connected(&mut board, &mut registry, old);
    let _ended = registry.step(EndConnection {
        connection: old,
        reason: DisconnectionReason::TransportLost,
    });
    let newer = begin(&mut registry, old.device());
    let _confirmed = registry.step(ConfirmConnection {
        connection: newer,
        target: newer.target(),
        link: LINK,
    });
    let late = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::Connected {
            confirmation: ConfirmConnection {
                connection: old,
                target: old.target(),
                link: LINK,
            },
        }),
    );
    let [
        Some(PrnsDeviceIn::Close { connection }),
        Some(PrnsDeviceIn::Inventory { request }),
    ] = late.commands
    else {
        panic!("cleanup must precede new inventory")
    };
    assert_eq!(connection, old);
    assert_eq!(request.connection(), newer);
    assert_eq!(late.cancelled, [Some(old_request), None]);
    assert!(matches!(
        late.event,
        DeviceSessionEvent::ConnectionConfirmed {
            outcome: ConfirmConnectionOutcome::StaleConnection { .. }
        }
    ));
    let stale_page = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(page(old_request)),
    );
    assert_eq!(stale_page.commands, [None, None]);
    assert_eq!(stale_page.snapshot, late.snapshot);
    let _forgotten = registry.step(ForgetDevice {
        device: old.device(),
    });
    let forgotten = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::Connected {
            confirmation: ConfirmConnection {
                connection: newer,
                target: newer.target(),
                link: LINK,
            },
        }),
    );
    assert_eq!(
        forgotten.commands,
        [Some(PrnsDeviceIn::Close { connection: newer }), None]
    );
    assert_eq!(forgotten.cancelled, [Some(request), None]);
    assert!(matches!(
        forgotten.event,
        DeviceSessionEvent::ConnectionConfirmed {
            outcome: ConfirmConnectionOutcome::MissingDevice { .. }
        }
    ));
}

#[test]
fn synchronization_can_start_inventory_without_duplicating_or_dispatching_cancelled_requests() {
    for message in [
        DeviceSessionMessage::Refresh,
        DeviceSessionMessage::Inspect,
        DeviceSessionMessage::Disconnect,
    ] {
        let (mut registry, connection, _) = fixture();
        let _confirmed = registry.step(ConfirmConnection {
            connection,
            target: connection.target(),
            link: LINK,
        });
        let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
        let routed = route(&mut board, &mut registry, message);
        assert_eq!(routed.commands.iter().flatten().count(), 1);
        match routed.event {
            DeviceSessionEvent::Disconnected { .. } => {
                assert_eq!(
                    routed.commands,
                    [Some(PrnsDeviceIn::Close { connection }), None]
                );
                assert!(routed.cancelled.last().unwrap().is_some());
            }
            DeviceSessionEvent::RefreshRequested {
                outcome: RefreshInterfacesOutcome::Requested { request },
            } => assert_eq!(
                routed.commands,
                [Some(PrnsDeviceIn::Inventory { request }), None]
            ),
            DeviceSessionEvent::Observed => assert!(matches!(
                routed.commands,
                [None, Some(PrnsDeviceIn::Inventory { .. })]
            )),
            other @ (DeviceSessionEvent::ConnectionRequested { .. }
            | DeviceSessionEvent::ConnectionConfirmed { .. }
            | DeviceSessionEvent::ConnectionFailed { .. }
            | DeviceSessionEvent::RefreshRequested { .. }
            | DeviceSessionEvent::InterfacesReceived { .. }
            | DeviceSessionEvent::InterfacesFailed { .. }
            | DeviceSessionEvent::Unavailable
            | DeviceSessionEvent::TransportSettled { .. }) => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn typed_failures_settle_the_core_without_erasing_last_complete_inventory() {
    let (mut registry, connection, _) = fixture();
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    let failed = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::ConnectionFailed {
            connection,
            source: ConnectRemoteControlTargetError::Resolve(
                ResolveRemoteControlTargetControlError::Busy,
            ),
        }),
    );
    assert_eq!(
        failed.event,
        DeviceSessionEvent::ConnectionFailed {
            connection,
            source: ConnectRemoteControlTargetError::Resolve(
                ResolveRemoteControlTargetControlError::Busy
            ),
            outcome: EndConnectionOutcome::AttemptEnded {
                connection,
                reason: DisconnectionReason::ConnectionFailed
            }
        }
    );
    assert_eq!(failed.commands, [None, None]);
    let newer = begin(&mut registry, connection.device());
    let request = connected(&mut board, &mut registry, newer);
    let complete = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(page(request)),
    );
    let refresh = route(&mut board, &mut registry, DeviceSessionMessage::Refresh);
    let [Some(PrnsDeviceIn::Inventory { request }), None] = refresh.commands else {
        panic!("missing refresh")
    };
    let failed = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::InterfacesFailed {
            request,
            source: RemoteControlTargetOperationError::NotPermitted(
                RemoteControlRequestKind::InventoryInterfaces,
            ),
        }),
    );
    assert_eq!(
        failed.event,
        DeviceSessionEvent::InterfacesFailed {
            source: RemoteControlTargetOperationError::NotPermitted(
                RemoteControlRequestKind::InventoryInterfaces
            ),
            outcome: InterfaceRefreshFailedOutcome::Failed {
                request,
                reason: InterfaceRefreshFailure::RequestFailed
            }
        }
    );
    let ReadDeviceInterfacesOutcome::Found {
        inventory: complete,
        ..
    } = complete.snapshot.interfaces
    else {
        panic!("missing snapshot")
    };
    let ReadDeviceInterfacesOutcome::Found {
        inventory: failed, ..
    } = failed.snapshot.interfaces
    else {
        panic!("missing snapshot")
    };
    assert_eq!(failed.interfaces, complete.interfaces);
    assert_eq!(
        failed.status,
        InterfaceInventoryStatus::Failed {
            reason: InterfaceRefreshFailure::RequestFailed
        }
    );
}

#[test]
fn missing_links_and_busy_fittings_settle_only_matching_connections() {
    let (mut registry, old, _) = fixture();
    let mut board = DeviceSessionSwitchboard::<4>::new(old.device());
    let request = connected(&mut board, &mut registry, old);
    let missing = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::StaleInventory { request }),
    );
    assert_eq!(missing.cancelled, [None, Some(request)]);
    assert_eq!(
        missing.snapshot.interfaces,
        ReadDeviceInterfacesOutcome::Unavailable {
            device: old.device()
        }
    );
    let newer = begin(&mut registry, old.device());
    let busy = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::Busy {
            active: old,
            rejected: newer,
        }),
    );
    assert_eq!(
        busy.commands,
        [Some(PrnsDeviceIn::Close { connection: old }), None]
    );
    assert_eq!(
        busy.event,
        DeviceSessionEvent::TransportSettled {
            output: PrnsDeviceOut::Busy {
                active: old,
                rejected: newer
            },
            ended: Some(EndConnectionOutcome::AttemptEnded {
                connection: newer,
                reason: DisconnectionReason::ConnectionFailed
            })
        }
    );
    let newest = begin(&mut registry, old.device());
    connected(&mut board, &mut registry, newest);
    let busy = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::Busy {
            active: newest,
            rejected: old,
        }),
    );
    assert_eq!(busy.commands, [None, None]);
    let closed = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::StaleClose { connection: old }),
    );
    assert_eq!(closed.snapshot, busy.snapshot);
    let closed = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::StaleClose { connection: newest }),
    );
    assert_eq!(
        closed.snapshot.interfaces,
        ReadDeviceInterfacesOutcome::Unavailable {
            device: old.device()
        }
    );
}

#[test]
fn mismatched_targets_are_closed_and_foreign_callbacks_preserve_both_devices() {
    let (mut registry, connection, _) = fixture();
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    let wrong = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::Connected {
            confirmation: ConfirmConnection {
                connection,
                target: target(99),
                link: LINK,
            },
        }),
    );
    assert!(matches!(
        wrong.event,
        DeviceSessionEvent::ConnectionConfirmed {
            outcome: ConfirmConnectionOutcome::TargetMismatch { .. }
        }
    ));
    assert_eq!(
        wrong.commands,
        [Some(PrnsDeviceIn::Close { connection }), None]
    );
    let ReadDeviceOutcome::Found { device } = wrong.snapshot.device else {
        panic!("missing device")
    };
    assert_eq!(
        device.connection,
        ConnectionState::Disconnected {
            connection,
            reason: DisconnectionReason::AuthenticationFailed
        }
    );
    let foreign_device = paired(&mut registry, target(99));
    let foreign = begin(&mut registry, foreign_device);
    let (_, request) = InterfaceInventory::<4>::with_initial_refresh(foreign);
    let before = route(&mut board, &mut registry, DeviceSessionMessage::Inspect).snapshot;
    let other_before = registry.step(ReadDevice {
        device: foreign_device,
    });
    for output in [
        PrnsDeviceOut::Connected {
            confirmation: ConfirmConnection {
                connection: foreign,
                target: foreign.target(),
                link: LINK,
            },
        },
        PrnsDeviceOut::AlreadyConnected {
            connection: foreign,
            link: LINK,
        },
        PrnsDeviceOut::ConnectionFailed {
            connection: foreign,
            source: ConnectRemoteControlTargetError::Resolve(
                ResolveRemoteControlTargetControlError::Busy,
            ),
        },
        page(request),
        PrnsDeviceOut::InterfacesFailed {
            request,
            source: RemoteControlTargetOperationError::NotPermitted(
                RemoteControlRequestKind::InventoryInterfaces,
            ),
        },
        PrnsDeviceOut::StaleInventory { request },
        PrnsDeviceOut::Closed {
            connection: foreign,
            settlement: CloseRemoteControlTargetOutcome::Queued,
        },
        PrnsDeviceOut::StaleClose {
            connection: foreign,
        },
        PrnsDeviceOut::Busy {
            active: foreign,
            rejected: connection,
        },
        PrnsDeviceOut::Busy {
            active: connection,
            rejected: foreign,
        },
    ] {
        assert_eq!(
            board.route(DeviceSessionInput {
                registry: &mut registry,
                message: DeviceSessionMessage::Prns(output)
            }),
            Err(DeviceSessionRoutingError::WrongDevice {
                expected: connection.device(),
                received: foreign_device
            })
        );
        assert_eq!(
            route(&mut board, &mut registry, DeviceSessionMessage::Inspect).snapshot,
            before
        );
        assert_eq!(
            registry.step(ReadDevice {
                device: foreign_device
            }),
            other_before
        );
    }
}

#[test]
fn exhausted_generation_errors_remain_precise_invariants() {
    let (_, connection, _) = fixture();
    assert_eq!(
        routing::connection_route(Err(BeginConnectionError::IdentifiersExhausted {
            device: connection.device()
        })),
        Err(DeviceSessionRoutingError::BeginConnection(
            BeginConnectionError::IdentifiersExhausted {
                device: connection.device()
            }
        ))
    );
    assert_eq!(
        routing::refresh_route(Err(RefreshInterfacesError::IdentifiersExhausted)),
        Err(DeviceSessionRoutingError::RefreshInterfaces(
            RefreshInterfacesError::IdentifiersExhausted
        ))
    );
}

#[test]
fn malformed_and_oversized_pages_stop_dispatch_and_wrong_target_duplicates_are_not_accepted() {
    for capacity in [false, true] {
        let (mut registry, connection, _) = fixture();
        let mut board = DeviceSessionSwitchboard::<1>::new(connection.device());
        let request = connected(&mut board, &mut registry, connection);
        let mut inventory = RemoteControlInterfaceInventory::empty();
        let entry = RemoteControlInterfaceEntry {
            id: InterfaceId::new([1; 8]),
            kind: personal_rns::interfaces::InterfaceKind::Loopback,
            mode: personal_rns::interfaces::InterfaceMode::Full,
            connection: personal_rns::interfaces::ConnectionState::Connected,
            enabled: true,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            rate_bytes_per_sec: None,
        };
        inventory.push(entry).unwrap();
        inventory
            .set_continuation(RemoteControlInterfaceContinuation::More(
                RemoteControlInterfaceCursor::after(entry.id),
            ))
            .unwrap();
        let first = route(
            &mut board,
            &mut registry,
            DeviceSessionMessage::Prns(PrnsDeviceOut::InterfacesReceived {
                response: ReceiveInterfacePage {
                    request,
                    page: inventory,
                },
                rtt: RttMillis::new(1),
            }),
        );
        let [Some(PrnsDeviceIn::Inventory { request }), None] = first.commands else {
            panic!("continuation missing")
        };
        let mut inventory = RemoteControlInterfaceInventory::empty();
        inventory
            .push(RemoteControlInterfaceEntry {
                id: if capacity {
                    InterfaceId::new([2; 8])
                } else {
                    entry.id
                },
                ..entry
            })
            .unwrap();
        let rejected = route(
            &mut board,
            &mut registry,
            DeviceSessionMessage::Prns(PrnsDeviceOut::InterfacesReceived {
                response: ReceiveInterfacePage {
                    request,
                    page: inventory,
                },
                rtt: RttMillis::new(1),
            }),
        );
        assert_eq!(rejected.commands, [None, None]);
        assert_eq!(
            rejected.event,
            DeviceSessionEvent::InterfacesReceived {
                outcome: if capacity {
                    ReceiveInterfacePageOutcome::CapacityExceeded {
                        request,
                        maximum: 1,
                    }
                } else {
                    ReceiveInterfacePageOutcome::OutOfOrder { request }
                },
                rtt: RttMillis::new(1)
            }
        );
        let duplicate = route(
            &mut board,
            &mut registry,
            DeviceSessionMessage::Prns(PrnsDeviceOut::Connected {
                confirmation: ConfirmConnection {
                    connection,
                    target: target(99),
                    link: LINK,
                },
            }),
        );
        assert_eq!(
            duplicate.commands,
            [Some(PrnsDeviceIn::Close { connection }), None]
        );
        assert!(matches!(
            duplicate.event,
            DeviceSessionEvent::ConnectionConfirmed {
                outcome: ConfirmConnectionOutcome::StaleConnection { .. }
            }
        ));
    }
}

#[test]
fn already_settled_initial_inventory_is_not_dispatched_again() {
    let (mut registry, connection, _) = fixture();
    let _confirmed = registry.step(ConfirmConnection {
        connection,
        target: connection.target(),
        link: LINK,
    });
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
    let routed = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(page(request)),
    );
    assert_eq!(routed.commands, [None, None]);
    assert_eq!(
        routed.event,
        DeviceSessionEvent::InterfacesReceived {
            outcome: ReceiveInterfacePageOutcome::Complete { count: 0 },
            rtt: RttMillis::new(42)
        }
    );
}
