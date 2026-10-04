#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::tests::*;
use core::{num::NonZeroUsize, time::Duration};

#[test]
fn automatic_connection_pages_refresh_and_shutdown_preserve_the_registry() {
    within_runtime(async {
        let (mut registry, old, backend) = fixture();
        let _ended = registry.step(EndConnection {
            connection: old,
            reason: DisconnectionReason::Cancelled,
        });
        let shared = backend.shared.clone();
        let (updates, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let session = prepare_device_session::<4>(registry, old.device(), backend, move |update| {
            updates.send(update).unwrap();
        })
        .unwrap();
        assert_eq!(
            session.handle.submit(DeviceSessionIntent::Connect).unwrap(),
            DeviceSessionSubmission::Submitted
        );
        let handle = session.handle;
        let exercise = async {
            loop {
                let update = observed.recv().await.unwrap();
                if matches!(
                    update.event,
                    DeviceSessionEvent::InterfacesReceived {
                        outcome: ReceiveInterfacePageOutcome::Complete { count: 2 },
                        ..
                    }
                ) {
                    break;
                }
            }
            assert_eq!(
                handle.submit(DeviceSessionIntent::Refresh).unwrap(),
                DeviceSessionSubmission::Submitted
            );
            loop {
                let update = observed.recv().await.unwrap();
                if matches!(
                    update.event,
                    DeviceSessionEvent::InterfacesReceived {
                        outcome: ReceiveInterfacePageOutcome::Complete { count: 2 },
                        ..
                    }
                ) {
                    break;
                }
            }
            handle.shutdown().unwrap();
            assert_eq!(
                handle.submit(DeviceSessionIntent::Connect).unwrap(),
                DeviceSessionSubmission::Stopped {
                    intent: DeviceSessionIntent::Connect
                }
            );
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(session.run, exercise)
        })
        .await
        .unwrap();
        let mut exit = result.unwrap();
        assert!(exit.settlement.is_ok());
        assert!(
            matches!(exit.registry.step(ReadDevice { device: old.device() }), ReadDeviceOutcome::Found { device } if matches!(device.connection, ConnectionState::Disconnected { .. }))
        );
        assert_eq!(
            shared
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| matches!(call, Call::Inventory(..)))
                .count(),
            4
        );
        assert_eq!(
            shared.calls.lock().unwrap().last(),
            Some(&Call::Close(LINK))
        );
    });
}

mod dispatch;
mod properties;
mod reactor;

fn driver_fixture() -> (
    DeviceSessionDriver,
    DeviceSessionHandle,
    Connection,
    impl Future<Output = ()>,
) {
    let (registry, connection, backend) = fixture();
    let (physical, wake) = MioSessionPoll::try_open().unwrap();
    let signal = Arc::new(handle::SessionSignal {
        stopping: AtomicBool::new(false),
        wake,
    });
    let (intents, inbox) = sync_channel(INTENT_CAPACITY);
    let handle = DeviceSessionHandle {
        intents,
        signal: handle::WakeOnDrop(Arc::clone(&signal)),
    };
    let (worker, work) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::MIN).unwrap();
    (
        DeviceSessionDriver {
            registry,
            worker,
            reactor: DeviceDriverReactor {
                physical,
                inbox,
                signal,
                pending: None,
                active: None,
                completed: None,
                stopping: false,
                admit_intents: true,
            },
            queued: [None, None],
        },
        handle,
        connection,
        work,
    )
}

