use crate::{DeviceSessionExit, DeviceSessionRuntime};
use core::future::Future;
use tokio::task::JoinError;

#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;

#[derive(Debug, PartialEq, Eq)]
pub enum SessionStopReason {
    Requested,
    SessionStopped,
    NodeStopped,
}

pub struct SupervisedSessionExit<NodeFailure> {
    pub reason: SessionStopReason,
    pub session: Result<DeviceSessionExit, JoinError>,
    pub node: Result<(), NodeFailure>,
    pub shutdown_wake: Result<(), std::io::Error>,
}

pub async fn supervise_device_session<NodeFailure>(
    session: DeviceSessionRuntime<impl Future<Output = Result<DeviceSessionExit, JoinError>>>,
    node: impl Future<Output = Result<(), NodeFailure>>,
    stop_node: impl FnOnce(),
    shutdown: impl Future<Output = ()>,
) -> SupervisedSessionExit<NodeFailure> {
    let DeviceSessionRuntime { handle, run } = session;
    tokio::pin!(run, node, shutdown);
    tokio::select! {
        session = &mut run => {
            stop_node();
            SupervisedSessionExit { reason: SessionStopReason::SessionStopped, session, node: node.await, shutdown_wake: Ok(()) }
        }
        node = &mut node => {
            let shutdown_wake = handle.shutdown();
            SupervisedSessionExit { reason: SessionStopReason::NodeStopped, session: run.await, node, shutdown_wake }
        }
        () = &mut shutdown => {
            let shutdown_wake = handle.shutdown();
            let drain = async {
                let session = run.await;
                stop_node();
                session
            };
            let (session, node) = tokio::join!(drain, node);
            SupervisedSessionExit { reason: SessionStopReason::Requested, session, node, shutdown_wake }
        }
    }
}
