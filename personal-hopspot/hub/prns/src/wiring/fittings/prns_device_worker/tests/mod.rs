#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::tests::*;
use core::time::Duration;
use pipecircuit::TransportFrom;

mod properties;

fn submit(worker: &mut PrnsDeviceWorker, input: PrnsDeviceIn) -> PrnsDeviceCompletion {
    let (_, mut outgoing) = <_ as DuplexFitting<PrnsDevice>>::split(worker);
    let PrnsDeviceSubmission::Submitted { completion } = outgoing.send(input) else {
        panic!("submission refused")
    };
    completion
}

async fn receive(completion: PrnsDeviceCompletion) -> PrnsDeviceOut {
    let mut incoming = PrnsDeviceWorkerIncoming;
    let ReceiveFromOutcome::Received { output } =
        incoming.receive_from(completion.complete().await)
    else {
        panic!("work failed")
    };
    output
}

async fn bounded(future: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .unwrap();
}

#[test]
fn queue_capacity_bounds_are_checked_before_creating_the_runtime_channel() {
    let maximum = tokio::sync::Semaphore::MAX_PERMITS;
    for capacity in [maximum, maximum.checked_add(1).unwrap(), usize::MAX] {
        let (_, connection, backend) = fixture();
        let capacity = NonZeroUsize::new(capacity).unwrap();
        let created = PrnsDeviceWorker::try_new(connection.device(), backend, capacity);
        if capacity.get() == maximum {
            assert!(created.is_ok());
        } else {
            let Err(error) = created else {
                panic!("invalid queue capacity accepted")
            };
            assert_eq!(
                error,
                PrnsDeviceQueueCapacityError {
                    requested: capacity,
                    maximum
                }
            );
        }
    }
}

#[tokio::test]
async fn fifo_work_preserves_typed_outputs_and_drains_before_shutdown() {
    let (_, connection, backend) = fixture();
    let shared = backend.shared.clone();
    let (mut worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(2).unwrap())
            .unwrap();
    let connect = submit(&mut worker, PrnsDeviceIn::Connect { connection });
    let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
    let inventory = submit(&mut worker, PrnsDeviceIn::Inventory { request });
    drop(worker);
    bounded(async {
        let ((), ()) = tokio::join!(run, async {
            assert_eq!(
                receive(connect).await,
                PrnsDeviceOut::Connected {
                    confirmation: ConfirmConnection {
                        connection,
                        target: connection.target(),
                        link: LINK
                    }
                }
            );
            let PrnsDeviceOut::InterfacesReceived { response, rtt } = receive(inventory).await
            else {
                panic!("no inventory")
            };
            assert_eq!(response.request, request);
            assert_eq!(rtt, RttMillis::new(42));
        });
    })
    .await;
    assert_eq!(
        *shared.calls.lock().unwrap(),
        [
            Call::Resolve(connection.target().identity_hash()),
            Call::Establish(connection.target().endpoint().destination_hash()),
            Call::Identify(LINK, controller().identity_hash()),
            Call::Inventory(LINK, RemoteControlInterfacePage::First),
            Call::Close(LINK),
        ]
    );
}

#[tokio::test]
async fn backpressure_and_stopped_submission_return_the_original_input() {
    let (_, connection, backend) = fixture();
    let (mut worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(1).unwrap())
            .unwrap();
    let pending = submit(&mut worker, PrnsDeviceIn::Connect { connection });
    let (_, mut outgoing) = <_ as DuplexFitting<PrnsDevice>>::split(&mut worker);
    let PrnsDeviceSubmission::Busy { input } = outgoing.send(PrnsDeviceIn::Close { connection })
    else {
        panic!("queue exceeded capacity")
    };
    assert_eq!(input, PrnsDeviceIn::Close { connection });
    drop(run);
    let PrnsDeviceSubmission::Stopped { input } = outgoing.send(input) else {
        panic!("stopped worker accepted input")
    };
    assert_eq!(input, PrnsDeviceIn::Close { connection });
    let mut incoming = PrnsDeviceWorkerIncoming;
    assert!(matches!(
        incoming.receive_from(pending.complete().await),
        ReceiveFromOutcome::Failed {
            failure: PrnsDeviceWorkerError::Stopped(_)
        }
    ));
}

