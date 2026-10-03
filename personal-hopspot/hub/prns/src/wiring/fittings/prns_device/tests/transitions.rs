use super::*;

#[tokio::test]
async fn connected_pages_preserve_tokens_cursors_status_and_rtt() {
    let (mut registry, connection, backend) = fixture();
    let shared = Arc::clone(&backend.shared);
    let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
    {
        let (_, mut outgoing) = fitting.split();
        let _unsent = outgoing.send(PrnsDeviceIn::Connect { connection });
        assert!(shared.calls.lock().unwrap().is_empty());
    }
    let confirmation = connect(&mut fitting, connection).await;
    assert_eq!(confirmation.link, LINK);
    assert_eq!(
        registry.step(confirmation),
        ConfirmConnectionOutcome::Connected {
            connection,
            link: LINK
        }
    );
    let mut interfaces = DeviceInterfaces::<4>::new(connection.device());
    let SynchronizeDeviceInterfacesOutcome::Started {
        request,
        link: LINK,
        cancelled: None,
    } = interfaces.step(SynchronizeDeviceInterfaces {
        registry: &mut registry,
    })
    else {
        panic!("inventory not started")
    };
    assert_eq!(request.page(), RemoteControlInterfacePage::First);
    let PrnsDeviceOut::InterfacesReceived { response, rtt } =
        perform(&mut fitting, PrnsDeviceIn::Inventory { request }).await
    else {
        panic!("missing page")
    };
    assert_eq!(response.request, request);
    assert_eq!(rtt, RttMillis::new(42));
    let ReceiveInterfacePageOutcome::More { request: next } = interfaces.step(response) else {
        panic!("missing continuation")
    };
    let after = RemoteControlInterfacePage::After(RemoteControlInterfaceCursor::after(
        InterfaceId::new([1; 8]),
    ));
    assert_eq!(next.page(), after);
    let PrnsDeviceOut::InterfacesReceived { response, rtt } =
        perform(&mut fitting, PrnsDeviceIn::Inventory { request: next }).await
    else {
        panic!("missing page")
    };
    assert_eq!(response.request, next);
    assert_eq!(rtt, RttMillis::new(42));
    assert_eq!(
        interfaces.step(response),
        ReceiveInterfacePageOutcome::Complete { count: 2 }
    );
    let ReadDeviceInterfacesOutcome::Found { inventory, .. } =
        interfaces.step(ReadDeviceInterfaces)
    else {
        panic!("missing inventory")
    };
    assert_eq!(inventory.status, InterfaceInventoryStatus::Ready);
    assert_eq!(
        inventory
            .interfaces
            .unwrap()
            .iter()
            .map(|e| (e.id, e.tx_bytes, e.rx_bytes))
            .collect::<alloc::vec::Vec<_>>(),
        alloc::vec![
            (InterfaceId::new([1; 8]), 17, 23),
            (InterfaceId::new([2; 8]), 17, 23)
        ]
    );
    assert_eq!(
        perform(&mut fitting, PrnsDeviceIn::Connect { connection }).await,
        PrnsDeviceOut::AlreadyConnected {
            connection,
            link: LINK
        }
    );
    assert_eq!(
        perform(&mut fitting, PrnsDeviceIn::Close { connection }).await,
        PrnsDeviceOut::Closed {
            connection,
            settlement: CloseRemoteControlTargetOutcome::Queued
        }
    );
    drop(fitting);
    assert_eq!(
        *shared.calls.lock().unwrap(),
        alloc::vec![
            Call::Resolve(connection.target().identity_hash()),
            Call::Establish(connection.target().endpoint().destination_hash()),
            Call::Identify(LINK, controller().identity_hash()),
            Call::Inventory(LINK, RemoteControlInterfacePage::First),
            Call::Inventory(LINK, after),
            Call::Close(LINK)
        ]
    );
}

