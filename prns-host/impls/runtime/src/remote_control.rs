//! Typed Remote Control operations on the host-owned runtime.
//!
//! The protocol request and response remain the authoritative public Rust types.
//! Admission, cancellation and shutdown use the common bounded native task lane.

mod operations;
pub use operations::RemoteControlOperations;

use crate::{HostRuntime, HostSubmitError};
use personal_rns::identity::IdentityHash;
use personal_rns::remote_control::{
    RemoteControlRequest, RemoteControlRequestKind, RemoteControlResponse,
};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{
    ConnectRemoteControlTargetError, RemoteControlError, RemoteControlTargetOperationError,
};
use personal_rns::units::RttMillis;
use personal_rns::PrnsNodeHandle;
use tokio::sync::oneshot;

type ExchangeResult = Result<(RemoteControlResponse, RttMillis), HostRemoteControlError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRemoteControlError {
    Admission(HostSubmitError),
    Connect(ConnectRemoteControlTargetError),
    Exchange(RemoteControlError),
    TargetOperation(RemoteControlTargetOperationError),
}

impl HostRuntime {
    /// Use a caller-managed link. The caller establishes and identifies it using
    /// canonical commands; the target enforces its remote-control authorization.
    pub async fn remote_control_exchange(
        &self,
        link_id: LinkId,
        request: RemoteControlRequest,
    ) -> Result<(RemoteControlResponse, RttMillis), HostRemoteControlError> {
        self.begin_remote_control_exchange(link_id, request)
            .map_err(HostRemoteControlError::Admission)?
            .await
            .map_err(|_| HostRemoteControlError::Admission(HostSubmitError::Stopped))?
    }

    pub(crate) fn begin_remote_control_exchange(
        &self,
        link_id: LinkId,
        request: RemoteControlRequest,
    ) -> Result<oneshot::Receiver<ExchangeResult>, HostSubmitError> {
        self.begin_preview_operation(move |handle| async move {
            handle
                .remote_control(link_id)
                .exchange(request)
                .await
                .map_err(HostRemoteControlError::Exchange)
        })
    }

    /// Resolve a persisted target grant, identify the controller, exchange one
    /// typed request, and close the temporary link on every completion path.
    pub async fn remote_control_target_exchange(
        &self,
        target: IdentityHash,
        request: RemoteControlRequest,
    ) -> Result<(RemoteControlResponse, RttMillis), HostRemoteControlError> {
        self.begin_remote_control_target_exchange(target, request)
            .map_err(HostRemoteControlError::Admission)?
            .await
            .map_err(|_| HostRemoteControlError::Admission(HostSubmitError::Stopped))?
    }

    pub(crate) fn begin_remote_control_target_exchange(
        &self,
        target: IdentityHash,
        request: RemoteControlRequest,
    ) -> Result<oneshot::Receiver<ExchangeResult>, HostSubmitError> {
        self.begin_preview_operation(move |handle| async move {
            let connection = handle
                .connect_remote_control_target(target)
                .await
                .map_err(HostRemoteControlError::Connect)?;
            let _close = CloseLinkOnDrop {
                handle: handle.clone(),
                link_id: connection.connection().link_id(),
            };
            connection
                .exchange(request)
                .await
                .map_err(HostRemoteControlError::TargetOperation)
        })
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlExchangeSettlement {
    Completed {
        response: RemoteControlResponse,
        rtt_millis: u64,
    },
    Failed {
        failure: HostRemoteControlError,
    },
}

impl From<Result<(RemoteControlResponse, RttMillis), HostRemoteControlError>>
    for RemoteControlExchangeSettlement
{
    fn from(result: Result<(RemoteControlResponse, RttMillis), HostRemoteControlError>) -> Self {
        match result {
            Ok((response, rtt)) => Self::Completed {
                response,
                rtt_millis: rtt.millis(),
            },
            Err(failure) => Self::Failed { failure },
        }
    }
}

/// Local destination metadata only; no credential-bearing request is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlOperationDestination {
    Target { identity: IdentityHash },
    Link { link_id: LinkId },
}

/// Submission is an uncertainty boundary, not a delivery claim. Completed may
/// contain a peer-reported protocol failure, including failed rollback.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlOperationState {
    Pending,
    Submitted,
    CancelledBeforeSubmission,
    NotSubmitted {
        failure: HostRemoteControlError,
    },
    Completed {
        settlement: RemoteControlExchangeSettlement,
    },
    OutcomeUnknown {
        failure: HostRemoteControlError,
    },
}

impl RemoteControlOperationState {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending | Self::Submitted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlOperationSnapshot {
    /// Process-owner-local identity, never a remote idempotency/replay token.
    pub operation_id: u64,
    pub destination: RemoteControlOperationDestination,
    pub request_kind: RemoteControlRequestKind,
    pub state: RemoteControlOperationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlOperationCancellation {
    Missing,
    CancelledBeforeSubmission,
    AlreadySettled,
    AlreadySubmitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlOperationRetirement {
    Missing,
    Pending,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlRetentionConfigError {
    pub maximum_operations: u32,
}
