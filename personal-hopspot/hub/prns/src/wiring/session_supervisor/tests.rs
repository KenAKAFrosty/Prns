#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::tests::*;
use crate::{DeviceSessionIntent, DeviceSessionSubmission, prepare_device_session};

#[test]
fn requested_shutdown_drains_authentication_and_closes_before_stopping_the_node() {
    within_runtime(async {
        let (mut registry, old, mut backend) = fixture();
        let _ended = registry.step(EndConnection {
            connection: old,
            reason: DisconnectionReason::Cancelled,
        });
        backend.block = true;
        let shared = backend.shared.clone();
        let session = prepare_device_session::<4>(registry, old.device(), backend, |_| {}).unwrap();
        let handle = session.handle.clone();
        assert_eq!(
            handle.submit(DeviceSessionIntent::Connect).unwrap(),
            DeviceSessionSubmission::Submitted
        );
        let (shutdown, requested) = tokio::sync::oneshot::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let work = supervise_device_session(
            session,
            async {
                stopped.await.unwrap();
                assert_eq!(
                    shared.calls.lock().unwrap().last(),
                    Some(&Call::Close(LINK))
                );
                Ok::<_, ()>(())
            },
            move || {
                stop.send(()).unwrap();
            },
            async {
                requested.await.unwrap();
            },
        );
        let exercise = async {
            shared.identified.notified().await;
            shutdown.send(()).unwrap();
            while !matches!(
                handle.submit(DeviceSessionIntent::Inspect).unwrap(),
                DeviceSessionSubmission::Stopped { .. }
            ) {
                tokio::task::yield_now().await;
            }
            assert!(!shared.calls.lock().unwrap().contains(&Call::Close(LINK)));
            shared.release.notify_one();
        };
        let (exit, ()) = tokio::join!(work, exercise);
        assert_eq!(exit.reason, SessionStopReason::Requested);
        assert!(exit.session.unwrap().settlement.is_ok());
        assert_eq!(exit.node, Ok(()));
        assert!(exit.shutdown_wake.is_ok());
    });
}

#[test]
fn spontaneous_node_failure_stops_the_session_and_retains_the_node_error() {
    within_runtime(async {
        let (registry, connection, backend) = fixture();
        let session =
            prepare_device_session::<4>(registry, connection.device(), backend, |_| {}).unwrap();
        let exit = supervise_device_session(
            session,
            async { Err(17u8) },
            || panic!("already stopped node was stopped twice"),
            core::future::pending(),
        )
        .await;
        assert_eq!(exit.reason, SessionStopReason::NodeStopped);
        assert_eq!(exit.node, Err(17));
        assert!(exit.session.unwrap().settlement.is_ok());
        assert!(exit.shutdown_wake.is_ok());
    });
}

#[test]
fn session_completion_requests_native_shutdown_without_waiting_for_an_external_signal() {
    within_runtime(async {
        let (registry, connection, backend) = fixture();
        let session =
            prepare_device_session::<4>(registry, connection.device(), backend, |_| {}).unwrap();
        session.handle.shutdown().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let exit = supervise_device_session(
            session,
            async {
                stopped.await.unwrap();
                Ok::<_, ()>(())
            },
            move || {
                stop.send(()).unwrap();
            },
            core::future::pending(),
        )
        .await;
        assert_eq!(exit.reason, SessionStopReason::SessionStopped);
        assert_eq!(exit.node, Ok(()));
        assert!(exit.session.unwrap().settlement.is_ok());
        assert!(exit.shutdown_wake.is_ok());
    });
}

#[test]
fn every_shutdown_initiator_preserves_both_owner_results() {
    for reason in [
        SessionStopReason::Requested,
        SessionStopReason::SessionStopped,
        SessionStopReason::NodeStopped,
    ] {
        within_runtime(async {
            let (registry, connection, backend) = fixture();
            let session =
                prepare_device_session::<4>(registry, connection.device(), backend, |_| {})
                    .unwrap();
            if reason == SessionStopReason::SessionStopped {
                session.handle.shutdown().unwrap();
            }
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let node = async {
                if reason == SessionStopReason::NodeStopped {
                    return Err(17u8);
                }
                stopped.await.unwrap();
                Ok(())
            };
            let shutdown = async {
                if reason != SessionStopReason::Requested {
                    core::future::pending::<()>().await;
                }
            };
            let exit = supervise_device_session(
                session,
                node,
                move || {
                    stop.send(()).unwrap();
                },
                shutdown,
            )
            .await;
            assert_eq!(exit.reason, reason);
            assert_eq!(
                exit.node,
                if reason == SessionStopReason::NodeStopped {
                    Err(17)
                } else {
                    Ok(())
                }
            );
            assert!(exit.session.unwrap().settlement.is_ok());
            assert!(exit.shutdown_wake.is_ok());
        });
    }
}
