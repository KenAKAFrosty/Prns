//! Typed Remote Control operations on the host-owned runtime.
//!
//! The protocol request and response remain the authoritative public Rust types.
//! Admission, cancellation and shutdown use the common bounded native task lane.

use crate::{NativeHost, NativeSubmitError};
use personal_rns::identity::IdentityHash;
use personal_rns::remote_control::{RemoteControlRequest, RemoteControlResponse};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{
    ConnectRemoteControlTargetError, RemoteControlError, RemoteControlTargetOperationError,
};
use personal_rns::units::RttMillis;
use personal_rns::PrnsNodeHandle;

#[derive(Debug, PartialEq, Eq)]
pub enum NativeRemoteControlError {
    Admission(NativeSubmitError),
    Connect(ConnectRemoteControlTargetError),
    Exchange(RemoteControlError),
    TargetOperation(RemoteControlTargetOperationError),
}

impl NativeHost {
    /// Use a caller-managed link. The caller establishes and identifies it using
    /// canonical commands; the target enforces its remote-control authorization.
    pub async fn remote_control_exchange(
        &self,
        link_id: LinkId,
        request: RemoteControlRequest,
    ) -> Result<(RemoteControlResponse, RttMillis), NativeRemoteControlError> {
        self.on_preview_runtime(move |handle| async move {
            handle
                .remote_control(link_id)
                .exchange(request)
                .await
                .map_err(NativeRemoteControlError::Exchange)
        })
        .await
        .map_err(NativeRemoteControlError::Admission)?
    }

    /// Resolve a persisted target grant, identify the controller, exchange one
    /// typed request, and close the temporary link on every completion path.
    pub async fn remote_control_target_exchange(
        &self,
        target: IdentityHash,
        request: RemoteControlRequest,
    ) -> Result<(RemoteControlResponse, RttMillis), NativeRemoteControlError> {
        self.on_preview_runtime(move |handle| async move {
            let connection = handle
                .connect_remote_control_target(target)
                .await
                .map_err(NativeRemoteControlError::Connect)?;
            let _close = CloseLinkOnDrop {
                handle: handle.clone(),
                link_id: connection.connection().link_id(),
            };
            connection
                .exchange(request)
                .await
                .map_err(NativeRemoteControlError::TargetOperation)
        })
        .await
        .map_err(NativeRemoteControlError::Admission)?
    }
}

struct CloseLinkOnDrop {
    handle: PrnsNodeHandle,
    link_id: LinkId,
}
impl Drop for CloseLinkOnDrop {
    fn drop(&mut self) {
        let _ = self.handle.close_link(self.link_id);
    }
}

/// Transport-neutral result preserving the authoritative protocol error tree.
/// RTT is exact milliseconds; generated foreign projections retain all variants.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlExchangeSettlement {
    Completed {
        response: RemoteControlResponse,
        rtt_millis: u64,
    },
    Failed {
        failure: NativeRemoteControlError,
    },
}

impl From<Result<(RemoteControlResponse, RttMillis), NativeRemoteControlError>>
    for RemoteControlExchangeSettlement
{
    fn from(result: Result<(RemoteControlResponse, RttMillis), NativeRemoteControlError>) -> Self {
        match result {
            Ok((response, rtt)) => Self::Completed {
                response,
                rtt_millis: rtt.millis(),
            },
            Err(failure) => Self::Failed { failure },
        }
    }
}
