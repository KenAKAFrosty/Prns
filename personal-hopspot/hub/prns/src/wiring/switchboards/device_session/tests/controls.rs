use super::*;

const COMMAND: DeviceControlCommand = DeviceControlCommand::GnssPower(RemoteControlGnssPower::On);

#[test]
fn commands_settle_once_without_claiming_observed_device_state() {
    let (mut registry, connection, _) = fixture();
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    let unavailable = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Control(RequestDeviceControl { command: COMMAND }),
    );
    assert_eq!(
        unavailable.event,
        DeviceSessionEvent::ControlRequested {
            outcome: RequestDeviceControlOutcome::NotConnected
        }
    );
    let _page = connected(&mut board, &mut registry, connection);
    let requested = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Control(RequestDeviceControl { command: COMMAND }),
    );
    let [Some(PrnsDeviceIn::Control { request }), None] = requested.commands else {
        panic!("control not issued")
    };
    assert_eq!(requested.snapshot.controls.pending, Some(request));
    assert_eq!(request.command(), COMMAND);
    assert_eq!(request.connection(), connection);
    let busy = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Control(RequestDeviceControl { command: COMMAND }),
    );
    assert_eq!(
        busy.event,
        DeviceSessionEvent::ControlRequested {
            outcome: RequestDeviceControlOutcome::Busy { pending: request }
        }
    );
    assert_eq!(busy.commands, [None, None]);
    let expected = PrnsDeviceOut::ControlAcknowledged {
        request,
        outcome: RemoteControlApplyOutcome::Scheduled,
        rtt: RttMillis::new(9),
    };
    let settled = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(expected),
    );
    assert_eq!(
        settled.event,
        DeviceSessionEvent::ControlSettled {
            settlement: SettleDeviceControlOutcome::Settled { request },
            output: PrnsDeviceOut::ControlAcknowledged {
                request,
                outcome: RemoteControlApplyOutcome::Scheduled,
                rtt: RttMillis::new(9)
            },
        }
    );
    assert_eq!(settled.snapshot.controls.pending, None);
    assert_eq!(settled.snapshot.interfaces, requested.snapshot.interfaces);
    let stale = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::StaleControl { request }),
    );
    assert_eq!(
        stale.event,
        DeviceSessionEvent::ControlSettled {
            settlement: SettleDeviceControlOutcome::StaleRequest { request },
            output: PrnsDeviceOut::StaleControl { request }
        }
    );
    assert_eq!(stale.commands, [None, None]);
    assert_eq!(
        super::super::routing::control_route(Err(RequestDeviceControlError::IdentifiersExhausted)),
        Err(DeviceSessionRoutingError::DeviceControls(
            RequestDeviceControlError::IdentifiersExhausted
        ))
    );
}

#[test]
fn disconnected_and_replaced_sessions_cancel_pending_controls_and_reject_late_results() {
    let (mut registry, connection, _) = fixture();
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    let _page = connected(&mut board, &mut registry, connection);
    let requested = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Control(RequestDeviceControl { command: COMMAND }),
    );
    let request = requested.snapshot.controls.pending.unwrap();
    let disconnected = route(&mut board, &mut registry, DeviceSessionMessage::Disconnect);
    assert_eq!(disconnected.cancelled_controls, [None, Some(request)]);
    assert_eq!(
        disconnected.snapshot.controls,
        DeviceControlsSnapshot {
            connection: None,
            pending: None
        }
    );
    let newer = begin(&mut registry, connection.device());
    let _page = connected(&mut board, &mut registry, newer);
    let requested = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Control(RequestDeviceControl { command: COMMAND }),
    );
    let next = requested.snapshot.controls.pending.unwrap();
    let late = route(
        &mut board,
        &mut registry,
        DeviceSessionMessage::Prns(PrnsDeviceOut::ControlUnconfirmed {
            request,
            source: RemoteControlTargetOperationError::NotPermitted(COMMAND.request_kind()),
        }),
    );
    assert_eq!(late.snapshot.controls.pending, Some(next));
    assert!(
        matches!(late.event, DeviceSessionEvent::ControlSettled { settlement: SettleDeviceControlOutcome::StaleRequest { request: stale }, .. } if stale == request)
    );
    let _ended = registry.step(EndConnection {
        connection: newer,
        reason: DisconnectionReason::TransportLost,
    });
    let inspected = route(&mut board, &mut registry, DeviceSessionMessage::Inspect);
    assert_eq!(inspected.cancelled_controls, [Some(next), None]);
    let foreign_device = paired(&mut registry, target(99));
    let foreign = begin(&mut registry, foreign_device);
    let request = control_request(&mut registry, foreign, COMMAND);
    for output in [
        PrnsDeviceOut::StaleControl { request },
        PrnsDeviceOut::ControlAcknowledged {
            request,
            outcome: RemoteControlApplyOutcome::Applied,
            rtt: RttMillis::new(1),
        },
        PrnsDeviceOut::ControlUnconfirmed {
            request,
            source: RemoteControlTargetOperationError::NotPermitted(COMMAND.request_kind()),
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
    }
}