#[test]
fn disconnect_interrupts_inventory_and_another_connection_can_then_start() {
    within_runtime(async {
        let (mut registry, old, mut backend) = fixture();
        let _ended = registry.step(EndConnection {
            connection: old,
            reason: DisconnectionReason::Cancelled,
        });
        backend.block_inventory = true;
        let shared = backend.shared.clone();
        let (updates, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let session = prepare_device_session::<4>(registry, old.device(), backend, move |update| {
            updates.send(update).unwrap();
        })
        .unwrap();
        let handle = session.handle;
        assert_eq!(
            handle.submit(DeviceSessionIntent::Connect).unwrap(),
            DeviceSessionSubmission::Submitted
        );
        let exercise = async {
            shared.inventory_started.notified().await;
            assert_eq!(
                handle.submit(DeviceSessionIntent::Disconnect).unwrap(),
                DeviceSessionSubmission::Submitted
            );
            shared.closed.notified().await;
            assert_eq!(
                handle.submit(DeviceSessionIntent::Connect).unwrap(),
                DeviceSessionSubmission::Submitted
            );
            shared.inventory_started.notified().await;
            assert_eq!(
                handle.submit(DeviceSessionIntent::Inspect).unwrap(),
                DeviceSessionSubmission::Submitted
            );
            loop {
                if matches!(
                    observed.recv().await.unwrap().event,
                    DeviceSessionEvent::Observed
                ) {
                    break;
                }
            }
            drop(handle);
        };
        let (exit, ()) = tokio::join!(session.run, exercise);
        assert!(exit.unwrap().settlement.is_ok());
        assert_eq!(
            shared
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| **call == Call::Close(LINK))
                .count(),
            2
        );
    });
}

#[test]
fn mailbox_capacity_stop_and_disconnection_preserve_rejected_intents() {
    let (driver, handle, _, work) = driver_fixture();
    assert_eq!(
        handle.submit(DeviceSessionIntent::Connect).unwrap(),
        DeviceSessionSubmission::Submitted
    );
    assert_eq!(
        handle.submit(DeviceSessionIntent::Refresh).unwrap(),
        DeviceSessionSubmission::Busy {
            intent: DeviceSessionIntent::Refresh
        }
    );
    drop(driver);
    assert_eq!(
        handle.submit(DeviceSessionIntent::Disconnect).unwrap(),
        DeviceSessionSubmission::Stopped {
            intent: DeviceSessionIntent::Disconnect
        }
    );
    drop(work);
    let (driver, handle, _, work) = driver_fixture();
    handle.shutdown().unwrap();
    assert_eq!(
        handle.submit(DeviceSessionIntent::Inspect).unwrap(),
        DeviceSessionSubmission::Stopped {
            intent: DeviceSessionIntent::Inspect
        }
    );
    drop((driver, work));
}

#[test]
fn stale_reactions_and_foreign_completions_do_not_mutate_the_registry() {
    let (mut driver, _handle, connection, _work) = driver_fixture();
    let foreign_device = paired(&mut driver.registry, target(99));
    let foreign = begin(&mut driver.registry, foreign_device);
    let before = driver.registry.step(ReadDevice {
        device: connection.device(),
    });
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    for reaction in [
        DeviceDriverReaction::IntentReady,
        DeviceDriverReaction::WorkReady,
    ] {
        assert!(matches!(
            driver.react(&mut board, &reaction).unwrap(),
            CircuitReactionOutcome::AwaitingReaction
        ));
    }
    driver.reactor.completed = Some(Ok(PrnsDeviceOut::StaleClose {
        connection: foreign,
    }));
    assert!(
        matches!(driver.react(&mut board, &DeviceDriverReaction::WorkReady), Err(DeviceDriverFailure::Routing(DeviceSessionRoutingError::WrongDevice { expected, received })) if expected == connection.device() && received == foreign_device)
    );
    assert_eq!(
        driver.registry.step(ReadDevice {
            device: connection.device()
        }),
        before
    );
}