#[tokio::test]
async fn stale_generations_and_foreign_devices_never_touch_the_backend() {
    let (mut registry, connection, backend) = fixture();
    let shared = Arc::clone(&backend.shared);
    let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
    let (_, old_request) = InterfaceInventory::<4>::with_initial_refresh(connection);
    assert_eq!(
        perform(
            &mut fitting,
            PrnsDeviceIn::Inventory {
                request: old_request
            }
        )
        .await,
        PrnsDeviceOut::StaleInventory {
            request: old_request
        }
    );
    assert_eq!(
        perform(&mut fitting, PrnsDeviceIn::Close { connection }).await,
        PrnsDeviceOut::StaleClose { connection }
    );
    assert!(shared.calls.lock().unwrap().is_empty());
    let confirmation = connect(&mut fitting, connection).await;
    assert_eq!(
        registry.step(confirmation),
        ConfirmConnectionOutcome::Connected {
            connection,
            link: LINK
        }
    );
    assert_eq!(
        registry.step(EndConnection {
            connection,
            reason: DisconnectionReason::Cancelled
        }),
        EndConnectionOutcome::SessionEnded {
            connection,
            link: LINK,
            reason: DisconnectionReason::Cancelled
        }
    );
    let newer = begin(&mut registry, connection.device());
    let (_, request) = InterfaceInventory::<4>::with_initial_refresh(newer);
    assert_eq!(
        perform(&mut fitting, PrnsDeviceIn::Connect { connection: newer }).await,
        PrnsDeviceOut::Busy {
            active: connection,
            rejected: newer
        }
    );
    assert_eq!(
        perform(&mut fitting, PrnsDeviceIn::Inventory { request }).await,
        PrnsDeviceOut::StaleInventory { request }
    );
    assert_eq!(
        perform(&mut fitting, PrnsDeviceIn::Close { connection: newer }).await,
        PrnsDeviceOut::StaleClose { connection: newer }
    );
    let foreign_device = paired(&mut registry, target(32));
    let foreign = begin(&mut registry, foreign_device);
    let (_, foreign_request) = InterfaceInventory::<4>::with_initial_refresh(foreign);
    for input in [
        PrnsDeviceIn::Connect {
            connection: foreign,
        },
        PrnsDeviceIn::Inventory {
            request: foreign_request,
        },
        PrnsDeviceIn::Close {
            connection: foreign,
        },
    ] {
        let (mut incoming, mut outgoing) = fitting.split();
        let result = outgoing.send(input).complete().await;
        let ReceiveFromOutcome::Failed {
            failure: PrnsFittingError::WrongDevice { expected, received },
        } = incoming.receive_from(result)
        else {
            panic!("foreign device accepted")
        };
        assert_eq!((expected, received), (connection.device(), foreign_device));
    }
    assert_eq!(shared.calls.lock().unwrap().len(), 3);
    assert_eq!(
        perform(&mut fitting, PrnsDeviceIn::Close { connection }).await,
        PrnsDeviceOut::Closed {
            connection,
            settlement: CloseRemoteControlTargetOutcome::Queued
        }
    );
    connect(&mut fitting, newer).await;
    assert_eq!(
        perform(
            &mut fitting,
            PrnsDeviceIn::Inventory {
                request: old_request
            }
        )
        .await,
        PrnsDeviceOut::StaleInventory {
            request: old_request
        }
    );
    drop(fitting);
    assert_eq!(
        shared
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, Call::Close(LINK)))
            .count(),
        2
    );
}

#[tokio::test]
async fn upstream_failures_remain_typed_and_identification_failure_closes_once() {
    for (failure, expected, count) in [
        (
            Failure::Resolve,
            ConnectRemoteControlTargetError::Resolve(
                ResolveRemoteControlTargetControlError::TargetNotAuthorized,
            ),
            1,
        ),
        (
            Failure::Establish,
            ConnectRemoteControlTargetError::EstablishLink(SendError::Busy),
            2,
        ),
        (
            Failure::Identify,
            ConnectRemoteControlTargetError::Identify(SendError::NodeStopped),
            4,
        ),
    ] {
        let (_, connection, mut backend) = fixture();
        backend.failure = failure;
        let shared = Arc::clone(&backend.shared);
        let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
        assert_eq!(
            perform(&mut fitting, PrnsDeviceIn::Connect { connection }).await,
            PrnsDeviceOut::ConnectionFailed {
                connection,
                source: expected
            }
        );
        assert_eq!(
            perform(&mut fitting, PrnsDeviceIn::Close { connection }).await,
            PrnsDeviceOut::StaleClose { connection }
        );
        drop(fitting);
        assert_eq!(shared.calls.lock().unwrap().len(), count);
        if matches!(failure, Failure::Identify) {
            assert_eq!(
                shared.calls.lock().unwrap().last(),
                Some(&Call::Close(LINK))
            );
        }
    }
}

