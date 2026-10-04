use super::*;

#[test]
fn running_controls_drain_before_disconnect_and_are_never_automatically_retried() {
    within_runtime(async {
        let (mut registry, connection, mut backend) = fixture();
        let _ended = registry.step(EndConnection {
            connection,
            reason: DisconnectionReason::Cancelled,
        });
        let command =
            DeviceControlCommand::DisplayVisibility(RemoteControlDisplayVisibility::Hidden);
        assert!(backend.permitted.insert(command.request_kind()));
        backend.block_control = true;
        let shared = backend.shared.clone();
        let (updates, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let session =
            prepare_device_session::<4>(registry, connection.device(), backend, move |update| {
                updates.send(update).unwrap();
            })
            .unwrap();
        let handle = session.handle;
        assert_eq!(
            handle.submit(DeviceSessionIntent::Connect).unwrap(),
            DeviceSessionSubmission::Submitted
        );
        let exercise = async {
            loop {
                if matches!(
                    observed.recv().await.unwrap().event,
                    DeviceSessionEvent::InterfacesReceived {
                        outcome: ReceiveInterfacePageOutcome::Complete { .. },
                        ..
                    }
                ) {
                    break;
                }
            }
            assert_eq!(
                handle
                    .submit(DeviceSessionIntent::Control(RequestDeviceControl {
                        command
                    }))
                    .unwrap(),
                DeviceSessionSubmission::Submitted
            );
            shared.control_started.notified().await;
            handle.shutdown().unwrap();
            loop {
                if matches!(
                    observed.recv().await.unwrap().event,
                    DeviceSessionEvent::Disconnected { .. }
                ) {
                    break;
                }
            }
            assert!(!shared.calls.lock().unwrap().contains(&Call::Close(LINK)));
            shared.control_release.notify_one();
        };
        let (exit, ()) = tokio::join!(session.run, exercise);
        assert!(exit.unwrap().settlement.is_ok());
        let calls = shared.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|call| matches!(call, Call::Control(..)))
                .count(),
            1
        );
        assert_eq!(calls.last(), Some(&Call::Close(LINK)));
    });
}

#[test]
fn explicit_cancellations_and_close_remove_only_the_matching_control() {
    let (mut driver, _handle, connection, _work) = driver_fixture();
    let command = DeviceControlCommand::GnssPower(RemoteControlGnssPower::On);
    let request = control_request(&mut driver.registry, connection, command);
    let foreign_device = paired(&mut driver.registry, target(99));
    let foreign = begin(&mut driver.registry, foreign_device);
    let other = control_request(&mut driver.registry, foreign, command);
    let mut route = DeviceSessionSwitchboard::<4>::new(connection.device())
        .route(DeviceSessionInput {
            registry: &mut driver.registry,
            message: DeviceSessionMessage::Inspect,
        })
        .unwrap();
    route.commands = [None, None];
    for cancel_by_close in [false, true] {
        driver.reactor.active = Some(super::super::reactor::ActiveWork {
            input: PrnsDeviceIn::Control { request },
            completion: Box::pin(core::future::pending()),
        });
        driver.queued = [
            Some(PrnsDeviceIn::Control { request }),
            Some(PrnsDeviceIn::Close {
                connection: foreign,
            }),
        ];
        route.cancelled_controls = [Some(other), None];
        driver.dispatch(&route).unwrap();
        assert_eq!(
            driver.reactor.active.as_ref().unwrap().input,
            PrnsDeviceIn::Control { request }
        );
        assert_eq!(
            driver.queued.first().unwrap(),
            &Some(PrnsDeviceIn::Control { request })
        );
        if cancel_by_close {
            route.cancelled_controls = [None, None];
            route.commands = [Some(PrnsDeviceIn::Close { connection }), None];
        } else {
            route.cancelled_controls = [Some(request), None];
        }
        driver.dispatch(&route).unwrap();
        assert_eq!(
            driver.reactor.active.as_ref().unwrap().input,
            PrnsDeviceIn::Close {
                connection: foreign
            }
        );
        assert!(
            !driver
                .queued
                .iter()
                .any(|slot| matches!(slot, Some(PrnsDeviceIn::Control { .. })))
        );
        route.commands = [None, None];
    }
}

#[test]
fn closing_another_connection_preserves_active_and_queued_controls() {
    let (mut driver, _handle, connection, _work) = driver_fixture();
    let command = DeviceControlCommand::GnssPower(RemoteControlGnssPower::Off);
    let request = control_request(&mut driver.registry, connection, command);
    let foreign_device = paired(&mut driver.registry, target(99));
    let foreign = begin(&mut driver.registry, foreign_device);
    let mut route = DeviceSessionSwitchboard::<4>::new(connection.device())
        .route(DeviceSessionInput {
            registry: &mut driver.registry,
            message: DeviceSessionMessage::Inspect,
        })
        .unwrap();
    route.commands = [
        Some(PrnsDeviceIn::Close {
            connection: foreign,
        }),
        None,
    ];
    driver.reactor.active = Some(super::super::reactor::ActiveWork {
        input: PrnsDeviceIn::Control { request },
        completion: Box::pin(core::future::pending()),
    });
    driver.queued = [Some(PrnsDeviceIn::Control { request }), None];
    driver.dispatch(&route).unwrap();
    assert_eq!(
        driver.reactor.active.as_ref().unwrap().input,
        PrnsDeviceIn::Control { request }
    );
    assert_eq!(
        driver.queued,
        [
            Some(PrnsDeviceIn::Control { request }),
            Some(PrnsDeviceIn::Close {
                connection: foreign
            })
        ]
    );
}