#[tokio::test]
async fn cancelled_queued_connect_and_inventory_never_reach_the_backend() {
    let (_, connection, backend) = fixture();
    let shared = backend.shared.clone();
    let (mut worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(2).unwrap())
            .unwrap();
    drop(submit(&mut worker, PrnsDeviceIn::Connect { connection }));
    let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
    drop(submit(&mut worker, PrnsDeviceIn::Inventory { request }));
    drop(worker);
    bounded(run).await;
    assert!(shared.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancelled_connect_settles_authentication_and_closes_before_later_work() {
    let (_, connection, mut backend) = fixture();
    backend.block = true;
    let shared = backend.shared.clone();
    let (mut worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(1).unwrap())
            .unwrap();
    let pending = submit(&mut worker, PrnsDeviceIn::Connect { connection });
    bounded(async {
        tokio::join!(run, async {
            shared.identified.notified().await;
            drop(pending);
            let closed = submit(&mut worker, PrnsDeviceIn::Close { connection });
            drop(worker);
            shared.release.notify_one();
            assert_eq!(
                receive(closed).await,
                PrnsDeviceOut::StaleClose { connection }
            );
        });
    })
    .await;
    assert_eq!(
        shared
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| **call == Call::Close(LINK))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_unaccepted_connection_completion_is_closed_even_after_delivery() {
    let (_, connection, backend) = fixture();
    let shared = backend.shared.clone();
    let (mut worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(1).unwrap())
            .unwrap();
    let mut pending = submit(&mut worker, PrnsDeviceIn::Connect { connection });
    bounded(async {
        tokio::join!(run, async {
            assert!(matches!(
                (&mut pending.result).await.unwrap(),
                Ok(PrnsDeviceOut::Connected { .. })
            ));
            drop(pending);
            let closed = submit(&mut worker, PrnsDeviceIn::Close { connection });
            assert_eq!(
                receive(closed).await,
                PrnsDeviceOut::StaleClose { connection }
            );
            drop(worker);
        });
    })
    .await;
    assert_eq!(
        shared
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| **call == Call::Close(LINK))
            .count(),
        1
    );
}

#[tokio::test]
async fn abandoning_a_duplicate_connect_does_not_close_the_owned_session() {
    let (_, connection, backend) = fixture();
    let shared = backend.shared.clone();
    let (mut worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(1).unwrap())
            .unwrap();
    bounded(async {
        tokio::join!(run, async {
            assert!(matches!(
                receive(submit(&mut worker, PrnsDeviceIn::Connect { connection })).await,
                PrnsDeviceOut::Connected { .. }
            ));
            let mut duplicate = submit(&mut worker, PrnsDeviceIn::Connect { connection });
            assert!(matches!(
                (&mut duplicate.result).await.unwrap(),
                Ok(PrnsDeviceOut::AlreadyConnected { .. })
            ));
            drop(duplicate);
            let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
            assert!(matches!(
                receive(submit(&mut worker, PrnsDeviceIn::Inventory { request })).await,
                PrnsDeviceOut::InterfacesReceived { .. }
            ));
            assert_eq!(
                receive(submit(&mut worker, PrnsDeviceIn::Close { connection })).await,
                PrnsDeviceOut::Closed {
                    connection,
                    settlement: CloseRemoteControlTargetOutcome::Queued
                }
            );
            drop(worker);
        });
    })
    .await;
    assert_eq!(
        shared
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| **call == Call::Close(LINK))
            .count(),
        1
    );
}

#[tokio::test]
async fn cancelling_a_running_inventory_unblocks_close_without_waiting_for_io() {
    let (_, connection, mut backend) = fixture();
    backend.block_inventory = true;
    let shared = backend.shared.clone();
    let (mut worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(1).unwrap())
            .unwrap();
    bounded(async {
        tokio::join!(run, async {
            let _connected =
                receive(submit(&mut worker, PrnsDeviceIn::Connect { connection })).await;
            let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
            let pending = submit(&mut worker, PrnsDeviceIn::Inventory { request });
            shared.inventory_started.notified().await;
            drop(pending);
            assert_eq!(
                receive(submit(&mut worker, PrnsDeviceIn::Close { connection })).await,
                PrnsDeviceOut::Closed {
                    connection,
                    settlement: CloseRemoteControlTargetOutcome::Queued
                }
            );
            drop(worker);
        });
    })
    .await;
    assert_eq!(
        shared.calls.lock().unwrap().last(),
        Some(&Call::Close(LINK))
    );
}

#[tokio::test]
async fn accepted_close_runs_even_when_its_completion_is_abandoned() {
    let (_, connection, backend) = fixture();
    let shared = backend.shared.clone();
    let (mut worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(2).unwrap())
            .unwrap();
    bounded(async {
        tokio::join!(run, async {
            let _connected =
                receive(submit(&mut worker, PrnsDeviceIn::Connect { connection })).await;
            drop(submit(&mut worker, PrnsDeviceIn::Close { connection }));
            let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
            assert_eq!(
                receive(submit(&mut worker, PrnsDeviceIn::Inventory { request })).await,
                PrnsDeviceOut::StaleInventory { request }
            );
            drop(worker);
        });
    })
    .await;
    assert_eq!(
        shared
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| **call == Call::Close(LINK))
            .count(),
        1
    );
}

