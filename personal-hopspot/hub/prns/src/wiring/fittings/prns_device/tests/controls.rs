use super::*;

#[test]
fn controls_require_the_exact_connection_and_permission_before_transmitting() {
    within_runtime(async {
        let (mut registry, connection, backend) = fixture();
        let command =
            DeviceControlCommand::DisplayVisibility(RemoteControlDisplayVisibility::Hidden);
        let request = control_request(&mut registry, connection, command);
        let shared = backend.shared.clone();
        let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
        assert_eq!(
            perform(&mut fitting, PrnsDeviceIn::Control { request }).await,
            PrnsDeviceOut::StaleControl { request }
        );
        let _confirmed = connect(&mut fitting, connection).await;
        assert_eq!(
            perform(&mut fitting, PrnsDeviceIn::Control { request }).await,
            PrnsDeviceOut::ControlUnconfirmed {
                request,
                source: RemoteControlTargetOperationError::NotPermitted(
                    RemoteControlRequestKind::SetDisplayVisibility
                ),
            }
        );
        assert!(
            !shared
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| matches!(call, Call::Control(..)))
        );
        let _ended = registry.step(EndConnection {
            connection,
            reason: DisconnectionReason::Cancelled,
        });
        let newer = begin(&mut registry, connection.device());
        let stale = control_request(&mut registry, newer, command);
        assert_eq!(
            perform(&mut fitting, PrnsDeviceIn::Control { request: stale }).await,
            PrnsDeviceOut::StaleControl { request: stale }
        );
        let foreign_device = paired(&mut registry, target(90));
        let foreign = begin(&mut registry, foreign_device);
        let foreign_request = control_request(&mut registry, foreign, command);
        let (_, mut outgoing) = fitting.split();
        assert!(
            matches!(outgoing.send(PrnsDeviceIn::Control { request: foreign_request }).complete().await,
            Err(PrnsFittingError::WrongDevice { expected, received }) if expected == connection.device() && received == foreign_device)
        );
    });
}

#[test]
fn command_acknowledgments_and_uncertain_exchanges_remain_distinct() {
    for command in [
        DeviceControlCommand::DisplayVisibility(RemoteControlDisplayVisibility::Visible),
        DeviceControlCommand::GnssPower(RemoteControlGnssPower::Off),
    ] {
        for outcome in [
            RemoteControlApplyOutcome::Applied,
            RemoteControlApplyOutcome::Unchanged,
            RemoteControlApplyOutcome::Scheduled,
        ] {
            for fails in [false, true] {
                within_runtime(async {
                    let (mut registry, connection, mut backend) = fixture();
                    backend.permitted = RemoteControlRequestSet::only(command.request_kind());
                    backend.control_outcome = outcome;
                    backend.failure = if fails {
                        Failure::Control
                    } else {
                        Failure::None
                    };
                    let shared = backend.shared.clone();
                    let request = control_request(&mut registry, connection, command);
                    let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
                    let _confirmed = connect(&mut fitting, connection).await;
                    let result = perform(&mut fitting, PrnsDeviceIn::Control { request }).await;
                    assert_eq!(
                        result,
                        if fails {
                            PrnsDeviceOut::ControlUnconfirmed {
                                request,
                                source: RemoteControlTargetOperationError::Exchange(
                                    RemoteControlError::Request(SendError::NodeStopped),
                                ),
                            }
                        } else {
                            PrnsDeviceOut::ControlAcknowledged {
                                request,
                                outcome,
                                rtt: RttMillis::new(43),
                            }
                        }
                    );
                    assert_eq!(
                        shared.calls.lock().unwrap().last(),
                        Some(&Call::Control(LINK, command))
                    );
                });
            }
        }
    }
}
