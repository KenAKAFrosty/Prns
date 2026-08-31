use heapless::Vec as HeaplessVec;

use crate::engine::{
    AdmitRemoteControlControllerPairingResponseOutcome, EngineState,
    RemoteControlControllerPairingResponseEffect,
};
use crate::remote_control::{
    ApproveRemoteControlControllerPairingOutcome, BeginRemoteControlControllerPairingOutcome,
    FailRemoteControlControllerPairingPersistenceOutcome,
    FailRemoteControlControllerPairingRequestOutcome, PersistRemoteControlControllerPairingOutcome,
    RejectRemoteControlControllerPairingOutcome, RemoteControlControllerPairingAborted,
    RemoteControlControllerPairingActivity, RemoteControlControllerPairingWindowError,
    RemoteControlPairingAttemptId, RemoteControlPairingContext, RemoteControlPairingInvitationCode,
    RemoteControlPairingMessageWriteError, RemoteControlPairingRequest,
    RemoteControlPairingResponse, RemoteControlTargetAccess,
    REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID,
};
use crate::routing::links::request::{
    write_packed_binary_header, PackBinaryError, SendRequestView, MAX_PACKED_BINARY_HEADER_LEN,
};
use crate::routing::request_handlers::RequestPathHash;
use crate::storage::StorageLayout;
use crate::units::{ByteLimit, InstantMillis};

use super::{
    PacketReceiptDelivered, PrnsCommand, RequestResponseTimeout, SendRequestFailure, Settleable,
    Settlement, MAX_SEND_REQUEST_DATA_LEN,
};

const MAX_REMOTE_CONTROL_CONTROLLER_PAIRING_REQUEST_DATA_LEN: usize =
    MAX_PACKED_BINARY_HEADER_LEN + RemoteControlPairingRequest::MAX_ENCODED_LEN;
type RemoteControlControllerPairingRequestData =
    HeaplessVec<u8, MAX_REMOTE_CONTROL_CONTROLLER_PAIRING_REQUEST_DATA_LEN>;