#[tokio::test]
async fn fitting_invariants_and_peer_failures_keep_their_original_types() {
    for failure in [Failure::None, Failure::Resolve, Failure::Panic] {
        let (mut registry, connection, mut backend) = fixture();
        backend.failure = failure;
        let foreign = paired(&mut registry, target(99));
        let foreign_connection = begin(&mut registry, foreign);
        let (mut worker, run) =
            PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(1).unwrap())
                .unwrap();
        bounded(async {
            tokio::join!(run, async {
                let foreign = submit(&mut worker, PrnsDeviceIn::Connect { connection: foreign_connection });
                assert!(matches!(foreign.complete().await, Err(PrnsDeviceWorkerError::Fitting(PrnsFittingError::WrongDevice { expected, received })) if expected == connection.device() && received == foreign_connection.device()));
                let result = submit(&mut worker, PrnsDeviceIn::Connect { connection }).complete().await;
                match failure {
                    Failure::None => assert!(matches!(result, Ok(PrnsDeviceOut::Connected { .. }))),
                    Failure::Resolve => assert!(matches!(result, Ok(PrnsDeviceOut::ConnectionFailed { source: ConnectRemoteControlTargetError::Resolve(ResolveRemoteControlTargetControlError::TargetNotAuthorized), .. }))),
                    Failure::Panic => assert!(matches!(result, Err(PrnsDeviceWorkerError::Fitting(PrnsFittingError::WorkerStopped { .. })))),
                    Failure::Establish | Failure::Identify | Failure::Inventory => panic!("unsupported scenario"),
                }
                drop(worker);
            });
        }).await;
    }
}

#[tokio::test]
async fn asynchronous_execution_waits_for_capacity_and_preserves_stopped_worker_errors() {
    let (_, connection, backend) = fixture();
    let (mut worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::MIN).unwrap();
    let queued = submit(&mut worker, PrnsDeviceIn::Connect { connection });
    let close = worker.execute(PrnsDeviceIn::Close { connection });
    tokio::pin!(close);
    core::future::poll_fn(|context| {
        assert!(close.as_mut().poll(context).is_pending());
        core::task::Poll::Ready(())
    })
    .await;
    drop(queued);
    drop(worker);
    bounded(async {
        tokio::join!(run, async {
            assert_eq!(
                close.await.unwrap(),
                PrnsDeviceOut::StaleClose { connection }
            );
        });
    })
    .await;
    let (_, connection, backend) = fixture();
    let (worker, run) =
        PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::MIN).unwrap();
    drop(run);
    assert!(matches!(
        worker.execute(PrnsDeviceIn::Close { connection }).await,
        Err(PrnsDeviceWorkerError::Stopped(_))
    ));
}