#[test]
fn worker_invariants_terminate_conduction_and_return_registry_ownership() {
    within_runtime(async {
        let (mut registry, connection, mut backend) = fixture();
        let _ended = registry.step(EndConnection {
            connection,
            reason: DisconnectionReason::Cancelled,
        });
        backend.failure = Failure::Panic;
        let session =
            prepare_device_session::<4>(registry, connection.device(), backend, |_| {}).unwrap();
        assert_eq!(
            session.handle.submit(DeviceSessionIntent::Connect).unwrap(),
            DeviceSessionSubmission::Submitted
        );
        let exit = session.run.await.unwrap();
        assert!(matches!(
            exit.settlement,
            Err(ConductionFailure::Circuit {
                failure: DeviceDriverFailure::Worker(crate::PrnsDeviceWorkerError::Fitting(
                    PrnsFittingError::WorkerStopped { .. }
                ))
            })
        ));
        assert_eq!(
            session.handle.submit(DeviceSessionIntent::Inspect).unwrap(),
            DeviceSessionSubmission::Stopped {
                intent: DeviceSessionIntent::Inspect
            }
        );
    });
}

#[test]
fn observer_panics_drain_the_worker_and_return_the_join_error() {
    within_runtime(async {
        let (registry, connection, backend) = fixture();
        let session = prepare_device_session::<4>(registry, connection.device(), backend, |_| {
            panic!("observer failed")
        })
        .unwrap();
        assert_eq!(
            session.handle.submit(DeviceSessionIntent::Inspect).unwrap(),
            DeviceSessionSubmission::Submitted
        );
        let Err(failure) = session.run.await else {
            panic!("observer panic was lost")
        };
        assert!(failure.is_panic());
    });
}

#[test]
fn dropping_an_unpolled_runtime_stops_admission_without_executing_device_work() {
    let (registry, connection, backend) = fixture();
    let shared = backend.shared.clone();
    let session =
        prepare_device_session::<4>(registry, connection.device(), backend, |_| {}).unwrap();
    drop(session.run);
    assert_eq!(
        session.handle.submit(DeviceSessionIntent::Connect).unwrap(),
        DeviceSessionSubmission::Stopped {
            intent: DeviceSessionIntent::Connect
        }
    );
    assert!(shared.calls.lock().unwrap().is_empty());
}

#[test]
fn execution_preserves_success_and_invariant_settlement_through_the_same_observer() {
    for failure in [Failure::None, Failure::Panic] {
        within_runtime(async {
            let (mut registry, connection, mut backend) = fixture();
            let _ended = registry.step(EndConnection {
                connection,
                reason: DisconnectionReason::Cancelled,
            });
            backend.failure = failure;
            let (updates, mut observed) = tokio::sync::mpsc::unbounded_channel();
            let session = prepare_device_session::<4>(
                registry,
                connection.device(),
                backend,
                move |update| {
                    updates.send(update).unwrap();
                },
            )
            .unwrap();
            assert_eq!(
                session.handle.submit(DeviceSessionIntent::Connect).unwrap(),
                DeviceSessionSubmission::Submitted
            );
            let exercise = async {
                if matches!(failure, Failure::None) {
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
                    session.handle.shutdown().unwrap();
                }
            };
            let (exit, ()) = tokio::join!(session.run, exercise);
            let exit = exit.unwrap();
            assert_eq!(exit.settlement.is_ok(), matches!(failure, Failure::None));
        });
    }
}

#[test]
fn abandoning_a_running_idle_session_stops_admission_and_releases_its_link() {
    within_runtime(async {
        let (mut registry, connection, backend) = fixture();
        let _ended = registry.step(EndConnection {
            connection,
            reason: DisconnectionReason::Cancelled,
        });
        let shared = backend.shared.clone();
        let (updates, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let session =
            prepare_device_session::<4>(registry, connection.device(), backend, move |update| {
                updates.send(update).unwrap();
            })
            .unwrap();
        assert_eq!(
            session.handle.submit(DeviceSessionIntent::Connect).unwrap(),
            DeviceSessionSubmission::Submitted
        );
        let mut run = Box::pin(session.run);
        let ready = async {
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
        };
        tokio::select! { _ = &mut run => panic!("session stopped early"), () = ready => {} }
        drop(run);
        assert_eq!(
            session.handle.submit(DeviceSessionIntent::Inspect).unwrap(),
            DeviceSessionSubmission::Stopped {
                intent: DeviceSessionIntent::Inspect
            }
        );
        shared.closed.notified().await;
    });
}

mod controls;