const _: () =
    assert!(MAX_REMOTE_CONTROL_CONTROLLER_PAIRING_REQUEST_DATA_LEN <= MAX_SEND_REQUEST_DATA_LEN);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginRemoteControlControllerPairing {
    pub context: RemoteControlPairingContext,
    pub invitation_code: RemoteControlPairingInvitationCode,
    pub pairing_expires_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingRequestBuildError {
    Encode(RemoteControlPairingMessageWriteError),
    Pack(PackBinaryError),
    Capacity { required: usize, maximum: usize },
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlControllerPairingRequest {
    link_id: crate::routing::links::LinkId,
    data: RemoteControlControllerPairingRequestData,
    response_timeout: RequestResponseTimeout,
}

impl RemoteControlControllerPairingRequest {
    pub(crate) fn new(
        context: RemoteControlPairingContext,
        request: RemoteControlPairingRequest,
        expires_at: InstantMillis,
        now: InstantMillis,
    ) -> Result<Self, RemoteControlControllerPairingRequestBuildError> {
        let mut encoded = [0u8; RemoteControlPairingRequest::MAX_ENCODED_LEN];
        let encoded_len = request
            .write_into(&mut encoded)
            .map_err(RemoteControlControllerPairingRequestBuildError::Encode)?;
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(encoded_len, &mut header)
            .map_err(RemoteControlControllerPairingRequestBuildError::Pack)?;
        let required = header_len.saturating_add(encoded_len);
        let mut data = RemoteControlControllerPairingRequestData::new();
        let maximum = data.capacity();
        data.extend_from_slice(&header[..header_len])
            .and_then(|()| data.extend_from_slice(&encoded[..encoded_len]))
            .map_err(
                |()| RemoteControlControllerPairingRequestBuildError::Capacity {
                    required,
                    maximum,
                },
            )?;
        Ok(Self {
            link_id: context.link_id(),
            data,
            response_timeout: RequestResponseTimeout::LinkDefaultAtMost {
                maximum: expires_at.duration_since(now),
            },
        })
    }

    pub(crate) fn send_request(&self) -> SendRequestView<'_> {
        SendRequestView::new(
            self.link_id,
            RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
            self.data.as_slice(),
            self.response_timeout,
            ByteLimit::Maximum(RemoteControlPairingResponse::MAX_ENCODED_LEN as u64),
        )
    }

    pub(crate) const fn link_id(&self) -> crate::routing::links::LinkId {
        self.link_id
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlControllerPairingBegun {
    request: RemoteControlControllerPairingRequest,
}

impl RemoteControlControllerPairingBegun {
    pub(crate) const fn new(request: RemoteControlControllerPairingRequest) -> Self {
        Self { request }
    }

    #[must_use]
    pub const fn request(&self) -> &RemoteControlControllerPairingRequest {
        &self.request
    }

    #[must_use]
    pub fn into_request(self) -> RemoteControlControllerPairingRequest {
        self.request
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginRemoteControlControllerPairingFailure {
    ControllerIdentityUnavailable,
    Busy {
        active: RemoteControlControllerPairingActivity,
    },
    PairingUnavailable {
        reason: RemoteControlControllerPairingWindowError,
    },
    RequestBuild {
        failure: RemoteControlControllerPairingRequestBuildError,
        rollback: FailRemoteControlControllerPairingRequestOutcome,
    },
}

impl Settleable for BeginRemoteControlControllerPairing {
    type Success = RemoteControlControllerPairingBegun;
    type Failure = BeginRemoteControlControllerPairingFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::BeginRemoteControlControllerPairing(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<RemoteControlControllerPairingBegun, Self::Failure>> {
        match settlement {
            Settlement::BeginRemoteControlControllerPairing(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::OpenRemoteControlPairing(_)
            | Settlement::CloseRemoteControlPairing(_)
            | Settlement::ApproveRemoteControlTargetPairing(_)
            | Settlement::RejectRemoteControlTargetPairing(_)
            | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
            | Settlement::ApproveRemoteControlControllerPairing(_)
            | Settlement::RejectRemoteControlControllerPairing(_)
            | Settlement::RemoteControlControllerPairingRequest(_)
            | Settlement::SettleRemoteControlControllerPairingPersistence(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApproveRemoteControlControllerPairing {
    pub attempt_id: RemoteControlPairingAttemptId,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlControllerPairingApproval {
    attempt_id: RemoteControlPairingAttemptId,
    request: RemoteControlControllerPairingRequest,
}

impl RemoteControlControllerPairingApproval {
    pub(crate) const fn new(
        attempt_id: RemoteControlPairingAttemptId,
        request: RemoteControlControllerPairingRequest,
    ) -> Self {
        Self {
            attempt_id,
            request,
        }
    }

    #[must_use]
    pub const fn attempt_id(&self) -> RemoteControlPairingAttemptId {
        self.attempt_id
    }

    #[must_use]
    pub const fn request(&self) -> &RemoteControlControllerPairingRequest {
        &self.request
    }

    #[must_use]
    pub fn into_request(self) -> RemoteControlControllerPairingRequest {
        self.request
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingResponseReceived {
    pub delivered: PacketReceiptDelivered,
    pub admission: AdmitRemoteControlControllerPairingResponseOutcome,
    pub effect: RemoteControlControllerPairingResponseEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingRequestFailureCause {
    Request(SendRequestFailure),
    ResourceResponseUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingRequestFailure {
    pub cause: RemoteControlControllerPairingRequestFailureCause,
    pub exchange: FailRemoteControlControllerPairingRequestOutcome,
}

impl Settleable for RemoteControlControllerPairingRequest {
    type Success = RemoteControlControllerPairingResponseReceived;
    type Failure = RemoteControlControllerPairingRequestFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::RemoteControlControllerPairingRequest(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<RemoteControlControllerPairingResponseReceived, Self::Failure>> {
        match settlement {
            Settlement::RemoteControlControllerPairingRequest(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::OpenRemoteControlPairing(_)
            | Settlement::CloseRemoteControlPairing(_)
            | Settlement::ApproveRemoteControlTargetPairing(_)
            | Settlement::RejectRemoteControlTargetPairing(_)
            | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
            | Settlement::BeginRemoteControlControllerPairing(_)
            | Settlement::ApproveRemoteControlControllerPairing(_)
            | Settlement::RejectRemoteControlControllerPairing(_)
            | Settlement::SettleRemoteControlControllerPairingPersistence(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveRemoteControlControllerPairingFailure {
    Expired {
        expired: RemoteControlControllerPairingAborted,
    },
    NoActiveAttempt,
    OfferNotReceived,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
    RequestBuild {
        failure: RemoteControlControllerPairingRequestBuildError,
        rollback: FailRemoteControlControllerPairingRequestOutcome,
    },
}

impl Settleable for ApproveRemoteControlControllerPairing {
    type Success = RemoteControlControllerPairingApproval;
    type Failure = ApproveRemoteControlControllerPairingFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::ApproveRemoteControlControllerPairing(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<RemoteControlControllerPairingApproval, Self::Failure>> {
        match settlement {
            Settlement::ApproveRemoteControlControllerPairing(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::OpenRemoteControlPairing(_)
            | Settlement::CloseRemoteControlPairing(_)
            | Settlement::ApproveRemoteControlTargetPairing(_)
            | Settlement::RejectRemoteControlTargetPairing(_)
            | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
            | Settlement::BeginRemoteControlControllerPairing(_)
            | Settlement::RejectRemoteControlControllerPairing(_)
            | Settlement::RemoteControlControllerPairingRequest(_)
            | Settlement::SettleRemoteControlControllerPairingPersistence(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RejectRemoteControlControllerPairing {
    pub attempt_id: RemoteControlPairingAttemptId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingRejection {
    pub aborted: RemoteControlControllerPairingAborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectRemoteControlControllerPairingFailure {
    Expired {
        expired: RemoteControlControllerPairingAborted,
    },
    NoActiveAttempt,
    OfferNotReceived,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlPairingAttemptId,
    },
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

impl Settleable for RejectRemoteControlControllerPairing {
    type Success = RemoteControlControllerPairingRejection;
    type Failure = RejectRemoteControlControllerPairingFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::RejectRemoteControlControllerPairing(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<RemoteControlControllerPairingRejection, Self::Failure>> {
        match settlement {
            Settlement::RejectRemoteControlControllerPairing(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::OpenRemoteControlPairing(_)
            | Settlement::CloseRemoteControlPairing(_)
            | Settlement::ApproveRemoteControlTargetPairing(_)
            | Settlement::RejectRemoteControlTargetPairing(_)
            | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
            | Settlement::BeginRemoteControlControllerPairing(_)
            | Settlement::ApproveRemoteControlControllerPairing(_)
            | Settlement::RemoteControlControllerPairingRequest(_)
            | Settlement::SettleRemoteControlControllerPairingPersistence(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingPersistence {
    Persisted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettleRemoteControlControllerPairingPersistence {
    pub attempt_id: RemoteControlPairingAttemptId,
    pub persistence: RemoteControlControllerPairingPersistence,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlControllerPairingFinalization {
    Completed {
        attempt_id: RemoteControlPairingAttemptId,
        access: RemoteControlTargetAccess,
    },
    PersistenceFailureRecorded {
        attempt_id: RemoteControlPairingAttemptId,
        access: RemoteControlTargetAccess,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleRemoteControlControllerPairingPersistenceFailure {
    NoPersistenceOwed {
        settled: RemoteControlPairingAttemptId,
    },
    AttemptMismatch {
        settled: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
}

impl Settleable for SettleRemoteControlControllerPairingPersistence {
    type Success = RemoteControlControllerPairingFinalization;
    type Failure = SettleRemoteControlControllerPairingPersistenceFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SettleRemoteControlControllerPairingPersistence(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<RemoteControlControllerPairingFinalization, Self::Failure>> {
        match settlement {
            Settlement::SettleRemoteControlControllerPairingPersistence(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::OpenRemoteControlPairing(_)
            | Settlement::CloseRemoteControlPairing(_)
            | Settlement::ApproveRemoteControlTargetPairing(_)
            | Settlement::RejectRemoteControlTargetPairing(_)
            | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
            | Settlement::BeginRemoteControlControllerPairing(_)
            | Settlement::ApproveRemoteControlControllerPairing(_)
            | Settlement::RejectRemoteControlControllerPairing(_)
            | Settlement::RemoteControlControllerPairingRequest(_) => None,
        }
    }
}

impl<S: StorageLayout> EngineState<S> {
    pub(in crate::engine) fn execute_begin_remote_control_controller_pairing(
        &mut self,
        command: BeginRemoteControlControllerPairing,
        now: InstantMillis,
    ) -> Result<RemoteControlControllerPairingBegun, BeginRemoteControlControllerPairingFailure>
    {
        let controller = match self.remote_control_controller_identity {
            crate::engine::RemoteControlControllerIdentityConfiguration::Unavailable => {
                return Err(
                    BeginRemoteControlControllerPairingFailure::ControllerIdentityUnavailable,
                )
            }
            crate::engine::RemoteControlControllerIdentityConfiguration::Configured(controller) => {
                controller
            }
        };
        match self.remote_control_controller_pairing.begin(
            controller,
            command.context,
            command.invitation_code,
            now,
            command.pairing_expires_at,
        ) {
            BeginRemoteControlControllerPairingOutcome::BeginOwed {
                begin,
                context,
                expires_at,
            } => match RemoteControlControllerPairingRequest::new(
                context,
                RemoteControlPairingRequest::Begin(begin),
                expires_at,
                now,
            ) {
                Ok(request) => Ok(RemoteControlControllerPairingBegun::new(request)),
                Err(failure) => {
                    let rollback = self
                        .remote_control_controller_pairing
                        .request_failed(context.link_id());
                    Err(BeginRemoteControlControllerPairingFailure::RequestBuild {
                        failure,
                        rollback,
                    })
                }
            },
            BeginRemoteControlControllerPairingOutcome::Busy { active } => {
                Err(BeginRemoteControlControllerPairingFailure::Busy { active })
            }
            BeginRemoteControlControllerPairingOutcome::PairingUnavailable { reason } => {
                Err(BeginRemoteControlControllerPairingFailure::PairingUnavailable { reason })
            }
        }
    }

    pub(in crate::engine) fn execute_approve_remote_control_controller_pairing(
        &mut self,
        command: ApproveRemoteControlControllerPairing,
        now: InstantMillis,
    ) -> Result<RemoteControlControllerPairingApproval, ApproveRemoteControlControllerPairingFailure>
    {
        match self
            .remote_control_controller_pairing
            .approve(command.attempt_id, now)
        {
            ApproveRemoteControlControllerPairingOutcome::CommitOwed {
                attempt_id,
                commit,
                context,
                expires_at,
            } => match RemoteControlControllerPairingRequest::new(
                context,
                RemoteControlPairingRequest::Commit(commit),
                expires_at,
                now,
            ) {
                Ok(request) => Ok(RemoteControlControllerPairingApproval::new(
                    attempt_id, request,
                )),
                Err(failure) => {
                    let rollback = self
                        .remote_control_controller_pairing
                        .request_failed(context.link_id());
                    Err(ApproveRemoteControlControllerPairingFailure::RequestBuild {
                        failure,
                        rollback,
                    })
                }
            },
            ApproveRemoteControlControllerPairingOutcome::Expired { expired } => {
                Err(ApproveRemoteControlControllerPairingFailure::Expired { expired })
            }
            ApproveRemoteControlControllerPairingOutcome::NoActiveAttempt => {
                Err(ApproveRemoteControlControllerPairingFailure::NoActiveAttempt)
            }
            ApproveRemoteControlControllerPairingOutcome::OfferNotReceived => {
                Err(ApproveRemoteControlControllerPairingFailure::OfferNotReceived)
            }
            ApproveRemoteControlControllerPairingOutcome::AttemptMismatch { requested, active } => {
                Err(
                    ApproveRemoteControlControllerPairingFailure::AttemptMismatch {
                        requested,
                        active,
                    },
                )
            }
            ApproveRemoteControlControllerPairingOutcome::PersistenceInProgress { attempt_id } => {
                Err(
                    ApproveRemoteControlControllerPairingFailure::PersistenceInProgress {
                        attempt_id,
                    },
                )
            }
        }
    }

    pub(in crate::engine) fn execute_reject_remote_control_controller_pairing(
        &mut self,
        command: RejectRemoteControlControllerPairing,
        now: InstantMillis,
    ) -> Result<RemoteControlControllerPairingRejection, RejectRemoteControlControllerPairingFailure>
    {
        match self
            .remote_control_controller_pairing
            .reject(command.attempt_id, now)
        {
            RejectRemoteControlControllerPairingOutcome::Rejected { aborted } => {
                Ok(RemoteControlControllerPairingRejection { aborted })
            }
            RejectRemoteControlControllerPairingOutcome::Expired { expired } => {
                Err(RejectRemoteControlControllerPairingFailure::Expired { expired })
            }
            RejectRemoteControlControllerPairingOutcome::NoActiveAttempt => {
                Err(RejectRemoteControlControllerPairingFailure::NoActiveAttempt)
            }
            RejectRemoteControlControllerPairingOutcome::OfferNotReceived => {
                Err(RejectRemoteControlControllerPairingFailure::OfferNotReceived)
            }
            RejectRemoteControlControllerPairingOutcome::AttemptMismatch { requested, active } => {
                Err(
                    RejectRemoteControlControllerPairingFailure::AttemptMismatch {
                        requested,
                        active,
                    },
                )
            }
            RejectRemoteControlControllerPairingOutcome::AlreadyApproved { attempt_id } => {
                Err(RejectRemoteControlControllerPairingFailure::AlreadyApproved { attempt_id })
            }
            RejectRemoteControlControllerPairingOutcome::PersistenceInProgress { attempt_id } => {
                Err(
                    RejectRemoteControlControllerPairingFailure::PersistenceInProgress {
                        attempt_id,
                    },
                )
            }
        }
    }

    pub(in crate::engine) fn execute_settle_remote_control_controller_pairing_persistence(
        &mut self,
        command: SettleRemoteControlControllerPairingPersistence,
    ) -> Result<
        RemoteControlControllerPairingFinalization,
        SettleRemoteControlControllerPairingPersistenceFailure,
    > {
        let attempt_id = command.attempt_id;
        match command.persistence {
            RemoteControlControllerPairingPersistence::Persisted => {
                match self
                    .remote_control_controller_pairing
                    .persistence_succeeded(attempt_id)
                {
                    PersistRemoteControlControllerPairingOutcome::Completed {
                        attempt_id,
                        access,
                    } => Ok(RemoteControlControllerPairingFinalization::Completed {
                        attempt_id,
                        access,
                    }),
                    PersistRemoteControlControllerPairingOutcome::NoPersistenceOwed => Err(
                        SettleRemoteControlControllerPairingPersistenceFailure::NoPersistenceOwed {
                            settled: attempt_id,
                        },
                    ),
                    PersistRemoteControlControllerPairingOutcome::AttemptMismatch {
                        persisted,
                        active,
                    } => Err(
                        SettleRemoteControlControllerPairingPersistenceFailure::AttemptMismatch {
                            settled: persisted,
                            active,
                        },
                    ),
                }
            }
            RemoteControlControllerPairingPersistence::Failed => {
                match self
                    .remote_control_controller_pairing
                    .persistence_failed(attempt_id)
                {
                    FailRemoteControlControllerPairingPersistenceOutcome::Failed {
                        attempt_id,
                        access,
                    } => Ok(
                        RemoteControlControllerPairingFinalization::PersistenceFailureRecorded {
                            attempt_id,
                            access,
                        },
                    ),
                    FailRemoteControlControllerPairingPersistenceOutcome::NoPersistenceOwed => Err(
                        SettleRemoteControlControllerPairingPersistenceFailure::NoPersistenceOwed {
                            settled: attempt_id,
                        },
                    ),
                    FailRemoteControlControllerPairingPersistenceOutcome::AttemptMismatch {
                        failed,
                        active,
                    } => Err(
                        SettleRemoteControlControllerPairingPersistenceFailure::AttemptMismatch {
                            settled: failed,
                            active,
                        },
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)]

    use super::*;
    use crate::engine::test_support::{filled_frame, routable_descriptor, TestStorageLayout};
    use crate::engine::{
        CommandId, DeliveryEvidence, Directive, EngineReaction, EngineState, IngestIo,
        IssuedCommand, Journaled, PacketReceiptDelivered, SendRequest, SendRequestData,
        SendRequestIntent, WakeSchedule, WakeSchedules,
    };
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
    use crate::interfaces::{AttachedInterfaces, InboundPacket};
    use crate::remote_control::{
        BeginRemoteControlControllerPairingOutcome,
        ReceiveRemoteControlControllerPairingCompletedOutcome,
        ReceiveRemoteControlControllerPairingOfferOutcome, RemoteControlControllerIdentity,
        RemoteControlControllerIdentitySecret, RemoteControlControllerPairingView,
        RemoteControlNodeIdentitySecrets, RemoteControlPairingAttemptTimeout,
        RemoteControlPairingBegin, RemoteControlPairingCommit, RemoteControlPairingCompleted,
        RemoteControlPairingIdentity, RemoteControlPairingPermissions,
        RemoteControlPairingPreparedOffer, RemoteControlRequestKind, RemoteControlRequestSet,
        RemoteControlTargetIdentitySecret,
    };
    use crate::routing::dedup::PacketHash;
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::request::{write_response_plaintext, RequestId};
    use crate::routing::links::resources::receive::tests_support::{
        advertise_response_segment_from, engine_with_active_link, feed, lane, link_id, link_key,
    };
    use crate::routing::links::resources::ResourceSegment;
    use crate::units::{DurationMillis, RttMillis};
    use crate::wire::{WireContext, BROADCAST_MTU};

    const STARTED_AT: InstantMillis = InstantMillis(1_000);
    const PAIRING_EXPIRES_AT: InstantMillis = InstantMillis(10_000);
    const OFFERED_AT: InstantMillis = InstantMillis(2_000);
    const ATTEMPT_EXPIRES_AT: InstantMillis = InstantMillis(5_000);

    fn signer(fill: u8) -> InMemoryNodeIdentity {
        InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
            [fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
        ))
    }

    fn controller() -> RemoteControlControllerIdentity {
        let signer = signer(0x31);
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: signer.encryption_public_key(),
            signing: signer.signing_public_key(),
        })
    }

    fn identity_secrets(controller_fill: u8, target_fill: u8) -> RemoteControlNodeIdentitySecrets {
        RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
                [controller_fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
            )),
            RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
                [target_fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
            )),
        )
        .unwrap()
    }

    fn configure_controller(
        engine: &mut EngineState<TestStorageLayout>,
    ) -> RemoteControlControllerIdentity {
        let identities = engine
            .configure_remote_control_identities(identity_secrets(0x31, 0x32))
            .unwrap();
        *identities.controller()
    }

    fn context() -> RemoteControlPairingContext {
        RemoteControlPairingContext::new(
            RemoteControlPairingIdentity::new(IdentityHash::new([0x73; 16])).endpoint(),
            link_id(),
        )
    }

    fn permissions() -> RemoteControlPairingPermissions {
        RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
            RemoteControlRequestKind::Describe,
        ))
        .unwrap()
    }

    fn invitation_code() -> RemoteControlPairingInvitationCode {
        RemoteControlPairingInvitationCode::from_value(0x1234_ABCD)
    }

    fn packed(request: RemoteControlPairingRequest) -> SendRequestData {
        let mut encoded = [0u8; RemoteControlPairingRequest::MAX_ENCODED_LEN];
        let encoded_len = request.write_into(&mut encoded).unwrap();
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(encoded_len, &mut header).unwrap();
        let mut data = SendRequestData::new();
        data.extend_from_slice(&header[..header_len]).unwrap();
        data.extend_from_slice(&encoded[..encoded_len]).unwrap();
        data
    }

    fn packed_response(response: RemoteControlPairingResponse) -> std::vec::Vec<u8> {
        let mut encoded = [0u8; RemoteControlPairingResponse::MAX_ENCODED_LEN];
        let encoded_len = response.write_into(&mut encoded).unwrap();
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(encoded_len, &mut header).unwrap();
        let mut packed = std::vec::Vec::with_capacity(header_len + encoded_len);
        packed.extend_from_slice(&header[..header_len]);
        packed.extend_from_slice(&encoded[..encoded_len]);
        packed
    }

    fn execute(
        engine: &mut EngineState<TestStorageLayout>,
        command: PrnsCommand,
        now: InstantMillis,
    ) -> (Settlement, WakeSchedules) {
        let mut settled = None;
        let schedules = engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(0xC1),
                command,
            },
            AttachedInterfaces::new(&[]),
            now,
            &mut |_| panic!("controller pairing commands need no entropy"),
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::CommandSettled {
                    id: CommandId(0xC1),
                    settlement,
                }) => settled = Some(settlement),
                EngineReaction::Journaled(Journaled::CommandSettled { .. })
                | EngineReaction::Journaled(_)
                | EngineReaction::Directive(_) => {}
            },
        );
        (settled.unwrap(), schedules)
    }

    fn dispatch_begin_request(
        engine: &mut EngineState<TestStorageLayout>,
    ) -> (RemoteControlControllerIdentity, RequestId) {
        let controller = configure_controller(engine);
        let (begin_settlement, _) = execute(
            engine,
            PrnsCommand::BeginRemoteControlControllerPairing(BeginRemoteControlControllerPairing {
                context: context(),
                invitation_code: invitation_code(),
                pairing_expires_at: PAIRING_EXPIRES_AT,
            }),
            STARTED_AT,
        );
        let Settlement::BeginRemoteControlControllerPairing(Ok(begun)) = begin_settlement else {
            panic!("begin request")
        };
        let interfaces = [routable_descriptor(lane())];
        let mut request_frame = None;
        let mut request_settled = false;
        engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(0xC2),
                command: begun.into_request().into_command(),
            },
            AttachedInterfaces::new(&interfaces),
            STARTED_AT,
            &mut |bytes| bytes.fill(0xA5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    request_frame = filled_frame(fill);
                }
                EngineReaction::Journaled(Journaled::CommandSettled {
                    id: CommandId(0xC2),
                    ..
                }) => request_settled = true,
                EngineReaction::Directive(_) | EngineReaction::Journaled(_) => {}
            },
        );
        assert!(!request_settled);
        let request_id =
            RequestId::of_packet(&PacketHash::of_wire_packet(&request_frame.unwrap()).unwrap());
        assert_eq!(
            engine.receipts.pending_request_intent(request_id),
            Some(SendRequestIntent::RemoteControlControllerPairing),
        );
        (controller, request_id)
    }

    fn awaiting_confirmation(
        engine: &mut EngineState<TestStorageLayout>,
    ) -> (
        RemoteControlPairingAttemptId,
        crate::remote_control::RemoteControlPairingTranscript,
    ) {
        awaiting_confirmation_from(engine, 0x52)
    }

    fn awaiting_confirmation_from(
        engine: &mut EngineState<TestStorageLayout>,
        target_fill: u8,
    ) -> (
        RemoteControlPairingAttemptId,
        crate::remote_control::RemoteControlPairingTranscript,
    ) {
        let controller = controller();
        let begin = match engine.remote_control_controller_pairing.begin(
            controller,
            context(),
            invitation_code(),
            STARTED_AT,
            PAIRING_EXPIRES_AT,
        ) {
            BeginRemoteControlControllerPairingOutcome::BeginOwed {
                begin,
                context: owed_context,
                expires_at,
            } => {
                assert_eq!(owed_context, context());
                assert_eq!(expires_at, PAIRING_EXPIRES_AT);
                begin
            }
            BeginRemoteControlControllerPairingOutcome::Busy { .. }
            | BeginRemoteControlControllerPairingOutcome::PairingUnavailable { .. } => {
                panic!("fresh controller pairing")
            }
        };
        let prepared = RemoteControlPairingPreparedOffer::new(
            &signer(target_fill),
            context(),
            &begin,
            permissions(),
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(3_000)).unwrap(),
        );
        let (offer, transcript) = prepared.into_parts();
        let ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired { attempt_id } =
            engine
                .remote_control_controller_pairing
                .receive_offer(offer, OFFERED_AT)
        else {
            panic!("confirmation required")
        };
        (attempt_id, transcript)
    }

    fn awaiting_persistence(
        engine: &mut EngineState<TestStorageLayout>,
    ) -> (
        RemoteControlPairingAttemptId,
        IdentityHash,
        RemoteControlRequestSet,
    ) {
        let (attempt_id, transcript) = awaiting_confirmation(engine);
        assert!(matches!(
            engine
                .remote_control_controller_pairing
                .approve(attempt_id, InstantMillis(2_500)),
            ApproveRemoteControlControllerPairingOutcome::CommitOwed { .. }
        ));
        let target_identity = transcript.target().identity_hash();
        let permitted_requests = *transcript.permissions().permitted_requests();
        let completed =
            RemoteControlPairingCompleted::signed_by(&signer(0x52), &transcript).unwrap();
        assert_eq!(
            engine
                .remote_control_controller_pairing
                .receive_completed(completed, InstantMillis(3_000)),
            ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed { attempt_id },
        );
        (attempt_id, target_identity, permitted_requests)
    }

    #[test]
    fn begin_returns_the_exact_bounded_request_and_engine_deadline() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let controller = configure_controller(&mut engine);
        let (settlement, schedules) = execute(
            &mut engine,
            PrnsCommand::BeginRemoteControlControllerPairing(BeginRemoteControlControllerPairing {
                context: context(),
                invitation_code: invitation_code(),
                pairing_expires_at: PAIRING_EXPIRES_AT,
            }),
            STARTED_AT,
        );
        let Settlement::BeginRemoteControlControllerPairing(Ok(begun)) = settlement else {
            panic!("begin settled")
        };
        let request = begun.into_request();
        let expected = SendRequest {
            link_id: context().link_id(),
            path_hash: RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
            data: packed(RemoteControlPairingRequest::Begin(
                RemoteControlPairingBegin::new(controller, context().endpoint(), invitation_code()),
            )),
            response_timeout: RequestResponseTimeout::LinkDefaultAtMost {
                maximum: DurationMillis(9_000),
            },
            maximum_response_bytes: ByteLimit::Maximum(
                RemoteControlPairingResponse::MAX_ENCODED_LEN as u64,
            ),
        };
        assert_eq!(request.send_request(), (&expected).into());
        assert!(matches!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::AwaitingOffer(view)
                if view.controller() == &controller
                    && view.context() == context()
                    && view.window().expires_at() == PAIRING_EXPIRES_AT
        ));
        assert_eq!(
            schedules.remote_control_pairing,
            WakeSchedule::At(PAIRING_EXPIRES_AT),
        );
    }

    #[test]
    fn exact_pairing_request_receipt_admits_offer_without_application_response() {
        let mut engine = engine_with_active_link();
        let (controller, request_id) = dispatch_begin_request(&mut engine);
        let interfaces = [routable_descriptor(lane())];

        let begin =
            RemoteControlPairingBegin::new(controller, context().endpoint(), invitation_code());
        let prepared = RemoteControlPairingPreparedOffer::new(
            &signer(0x52),
            context(),
            &begin,
            permissions(),
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(3_000)).unwrap(),
        );
        let (offer, transcript) = prepared.into_parts();
        let attempt_id = (&transcript).into();
        let packed = packed_response(RemoteControlPairingResponse::Offer(offer));
        let mut plaintext = [0u8; BROADCAST_MTU];
        let plaintext_len = write_response_plaintext(&request_id, &packed, &mut plaintext).unwrap();
        let mut response_frame = [0u8; BROADCAST_MTU];
        let response_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::Response,
            &plaintext[..plaintext_len],
            &[0xB5; 16],
            &mut response_frame,
        )
        .unwrap();
        let mut response_settlement = None;
        let mut confirmation = None;
        let mut application_responses = 0usize;
        engine.ingest_packet_into(
            InboundPacket {
                arrived_at: OFFERED_AT,
                source_interface: lane(),
                bytes: &mut response_frame[..response_len],
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&interfaces),
                now: OFFERED_AT,
                fill_entropy: &mut |_| {},
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Journaled(
                        Journaled::RemoteControlControllerPairingConfirmationRequired(view),
                    ) => confirmation = Some(view.attempt_id()),
                    EngineReaction::Journaled(Journaled::ResponseReceived { .. }) => {
                        application_responses = application_responses.saturating_add(1);
                    }
                    EngineReaction::Journaled(Journaled::CommandSettled {
                        id: CommandId(0xC2),
                        settlement,
                    }) => response_settlement = Some(settlement),
                    EngineReaction::Directive(_) | EngineReaction::Journaled(_) => {}
                },
            },
        );

        assert_eq!(
            response_settlement,
            Some(Settlement::RemoteControlControllerPairingRequest(Ok(
                RemoteControlControllerPairingResponseReceived {
                    delivered: PacketReceiptDelivered {
                        rtt: RttMillis::new(1_000),
                        evidence: DeliveryEvidence::Response,
                    },
                    admission: AdmitRemoteControlControllerPairingResponseOutcome::Offer(
                        ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired {
                            attempt_id,
                        },
                    ),
                    effect: RemoteControlControllerPairingResponseEffect::Advanced,
                },
            ))),
        );
        assert_eq!(confirmation, Some(attempt_id));
        assert_eq!(application_responses, 0);
        assert!(!engine.receipts.has_pending_request(request_id));
        assert!(matches!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::AwaitingApproval(view)
                if view.attempt_id() == attempt_id
        ));
    }

    #[test]
    fn pairing_request_timeout_settles_typed_and_aborts_exchange() {
        let mut engine = engine_with_active_link();
        let (_, request_id) = dispatch_begin_request(&mut engine);
        let mut settlement = None;

        engine.settle_timed_out_receipts(PAIRING_EXPIRES_AT, &mut |reaction| match reaction {
            EngineReaction::Journaled(Journaled::CommandSettled {
                id: CommandId(0xC2),
                settlement: settled,
            }) => settlement = Some(settled),
            EngineReaction::Directive(_) | EngineReaction::Journaled(_) => {}
        });

        assert_eq!(
            settlement,
            Some(Settlement::RemoteControlControllerPairingRequest(Err(
                RemoteControlControllerPairingRequestFailure {
                    cause: RemoteControlControllerPairingRequestFailureCause::Request(
                        SendRequestFailure::Timeout,
                    ),
                    exchange: FailRemoteControlControllerPairingRequestOutcome::Aborted {
                        aborted: RemoteControlControllerPairingAborted::AwaitingOffer {
                            context: context(),
                        },
                    },
                },
            ))),
        );
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
        assert!(!engine.receipts.has_pending_request(request_id));
    }

    #[test]
    fn malformed_pairing_response_retires_consumed_exchange() {
        let mut engine = engine_with_active_link();
        let (_, request_id) = dispatch_begin_request(&mut engine);
        let mut plaintext = [0u8; BROADCAST_MTU];
        let plaintext_len = write_response_plaintext(&request_id, &[0x00], &mut plaintext).unwrap();
        let mut response_frame = [0u8; BROADCAST_MTU];
        let response_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::Response,
            &plaintext[..plaintext_len],
            &[0xB6; 16],
            &mut response_frame,
        )
        .unwrap();

        let capture = feed(&mut engine, &response_frame[..response_len], OFFERED_AT.0);

        assert_eq!(
            capture.settlements,
            std::vec![(
                CommandId(0xC2),
                Settlement::RemoteControlControllerPairingRequest(Ok(
                    RemoteControlControllerPairingResponseReceived {
                        delivered: PacketReceiptDelivered {
                            rtt: RttMillis::new(1_000),
                            evidence: DeliveryEvidence::Response,
                        },
                        admission:
                            AdmitRemoteControlControllerPairingResponseOutcome::MalformedEnvelope(
                                crate::routing::links::request::PackedBinaryParseError::NotBinary,
                            ),
                        effect: RemoteControlControllerPairingResponseEffect::NotAdvanced(
                            FailRemoteControlControllerPairingRequestOutcome::Aborted {
                                aborted: RemoteControlControllerPairingAborted::AwaitingOffer {
                                    context: context(),
                                },
                            },
                        ),
                    },
                )),
            )],
        );
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
        assert!(!engine.receipts.has_pending_request(request_id));
    }

    #[test]
    fn pairing_request_rejects_resource_response_and_aborts_exchange() {
        let mut engine = engine_with_active_link();
        let (_, request_id) = dispatch_begin_request(&mut engine);
        let mut sender = engine_with_active_link();
        let advertisement = advertise_response_segment_from(
            &mut sender,
            CommandId(0xD1),
            request_id,
            b"offer",
            None,
            ResourceSegment::whole(5),
            1_500,
        );

        let capture = feed(&mut engine, &advertisement, OFFERED_AT.0);

        assert_eq!(
            capture.settlements,
            std::vec![
                (
                    CommandId(0xC2),
                    Settlement::RemoteControlControllerPairingRequest(Err(
                        RemoteControlControllerPairingRequestFailure {
                            cause: RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported,
                            exchange:
                                FailRemoteControlControllerPairingRequestOutcome::Aborted {
                                    aborted:
                                        RemoteControlControllerPairingAborted::AwaitingOffer {
                                            context: context(),
                                        },
                                },
                        },
                    )),
                ),
            ],
        );
        assert_eq!(capture.frames.len(), 1);
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
        assert!(!engine.receipts.has_pending_request(request_id));
    }

    #[test]
    fn approval_returns_the_exact_commit_request_and_attempt_deadline() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let (attempt_id, transcript) = awaiting_confirmation(&mut engine);
        let approved_at = InstantMillis(2_500);
        let (settlement, schedules) = execute(
            &mut engine,
            PrnsCommand::ApproveRemoteControlControllerPairing(
                ApproveRemoteControlControllerPairing { attempt_id },
            ),
            approved_at,
        );
        let Settlement::ApproveRemoteControlControllerPairing(Ok(approval)) = settlement else {
            panic!("approval settled")
        };
        assert_eq!(approval.attempt_id(), attempt_id);
        let request = approval.into_request();
        let expected = SendRequest {
            link_id: context().link_id(),
            path_hash: RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
            data: packed(RemoteControlPairingRequest::Commit(
                RemoteControlPairingCommit::new(&transcript),
            )),
            response_timeout: RequestResponseTimeout::LinkDefaultAtMost {
                maximum: DurationMillis(2_500),
            },
            maximum_response_bytes: ByteLimit::Maximum(
                RemoteControlPairingResponse::MAX_ENCODED_LEN as u64,
            ),
        };
        assert_eq!(request.send_request(), (&expected).into());
        assert!(matches!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::AwaitingCompletion(view)
                if view.attempt_id() == attempt_id
                    && view.window().expires_at() == ATTEMPT_EXPIRES_AT
        ));
        assert_eq!(
            schedules.remote_control_pairing,
            WakeSchedule::At(ATTEMPT_EXPIRES_AT),
        );
    }

    #[test]
    fn rejection_aborts_only_the_confirmed_attempt_without_network_debt() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let (attempt_id, _) = awaiting_confirmation(&mut engine);
        let (settlement, schedules) = execute(
            &mut engine,
            PrnsCommand::RejectRemoteControlControllerPairing(
                RejectRemoteControlControllerPairing { attempt_id },
            ),
            InstantMillis(2_500),
        );
        assert_eq!(
            settlement,
            Settlement::RejectRemoteControlControllerPairing(Ok(
                RemoteControlControllerPairingRejection {
                    aborted: RemoteControlControllerPairingAborted::AwaitingApproval {
                        attempt_id,
                        context: context(),
                    },
                },
            )),
        );
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
        assert_eq!(schedules.remote_control_pairing, WakeSchedule::Idle);
    }

    #[test]
    fn persisted_controller_access_completes_once_with_the_exact_access() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let (attempt_id, target_identity, permitted_requests) = awaiting_persistence(&mut engine);
        let command = SettleRemoteControlControllerPairingPersistence {
            attempt_id,
            persistence: RemoteControlControllerPairingPersistence::Persisted,
        };

        let (settlement, schedules) =
            execute(&mut engine, command.into_command(), InstantMillis(4_000));
        let Settlement::SettleRemoteControlControllerPairingPersistence(Ok(
            RemoteControlControllerPairingFinalization::Completed {
                attempt_id: completed,
                access,
            },
        )) = settlement
        else {
            panic!("persistence settled")
        };
        assert_eq!(completed, attempt_id);
        assert_eq!(access.target().identity_hash(), target_identity);
        assert_eq!(access.permitted_requests(), &permitted_requests);
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
        assert_eq!(schedules.remote_control_pairing, WakeSchedule::Idle);

        let (repeated, repeated_schedules) =
            execute(&mut engine, command.into_command(), InstantMillis(4_001));
        assert_eq!(
            repeated,
            Settlement::SettleRemoteControlControllerPairingPersistence(Err(
                SettleRemoteControlControllerPairingPersistenceFailure::NoPersistenceOwed {
                    settled: attempt_id,
                },
            )),
        );
        assert_eq!(
            repeated_schedules.remote_control_pairing,
            WakeSchedule::Idle
        );
    }

    #[test]
    fn failed_controller_access_persistence_aborts_with_the_exact_access() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let (attempt_id, target_identity, permitted_requests) = awaiting_persistence(&mut engine);

        let (settlement, schedules) = execute(
            &mut engine,
            SettleRemoteControlControllerPairingPersistence {
                attempt_id,
                persistence: RemoteControlControllerPairingPersistence::Failed,
            }
            .into_command(),
            InstantMillis(4_000),
        );
        let Settlement::SettleRemoteControlControllerPairingPersistence(Ok(
            RemoteControlControllerPairingFinalization::PersistenceFailureRecorded {
                attempt_id: failed,
                access,
            },
        )) = settlement
        else {
            panic!("persistence failure settled")
        };
        assert_eq!(failed, attempt_id);
        assert_eq!(access.target().identity_hash(), target_identity);
        assert_eq!(access.permitted_requests(), &permitted_requests);
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
        assert_eq!(schedules.remote_control_pairing, WakeSchedule::Idle);
    }

    #[test]
    fn stale_controller_persistence_cannot_disturb_the_active_attempt() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let (attempt_id, target_identity, permitted_requests) = awaiting_persistence(&mut engine);
        let mut unrelated = EngineState::<TestStorageLayout>::default();
        let (unrelated_attempt_id, _) = awaiting_confirmation_from(&mut unrelated, 0x53);
        assert_ne!(unrelated_attempt_id, attempt_id);

        let (stale, _) = execute(
            &mut engine,
            SettleRemoteControlControllerPairingPersistence {
                attempt_id: unrelated_attempt_id,
                persistence: RemoteControlControllerPairingPersistence::Persisted,
            }
            .into_command(),
            InstantMillis(4_000),
        );
        assert_eq!(
            stale,
            Settlement::SettleRemoteControlControllerPairingPersistence(Err(
                SettleRemoteControlControllerPairingPersistenceFailure::AttemptMismatch {
                    settled: unrelated_attempt_id,
                    active: attempt_id,
                },
            )),
        );
        assert!(matches!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Persisting(persistence)
                if persistence.attempt_id() == attempt_id
                    && persistence.access().target().identity_hash() == target_identity
                    && persistence.access().permitted_requests() == &permitted_requests
        ));

        let (settled, _) = execute(
            &mut engine,
            SettleRemoteControlControllerPairingPersistence {
                attempt_id,
                persistence: RemoteControlControllerPairingPersistence::Failed,
            }
            .into_command(),
            InstantMillis(4_001),
        );
        assert!(matches!(
            settled,
            Settlement::SettleRemoteControlControllerPairingPersistence(Ok(
                RemoteControlControllerPairingFinalization::PersistenceFailureRecorded {
                    attempt_id: failed,
                    ..
                },
            )) if failed == attempt_id
        ));
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
    }

    #[test]
    fn elapsed_begin_settles_without_creating_request_debt() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        configure_controller(&mut engine);
        let (settlement, schedules) = execute(
            &mut engine,
            PrnsCommand::BeginRemoteControlControllerPairing(BeginRemoteControlControllerPairing {
                context: context(),
                invitation_code: invitation_code(),
                pairing_expires_at: PAIRING_EXPIRES_AT,
            }),
            PAIRING_EXPIRES_AT,
        );
        assert_eq!(
            settlement,
            Settlement::BeginRemoteControlControllerPairing(Err(
                BeginRemoteControlControllerPairingFailure::PairingUnavailable {
                    reason: RemoteControlControllerPairingWindowError::DeadlineNotFuture {
                        started_at: PAIRING_EXPIRES_AT,
                        expires_at: PAIRING_EXPIRES_AT,
                    },
                },
            )),
        );
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
        assert_eq!(schedules.remote_control_pairing, WakeSchedule::Idle);
    }

    #[test]
    fn begin_requires_the_configured_controller_identity() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let (settlement, schedules) = execute(
            &mut engine,
            PrnsCommand::BeginRemoteControlControllerPairing(BeginRemoteControlControllerPairing {
                context: context(),
                invitation_code: invitation_code(),
                pairing_expires_at: PAIRING_EXPIRES_AT,
            }),
            STARTED_AT,
        );

        assert_eq!(
            settlement,
            Settlement::BeginRemoteControlControllerPairing(Err(
                BeginRemoteControlControllerPairingFailure::ControllerIdentityUnavailable,
            )),
        );
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
        assert_eq!(schedules.remote_control_pairing, WakeSchedule::Idle);
    }

    #[test]
    fn configured_remote_control_identities_cannot_be_replaced() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let first = engine
            .configure_remote_control_identities(identity_secrets(0x31, 0x32))
            .unwrap();
        let expected_hashes = [
            first.controller().identity_hash(),
            first.target().identity_hash(),
        ];

        assert_eq!(
            engine.configure_remote_control_identities(identity_secrets(0x41, 0x42)),
            Err(crate::engine::ConfigureRemoteControlIdentitiesError::AlreadyConfigured),
        );
        assert_eq!(engine.held_identity_hashes(), &expected_hashes);
    }

    #[test]
    fn retiring_the_exchange_link_aborts_and_journals_controller_pairing() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let controller = configure_controller(&mut engine);
        let BeginRemoteControlControllerPairingOutcome::BeginOwed { .. } =
            engine.remote_control_controller_pairing.begin(
                controller,
                context(),
                invitation_code(),
                STARTED_AT,
                PAIRING_EXPIRES_AT,
            )
        else {
            panic!("fresh controller pairing")
        };
        let mut pairing_closed = None;
        let mut link_closed = None;

        engine.retire_link(
            &context().link_id(),
            crate::engine::LinkClosedReason::PeerClosed,
            &mut |reaction| match reaction {
                EngineReaction::Journaled(
                    Journaled::RemoteControlControllerPairingLinkClosed { aborted },
                ) => pairing_closed = Some(aborted),
                EngineReaction::Journaled(Journaled::LinkClosed { link_id, reason }) => {
                    link_closed = Some((link_id, reason));
                }
                EngineReaction::Journaled(_) | EngineReaction::Directive(_) => {}
            },
        );

        assert_eq!(
            pairing_closed,
            Some(RemoteControlControllerPairingAborted::AwaitingOffer { context: context() }),
        );
        assert_eq!(
            link_closed,
            Some((
                context().link_id(),
                crate::engine::LinkClosedReason::PeerClosed,
            )),
        );
        assert_eq!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::Idle,
        );
    }
}
