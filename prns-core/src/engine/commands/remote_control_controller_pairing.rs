use heapless::Vec as HeaplessVec;

use crate::engine::EngineState;
use crate::remote_control::{
    ApproveRemoteControlControllerPairingOutcome, BeginRemoteControlControllerPairingOutcome,
    FailRemoteControlControllerPairingRequestOutcome, RejectRemoteControlControllerPairingOutcome,
    RemoteControlControllerIdentity, RemoteControlControllerPairingAborted,
    RemoteControlControllerPairingActivity, RemoteControlControllerPairingWindowError,
    RemoteControlPairingAttemptId, RemoteControlPairingContext, RemoteControlPairingInvitationCode,
    RemoteControlPairingMessageWriteError, RemoteControlPairingRequest,
    RemoteControlPairingResponse, REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID,
};
use crate::routing::links::request::{
    write_packed_binary_header, PackBinaryError, MAX_PACKED_BINARY_HEADER_LEN,
};
use crate::routing::request_handlers::RequestPathHash;
use crate::storage::StorageLayout;
use crate::units::{ByteLimit, InstantMillis};

use super::{
    PrnsCommand, RequestResponseTimeout, SendRequest, SendRequestData, Settleable, Settlement,
    MAX_SEND_REQUEST_DATA_LEN,
};

const MAX_REMOTE_CONTROL_CONTROLLER_PAIRING_REQUEST_DATA_LEN: usize =
    MAX_PACKED_BINARY_HEADER_LEN + RemoteControlPairingRequest::MAX_ENCODED_LEN;
type RemoteControlControllerPairingRequestData =
    HeaplessVec<u8, MAX_REMOTE_CONTROL_CONTROLLER_PAIRING_REQUEST_DATA_LEN>;
const _: () =
    assert!(MAX_REMOTE_CONTROL_CONTROLLER_PAIRING_REQUEST_DATA_LEN <= MAX_SEND_REQUEST_DATA_LEN);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginRemoteControlControllerPairing {
    pub controller: RemoteControlControllerIdentity,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingRequestConversionError {
    Capacity { required: usize, maximum: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
}

impl TryFrom<RemoteControlControllerPairingRequest> for SendRequest {
    type Error = RemoteControlControllerPairingRequestConversionError;

    fn try_from(request: RemoteControlControllerPairingRequest) -> Result<Self, Self::Error> {
        let required = request.data.len();
        let data = SendRequestData::from_slice(request.data.as_slice()).map_err(|()| {
            RemoteControlControllerPairingRequestConversionError::Capacity {
                required,
                maximum: MAX_SEND_REQUEST_DATA_LEN,
            }
        })?;
        Ok(Self {
            link_id: request.link_id,
            path_hash: RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
            data,
            response_timeout: request.response_timeout,
            maximum_response_bytes: ByteLimit::Maximum(
                RemoteControlPairingResponse::MAX_ENCODED_LEN as u64,
            ),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
            | Settlement::RejectRemoteControlControllerPairing(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApproveRemoteControlControllerPairing {
    pub attempt_id: RemoteControlPairingAttemptId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
            | Settlement::RejectRemoteControlControllerPairing(_) => None,
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
            | Settlement::ApproveRemoteControlControllerPairing(_) => None,
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
        match self.remote_control_controller_pairing.begin(
            command.controller,
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
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)]

    use super::*;
    use crate::engine::test_support::TestStorageLayout;
    use crate::engine::{
        CommandId, EngineReaction, EngineState, IssuedCommand, Journaled, WakeSchedule,
        WakeSchedules,
    };
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
    use crate::interfaces::AttachedInterfaces;
    use crate::remote_control::{
        BeginRemoteControlControllerPairingOutcome,
        ReceiveRemoteControlControllerPairingOfferOutcome, RemoteControlControllerPairingView,
        RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin, RemoteControlPairingCommit,
        RemoteControlPairingIdentity, RemoteControlPairingPermissions,
        RemoteControlPairingPreparedOffer, RemoteControlRequestKind, RemoteControlRequestSet,
    };
    use crate::routing::links::LinkId;
    use crate::units::DurationMillis;

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

    fn context() -> RemoteControlPairingContext {
        RemoteControlPairingContext::new(
            RemoteControlPairingIdentity::new(IdentityHash::new([0x73; 16])).endpoint(),
            LinkId::new([0x84; 16]),
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

    fn awaiting_confirmation(
        engine: &mut EngineState<TestStorageLayout>,
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
            &signer(0x52),
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

    #[test]
    fn begin_returns_the_exact_bounded_request_and_engine_deadline() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let controller = controller();
        let (settlement, schedules) = execute(
            &mut engine,
            PrnsCommand::BeginRemoteControlControllerPairing(BeginRemoteControlControllerPairing {
                controller,
                context: context(),
                invitation_code: invitation_code(),
                pairing_expires_at: PAIRING_EXPIRES_AT,
            }),
            STARTED_AT,
        );
        let Settlement::BeginRemoteControlControllerPairing(Ok(begun)) = settlement else {
            panic!("begin settled")
        };
        assert_eq!(
            SendRequest::try_from(begun.into_request()).unwrap(),
            SendRequest {
                link_id: context().link_id(),
                path_hash: RequestPathHash::of(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID),
                data: packed(RemoteControlPairingRequest::Begin(
                    RemoteControlPairingBegin::new(
                        controller,
                        context().endpoint(),
                        invitation_code(),
                    ),
                )),
                response_timeout: RequestResponseTimeout::LinkDefaultAtMost {
                    maximum: DurationMillis(9_000),
                },
                maximum_response_bytes: ByteLimit::Maximum(
                    RemoteControlPairingResponse::MAX_ENCODED_LEN as u64,
                ),
            },
        );
        assert!(matches!(
            engine.remote_control_controller_pairing.view(),
            RemoteControlControllerPairingView::AwaitingOffer(view)
                if view.context() == context()
                    && view.window().expires_at() == PAIRING_EXPIRES_AT
        ));
        assert_eq!(
            schedules.remote_control_pairing,
            WakeSchedule::At(PAIRING_EXPIRES_AT),
        );
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
        assert_eq!(
            SendRequest::try_from(approval.into_request()).unwrap(),
            SendRequest {
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
            },
        );
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
    fn elapsed_begin_settles_without_creating_request_debt() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let controller = controller();
        let (settlement, schedules) = execute(
            &mut engine,
            PrnsCommand::BeginRemoteControlControllerPairing(BeginRemoteControlControllerPairing {
                controller,
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
}
