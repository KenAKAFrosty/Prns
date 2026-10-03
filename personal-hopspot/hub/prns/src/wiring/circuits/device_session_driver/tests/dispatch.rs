use super::*;

fn empty_route(driver: &mut DeviceSessionDriver, connection: Connection) -> DeviceSessionRoute<4> {
    DeviceSessionSwitchboard::<4>::new(connection.device())
        .route(DeviceSessionInput {
            registry: &mut driver.registry,
            message: DeviceSessionMessage::Inspect,
        })
        .unwrap()
}

fn hold(driver: &mut DeviceSessionDriver, input: PrnsDeviceIn) {
    driver.reactor.active = Some(super::super::reactor::ActiveWork {
        input,
        completion: Box::pin(core::future::pending()),
    });
}

#[test]
fn cancellations_remove_only_matching_work_and_preserve_close_order() {
    let (mut driver, _handle, connection, _work) = driver_fixture();
    let foreign_device = paired(&mut driver.registry, target(99));
    let foreign = begin(&mut driver.registry, foreign_device);
    let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
    let (_, other) = InterfaceInventory::<4>::with_initial_refresh(foreign);
    let mut route = empty_route(&mut driver, connection);
    hold(&mut driver, PrnsDeviceIn::Inventory { request });
    driver.queued = [
        Some(PrnsDeviceIn::Inventory { request }),
        Some(PrnsDeviceIn::Close {
            connection: foreign,
        }),
    ];
    route.cancelled = [Some(other), None];
    driver.dispatch(&route).unwrap();
    assert!(
        matches!(driver.reactor.active.as_ref().unwrap().input, PrnsDeviceIn::Inventory { request: active } if active == request)
    );
    assert!(
        matches!(driver.queued.first(), Some(Some(PrnsDeviceIn::Inventory { request: queued })) if *queued == request)
    );
    route.cancelled = [Some(request), None];
    route.commands = [Some(PrnsDeviceIn::Close { connection }), None];
    driver.dispatch(&route).unwrap();
    assert_eq!(
        driver.reactor.active.as_ref().unwrap().input,
        PrnsDeviceIn::Close {
            connection: foreign
        }
    );
    assert_eq!(
        driver.queued,
        [None, Some(PrnsDeviceIn::Close { connection })]
    );
    assert!(!driver.reactor.admit_intents);
    driver.reactor.active = None;
    route.commands = [None, None];
    driver.dispatch(&route).unwrap();
    assert_eq!(
        driver.reactor.active.as_ref().unwrap().input,
        PrnsDeviceIn::Close { connection }
    );
    assert!(driver.reactor.admit_intents);
}

#[test]
fn close_cancels_connect_and_inventory_but_never_another_close_or_connection() {
    let (mut driver, _handle, connection, _work) = driver_fixture();
    let foreign_device = paired(&mut driver.registry, target(99));
    let foreign = begin(&mut driver.registry, foreign_device);
    let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
    let (_, other) = InterfaceInventory::<4>::with_initial_refresh(foreign);
    for (input, cancelled) in [
        (PrnsDeviceIn::Connect { connection }, true),
        (PrnsDeviceIn::Inventory { request }, true),
        (PrnsDeviceIn::Close { connection }, false),
        (
            PrnsDeviceIn::Connect {
                connection: foreign,
            },
            false,
        ),
        (PrnsDeviceIn::Inventory { request: other }, false),
    ] {
        let mut route = empty_route(&mut driver, connection);
        route.commands = [Some(PrnsDeviceIn::Close { connection }), None];
        driver.queued = [Some(super::super::dispatch::duplicate(&input)), None];
        hold(&mut driver, input);
        driver.dispatch(&route).unwrap();
        if cancelled {
            assert_eq!(
                driver.reactor.active.as_ref().unwrap().input,
                PrnsDeviceIn::Close { connection }
            );
            assert_eq!(driver.queued, [None, None]);
        } else {
            assert_eq!(
                driver.queued.first().unwrap().as_ref().unwrap(),
                &driver.reactor.active.as_ref().unwrap().input
            );
            assert_eq!(
                driver.queued.last(),
                Some(&Some(PrnsDeviceIn::Close { connection }))
            );
        }
    }
}

#[test]
fn a_route_that_exceeds_the_bounded_command_slots_reports_the_rejected_command() {
    let (mut driver, _handle, connection, _work) = driver_fixture();
    hold(&mut driver, PrnsDeviceIn::Close { connection });
    driver.queued = [
        Some(PrnsDeviceIn::Close { connection }),
        Some(PrnsDeviceIn::Close { connection }),
    ];
    let mut route = empty_route(&mut driver, connection);
    route.commands = [Some(PrnsDeviceIn::Connect { connection }), None];
    assert!(
        matches!(driver.dispatch(&route), Err(DeviceDriverFailure::CommandCapacity { rejected: PrnsDeviceIn::Connect { connection: rejected } }) if rejected == connection)
    );
    assert_eq!(
        driver.queued,
        [
            Some(PrnsDeviceIn::Close { connection }),
            Some(PrnsDeviceIn::Close { connection })
        ]
    );
}

#[test]
fn command_capacity_invariants_propagate_out_of_the_circuit() {
    let (mut driver, _handle, connection, _work) = driver_fixture();
    hold(&mut driver, PrnsDeviceIn::Close { connection });
    driver.queued = [
        Some(PrnsDeviceIn::Close { connection }),
        Some(PrnsDeviceIn::Close { connection }),
    ];
    driver.reactor.completed = Some(Ok(PrnsDeviceOut::Connected {
        confirmation: ConfirmConnection {
            connection,
            target: connection.target(),
            link: LINK,
        },
    }));
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    assert!(
        matches!(driver.react(&mut board, &DeviceDriverReaction::WorkReady), Err(DeviceDriverFailure::CommandCapacity { rejected: PrnsDeviceIn::Inventory { request } }) if request.connection() == connection)
    );
}