#[tokio::test]
async fn admission_and_exchange_failures_preserve_request_and_link_ownership() {
    for permitted in [true, false] {
        let (_, connection, mut backend) = fixture();
        backend.failure = Failure::Inventory;
        backend.settlement = CloseRemoteControlTargetOutcome::NotQueued;
        if !permitted {
            backend.permitted = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
        }
        let shared = Arc::clone(&backend.shared);
        let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
        connect(&mut fitting, connection).await;
        let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
        let source = if permitted {
            RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
                SendError::NodeStopped,
            ))
        } else {
            RemoteControlTargetOperationError::NotPermitted(
                RemoteControlRequestKind::InventoryInterfaces,
            )
        };
        assert_eq!(
            perform(&mut fitting, PrnsDeviceIn::Inventory { request }).await,
            PrnsDeviceOut::InterfacesFailed { request, source }
        );
        assert_eq!(
            shared.calls.lock().unwrap().len(),
            if permitted { 4 } else { 3 }
        );
        assert_eq!(
            perform(&mut fitting, PrnsDeviceIn::Close { connection }).await,
            PrnsDeviceOut::Closed {
                connection,
                settlement: CloseRemoteControlTargetOutcome::NotQueued
            }
        );
        assert_eq!(
            perform(&mut fitting, PrnsDeviceIn::Close { connection }).await,
            PrnsDeviceOut::StaleClose { connection }
        );
        drop(fitting);
        assert_eq!(
            shared
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| matches!(c, Call::Close(LINK)))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn unexpected_targets_are_rejected_and_worker_panics_are_invariants() {
    let (_, connection, mut backend) = fixture();
    backend.target = target(99);
    let shared = Arc::clone(&backend.shared);
    let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
    let (_, mut outgoing) = fitting.split();
    let Err(PrnsFittingError::UnexpectedTarget { expected, received }) = outgoing
        .send(PrnsDeviceIn::Connect { connection })
        .complete()
        .await
    else {
        panic!("wrong target accepted")
    };
    assert_eq!(
        (expected, received),
        (
            connection.target().identity_hash(),
            target(99).identity_hash()
        )
    );
    drop(fitting);
    assert_eq!(
        shared.calls.lock().unwrap().last(),
        Some(&Call::Close(LINK))
    );
    let (_, connection, mut backend) = fixture();
    backend.failure = Failure::Panic;
    let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
    let (_, mut outgoing) = fitting.split();
    let Err(PrnsFittingError::WorkerStopped { source }) = outgoing
        .send(PrnsDeviceIn::Connect { connection })
        .complete()
        .await
    else {
        panic!("panic not reported")
    };
    assert!(source.is_panic());
}

#[tokio::test]
async fn abandoned_connect_finishes_identification_and_releases_its_link() {
    let (_, connection, mut backend) = fixture();
    backend.block = true;
    let shared = Arc::clone(&backend.shared);
    let mut fitting = PrnsDeviceFitting::new(connection.device(), backend);
    {
        let (_, mut outgoing) = fitting.split();
        let completion = outgoing
            .send(PrnsDeviceIn::Connect { connection })
            .complete();
        tokio::pin!(completion);
        tokio::select! { biased;
            result = &mut completion => panic!("unexpected completion {result:?}"),
            () = shared.identified.notified() => {}
        }
    }
    assert_eq!(shared.calls.lock().unwrap().len(), 3);
    drop(fitting);
    shared.release.notify_one();
    tokio::time::timeout(core::time::Duration::from_secs(5), shared.closed.notified())
        .await
        .unwrap();
    assert_eq!(
        *shared.calls.lock().unwrap(),
        alloc::vec![
            Call::Resolve(connection.target().identity_hash()),
            Call::Establish(connection.target().endpoint().destination_hash()),
            Call::Identify(LINK, controller().identity_hash()),
            Call::Close(LINK)
        ]
    );
}
