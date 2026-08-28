use crate::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, InstantMillis, SendRequestFailure,
};
use crate::identity::IdentityHash;
use crate::remote_control::{
    RemoteControlAccessTable, RemoteControlAnnounceSelfOutcome, RemoteControlDescription,
    RemoteControlDescriptionError, RemoteControlMessageWriteError, RemoteControlProtocolError,
    RemoteControlRequest, RemoteControlRequestKind, RemoteControlRequestParseError,
    RemoteControlRequestSet, RemoteControlResponse, RemoteControlResponseKind,
    RemoteControlResponseParseError, RemoteControlSelfAnnouncement,
    REMOTE_CONTROL_REQUEST_ENDPOINT_ID,
};
use crate::units::ByteLimit;
use crate::wire::DestinationHash;

use super::request_endpoints::{
    Decline, InboundRequest, RequestContext, RequestEndpoint, RequestEndpointPolicy, RespondToken,
    ResponseSink,
};
use super::{AnnounceNowError, PrnsNodeApi, SendError};

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlError {
    Encode(RemoteControlMessageWriteError),
    Request(SendError<SendRequestFailure>),
    Response(RemoteControlResponseParseError),
    Remote(RemoteControlProtocolError),
    UnexpectedResponse {
        expected: RemoteControlResponseKind,
        found: RemoteControlResponseKind,
    },
    AnnounceSelf(RemoteControlAnnounceSelfFailure),
}

impl core::fmt::Display for RemoteControlError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Encode(error) => write!(
                formatter,
                "remote control request encoding failed: {error:?}"
            ),
            Self::Request(error) => write!(formatter, "remote control request failed: {error:?}"),
            Self::Response(error) => {
                write!(formatter, "remote control response was invalid: {error:?}")
            }
            Self::Remote(error) => write!(
                formatter,
                "remote control peer refused the request: {error:?}"
            ),
            Self::UnexpectedResponse { expected, found } => write!(
                formatter,
                "remote control response kind was {found:?}, expected {expected:?}"
            ),
            Self::AnnounceSelf(failure) => {
                write!(
                    formatter,
                    "remote control self-announcement failed: {failure:?}"
                )
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RemoteControlError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlAnnounceSelfFailure {
    Unavailable,
    Rejected,
    WriteFailed,
}

pub struct RemoteControlDescribe;

impl RemoteControlDescribe {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::Describe;
    pub const RESPONSE_CAPACITY: usize = RemoteControlResponse::MAX_ENCODED_LEN;
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlDescription, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::Describe(description) => Ok(description),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::Describe,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlAnnounceSelf;

impl RemoteControlAnnounceSelf {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::AnnounceSelf;
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<(), RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::AnnounceSelf(RemoteControlAnnounceSelfOutcome::Announced) => {
                Ok(())
            }
            RemoteControlResponse::AnnounceSelf(RemoteControlAnnounceSelfOutcome::Unavailable) => {
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::Unavailable,
                ))
            }
            RemoteControlResponse::AnnounceSelf(RemoteControlAnnounceSelfOutcome::Rejected) => Err(
                RemoteControlError::AnnounceSelf(RemoteControlAnnounceSelfFailure::Rejected),
            ),
            RemoteControlResponse::AnnounceSelf(RemoteControlAnnounceSelfOutcome::WriteFailed) => {
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::WriteFailed,
                ))
            }
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::AnnounceSelf,
                found: response.kind(),
            }),
        }
    }
}

struct RemoteControlRequestEndpoint;

impl RemoteControlRequestEndpoint {
    fn resolve(
        request: Result<RemoteControlRequest, RemoteControlRequestParseError>,
        available_requests: RemoteControlRequestSet,
        self_announcement: RemoteControlSelfAnnouncement,
    ) -> Result<AdmittedRemoteControlOperation, Decline> {
        match request {
            Ok(RemoteControlRequest::Describe) => {
                if !available_requests.supports(RemoteControlRequestKind::Describe) {
                    return Err(Decline::Ignore);
                }
                let description = RemoteControlDescription::try_from(available_requests).map_err(
                    |RemoteControlDescriptionError::DescribeUnavailable| Decline::Ignore,
                )?;
                Ok(AdmittedRemoteControlOperation::Describe(description))
            }
            Ok(RemoteControlRequest::AnnounceSelf) => {
                if !available_requests.supports(RemoteControlRequestKind::AnnounceSelf) {
                    return Err(Decline::Ignore);
                }
                let RemoteControlSelfAnnouncement::Destination(destination) = self_announcement
                else {
                    return Err(Decline::Ignore);
                };
                Ok(AdmittedRemoteControlOperation::AnnounceSelf { destination })
            }
            Err(error) => Ok(AdmittedRemoteControlOperation::ProtocolError(
                RemoteControlProtocolError::from(error),
            )),
        }
    }

    async fn handle_admitted<AppState>(
        mut context: RequestContext<'_, AppState>,
        node: &impl PrnsNodeApi,
        operation: AdmittedRemoteControlOperation,
    ) -> Result<(), Decline> {
        let response = match operation {
            AdmittedRemoteControlOperation::Describe(description) => {
                RemoteControlResponse::Describe(description)
            }
            AdmittedRemoteControlOperation::AnnounceSelf { destination } => {
                let outcome = match node
                    .announce_now(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    })
                    .await
                {
                    Ok(()) => RemoteControlAnnounceSelfOutcome::Announced,
                    Err(AnnounceNowError::NodeStopped | AnnounceNowError::Busy) => {
                        RemoteControlAnnounceSelfOutcome::Unavailable
                    }
                    Err(AnnounceNowError::Rejected(_)) => {
                        RemoteControlAnnounceSelfOutcome::Rejected
                    }
                    Err(AnnounceNowError::WriteFailed(_)) => {
                        RemoteControlAnnounceSelfOutcome::WriteFailed
                    }
                };
                RemoteControlResponse::AnnounceSelf(outcome)
            }
            AdmittedRemoteControlOperation::ProtocolError(error) => {
                RemoteControlResponse::ProtocolError(error)
            }
        };
        let mut out = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let encoded_len = response
            .write_into(&mut out)
            .map_err(|_| Decline::ResponseTooLarge)?;
        let encoded = out.get(..encoded_len).ok_or(Decline::ResponseTooLarge)?;
        context.respond(encoded)
    }
}

impl<AppState> RequestEndpoint<AppState> for RemoteControlRequestEndpoint {
    const ENDPOINT_ID: &'static str = REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::RequireIdentified;

    async fn handle(
        context: RequestContext<'_, AppState>,
        node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        let operation = Self::resolve(
            RemoteControlRequest::parse(context.data),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            RemoteControlSelfAnnouncement::Unavailable,
        )?;
        Self::handle_admitted(context, node, operation).await
    }
}

enum AdmittedRemoteControlOperation {
    Describe(RemoteControlDescription),
    AnnounceSelf { destination: DestinationHash },
    ProtocolError(RemoteControlProtocolError),
}

struct RemoteControlRequestBinding {
    destination: DestinationHash,
    controller: IdentityHash,
    responder: RespondToken,
    requested_at: InstantMillis,
    data: [u8; RemoteControlRequest::MAX_ENCODED_LEN],
    data_len: usize,
}

impl RemoteControlRequestBinding {
    fn new(request: &InboundRequest<'_>) -> Result<Self, Decline> {
        let Some(controller) = request.requester else {
            return Err(Decline::Ignore);
        };
        let mut data = [0; RemoteControlRequest::MAX_ENCODED_LEN];
        let Some(bound) = data.get_mut(..request.data.len()) else {
            return Err(Decline::Ignore);
        };
        bound.copy_from_slice(request.data);
        Ok(Self {
            destination: request.destination,
            controller,
            responder: request.respond_token(),
            requested_at: request.requested_at,
            data,
            data_len: request.data.len(),
        })
    }

    fn matches(&self, request: &InboundRequest<'_>) -> bool {
        self.destination == request.destination
            && request.requester == Some(self.controller)
            && self.responder == request.respond_token()
            && self.requested_at == request.requested_at
            && self
                .data
                .get(..self.data_len)
                .is_some_and(|data| data == request.data)
    }
}

pub struct AdmittedRemoteControlRequest {
    binding: RemoteControlRequestBinding,
    operation: AdmittedRemoteControlOperation,
}

pub fn admit_remote_control_request<Access>(
    access: &Access,
    supported_requests: RemoteControlRequestSet,
    self_announcement: RemoteControlSelfAnnouncement,
    request: &InboundRequest<'_>,
) -> Result<AdmittedRemoteControlRequest, Decline>
where
    Access: RemoteControlAccessTable,
{
    let binding = RemoteControlRequestBinding::new(request)?;
    let Some(grant) = access.grant_for(&binding.controller) else {
        return Err(Decline::Ignore);
    };
    let available_requests = supported_requests.intersection(grant.permitted_requests());
    let operation = RemoteControlRequestEndpoint::resolve(
        RemoteControlRequest::parse(request.data),
        available_requests,
        self_announcement,
    )?;
    Ok(AdmittedRemoteControlRequest { binding, operation })
}

pub async fn dispatch_admitted_remote_control_request<'a, AppState>(
    state: &'a AppState,
    node: &impl PrnsNodeApi,
    request: InboundRequest<'a>,
    sink: &'a mut dyn ResponseSink,
    admission: AdmittedRemoteControlRequest,
) -> Result<(), Decline> {
    if !admission.binding.matches(&request) {
        return Err(Decline::Ignore);
    }
    RemoteControlRequestEndpoint::handle_admitted(
        RequestContext::from_inbound(state, request, sink),
        node,
        admission.operation,
    )
    .await
}

pub async fn dispatch_remote_control_request<'a, AppState, Access>(
    state: &'a AppState,
    access: &Access,
    supported_requests: RemoteControlRequestSet,
    self_announcement: RemoteControlSelfAnnouncement,
    node: &impl PrnsNodeApi,
    request: InboundRequest<'a>,
    sink: &'a mut dyn ResponseSink,
) -> Result<(), Decline>
where
    Access: RemoteControlAccessTable,
{
    let admission =
        admit_remote_control_request(access, supported_requests, self_announcement, &request)?;
    dispatch_admitted_remote_control_request(state, node, request, sink, admission).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityHash, IdentityPublicKeys, IdentitySigningPublicKey,
    };
    use crate::remote_control::{
        FixedRemoteControlAccessTable, RemoteControlControllerGrant,
        RemoteControlControllerIdentity, RemoteControlProtocolVersion, RemoteControlRequestKind,
        RemoteControlRequestSet,
    };
    use crate::routing::links::request::RequestId;
    use crate::routing::links::LinkId;
    use crate::runtime::request_endpoints::InboundRequest;
    use crate::units::{InstantMillis, RttMillis};
    use crate::wire::DestinationHash;
    use core::cell::RefCell;

    struct AnnounceNode {
        result: Result<(), AnnounceNowError>,
        received: RefCell<Option<AnnounceNow>>,
    }

    impl AnnounceNode {
        fn new(result: Result<(), AnnounceNowError>) -> Self {
            Self {
                result,
                received: RefCell::new(None),
            }
        }
    }

    impl PrnsNodeApi for AnnounceNode {
        fn issue(&self, command: crate::engine::PrnsCommand) -> Option<crate::engine::CommandId> {
            <() as PrnsNodeApi>::issue(&(), command)
        }

        async fn announce_now(&self, announce: AnnounceNow) -> Result<(), AnnounceNowError> {
            self.received.replace(Some(announce));
            self.result
        }

        async fn set_registered_announce_app_data(
            &self,
            set: crate::engine::SetRegisteredAnnounceAppData,
        ) -> Result<(), super::super::SetRegisteredAnnounceAppDataError> {
            <() as PrnsNodeApi>::set_registered_announce_app_data(&(), set).await
        }

        async fn send_single_packet(
            &self,
            destination: DestinationHash,
            data: &[u8],
        ) -> Result<
            crate::engine::PacketReceiptDelivered,
            SendError<crate::engine::SendSinglePacketFailure>,
        > {
            <() as PrnsNodeApi>::send_single_packet(&(), destination, data).await
        }

        async fn send_plain_packet(
            &self,
            destination: DestinationHash,
            data: &[u8],
        ) -> Result<(), SendError<crate::engine::SendPlainPacketFailure>> {
            <() as PrnsNodeApi>::send_plain_packet(&(), destination, data).await
        }

        async fn send_group_packet(
            &self,
            destination: DestinationHash,
            data: &[u8],
        ) -> Result<(), SendError<crate::engine::SendGroupFailure>> {
            <() as PrnsNodeApi>::send_group_packet(&(), destination, data).await
        }

        fn respond_packed(
            &self,
            responder: super::super::request_endpoints::RespondToken,
            packed: &[u8],
        ) -> bool {
            <() as PrnsNodeApi>::respond_packed(&(), responder, packed)
        }

        fn close_link(&self, link_id: LinkId) -> bool {
            <() as PrnsNodeApi>::close_link(&(), link_id)
        }
    }

    fn identity(fill: u8) -> RemoteControlControllerIdentity {
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        })
    }

    fn access(allowed: RemoteControlControllerIdentity) -> FixedRemoteControlAccessTable<1> {
        access_permitting(allowed, RemoteControlRequestSet::all())
    }

    fn access_permitting(
        allowed: RemoteControlControllerIdentity,
        permitted_requests: RemoteControlRequestSet,
    ) -> FixedRemoteControlAccessTable<1> {
        let mut access = FixedRemoteControlAccessTable::default();
        access
            .upsert(RemoteControlControllerGrant::new(allowed, permitted_requests).unwrap())
            .unwrap();
        access
    }

    async fn dispatch(
        access: &impl RemoteControlAccessTable,
        requester: Option<IdentityHash>,
        data: &[u8],
        sink: &mut dyn super::super::request_endpoints::ResponseSink,
    ) -> Result<(), Decline> {
        dispatch_with_node(access, &(), requester, data, sink).await
    }

    async fn dispatch_with_node(
        access: &impl RemoteControlAccessTable,
        node: &impl PrnsNodeApi,
        requester: Option<IdentityHash>,
        data: &[u8],
        sink: &mut dyn super::super::request_endpoints::ResponseSink,
    ) -> Result<(), Decline> {
        dispatch_with_configuration(
            access,
            RemoteControlRequestSet::all(),
            RemoteControlSelfAnnouncement::Destination(DestinationHash::new([0x87; 16])),
            node,
            requester,
            data,
            sink,
        )
        .await
    }

    async fn dispatch_with_configuration(
        access: &impl RemoteControlAccessTable,
        supported_requests: RemoteControlRequestSet,
        self_announcement: RemoteControlSelfAnnouncement,
        node: &impl PrnsNodeApi,
        requester: Option<IdentityHash>,
        data: &[u8],
        sink: &mut dyn super::super::request_endpoints::ResponseSink,
    ) -> Result<(), Decline> {
        let request = InboundRequest::new(
            DestinationHash::new([0x21; 16]),
            LinkId::new([0x43; 16]),
            RequestId([0x65; 16]),
            requester,
            InstantMillis(1_000),
            RttMillis::new(20),
            data,
        );
        dispatch_remote_control_request(
            &(),
            access,
            supported_requests,
            self_announcement,
            node,
            request,
            sink,
        )
        .await
    }

    fn describe_request() -> [u8; RemoteControlRequest::Describe.encoded_len()] {
        let mut request = [0u8; RemoteControlRequest::Describe.encoded_len()];
        RemoteControlRequest::Describe
            .write_into(&mut request)
            .unwrap();
        request
    }

    fn announce_self_request() -> [u8; RemoteControlRequest::AnnounceSelf.encoded_len()] {
        let mut request = [0u8; RemoteControlRequest::AnnounceSelf.encoded_len()];
        RemoteControlRequest::AnnounceSelf
            .write_into(&mut request)
            .unwrap();
        request
    }

    #[derive(Clone, Copy)]
    struct InboundRequestFixture<'a> {
        destination: DestinationHash,
        link_id: LinkId,
        request_id: RequestId,
        requester: Option<IdentityHash>,
        requested_at: InstantMillis,
        rtt: RttMillis,
        data: &'a [u8],
    }

    impl<'a> InboundRequestFixture<'a> {
        fn inbound(self) -> InboundRequest<'a> {
            InboundRequest::new(
                self.destination,
                self.link_id,
                self.request_id,
                self.requester,
                self.requested_at,
                self.rtt,
                self.data,
            )
        }
    }

    #[test]
    fn describe_exchange_owns_its_wire_contract() {
        let mut request = [0u8; RemoteControlDescribe::REQUEST.encoded_len()];
        assert_eq!(
            RemoteControlDescribe::write_request(&mut request),
            Ok(request.len()),
        );
        assert_eq!(
            request,
            [
                RemoteControlProtocolVersion::V1.wire_value(),
                RemoteControlDescribe::REQUEST.kind().wire_value(),
            ],
        );
        assert_eq!(
            RemoteControlDescribe::MAXIMUM_RESPONSE_BYTES,
            ByteLimit::Maximum(RemoteControlResponse::MAX_ENCODED_LEN as u64),
        );

        let description =
            RemoteControlDescription::try_from(RemoteControlRequestSet::all()).unwrap();
        let response = RemoteControlResponse::Describe(description);
        let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let encoded_len = response.write_into(&mut encoded).unwrap();
        assert_eq!(
            RemoteControlDescribe::parse_response(&encoded[..encoded_len]),
            Ok(description),
        );

        let protocol_error = RemoteControlProtocolError::UnknownRequestKind { found: 0xA5 };
        let response = RemoteControlResponse::ProtocolError(protocol_error);
        let encoded_len = response.write_into(&mut encoded).unwrap();
        assert_eq!(
            RemoteControlDescribe::parse_response(&encoded[..encoded_len]),
            Err(RemoteControlError::Remote(protocol_error)),
        );
    }

    #[test]
    fn announce_self_exchange_owns_its_wire_contract() {
        let mut request = [0u8; RemoteControlAnnounceSelf::REQUEST.encoded_len()];
        assert_eq!(
            RemoteControlAnnounceSelf::write_request(&mut request),
            Ok(request.len()),
        );
        assert_eq!(
            request,
            [
                RemoteControlProtocolVersion::V1.wire_value(),
                RemoteControlAnnounceSelf::REQUEST.kind().wire_value(),
            ],
        );
        assert_eq!(
            RemoteControlAnnounceSelf::MAXIMUM_RESPONSE_BYTES,
            ByteLimit::Maximum(
                RemoteControlAnnounceSelf::REQUEST.maximum_response_encoded_len() as u64,
            ),
        );

        let cases = [
            (RemoteControlAnnounceSelfOutcome::Announced, Ok(())),
            (
                RemoteControlAnnounceSelfOutcome::Unavailable,
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::Unavailable,
                )),
            ),
            (
                RemoteControlAnnounceSelfOutcome::Rejected,
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::Rejected,
                )),
            ),
            (
                RemoteControlAnnounceSelfOutcome::WriteFailed,
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::WriteFailed,
                )),
            ),
        ];
        for (outcome, expected) in cases {
            let response = RemoteControlResponse::AnnounceSelf(outcome);
            let mut encoded = [0u8; RemoteControlAnnounceSelf::RESPONSE_CAPACITY];
            let encoded_len = response.write_into(&mut encoded).unwrap();
            assert_eq!(
                RemoteControlAnnounceSelf::parse_response(&encoded[..encoded_len]),
                expected,
            );
        }
    }

    #[test]
    fn an_admitted_announce_self_waits_for_the_exact_destination_effect() {
        futures_executor::block_on(async {
            let allowed = identity(0x31);
            let access = access(allowed);
            let node = AnnounceNode::new(Ok(()));
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch_with_node(
                    &access,
                    &node,
                    Some(allowed.identity_hash()),
                    &announce_self_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            assert_eq!(
                node.received.take(),
                Some(AnnounceNow {
                    destination: DestinationHash::new([0x87; 16]),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            );
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::AnnounceSelf(
                    RemoteControlAnnounceSelfOutcome::Announced,
                )),
            );
        });
    }

    #[test]
    fn unavailable_self_announcement_is_neither_described_nor_dispatched() {
        futures_executor::block_on(async {
            let allowed = identity(0x35);
            let access = access(allowed);
            let supported_requests =
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
            let node = AnnounceNode::new(Ok(()));
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch_with_configuration(
                    &access,
                    supported_requests,
                    RemoteControlSelfAnnouncement::Unavailable,
                    &node,
                    Some(allowed.identity_hash()),
                    &describe_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            let description = RemoteControlDescription::try_from(supported_requests).unwrap();
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::Describe(description)),
            );

            response.clear();
            assert_eq!(
                dispatch_with_configuration(
                    &access,
                    supported_requests,
                    RemoteControlSelfAnnouncement::Unavailable,
                    &node,
                    Some(allowed.identity_hash()),
                    &announce_self_request(),
                    &mut response,
                )
                .await,
                Err(Decline::Ignore),
            );
            assert!(response.is_empty());
            assert!(node.received.borrow().is_none());
        });
    }

    #[test]
    fn a_controller_grant_reaches_only_its_permitted_requests() {
        futures_executor::block_on(async {
            let allowed = identity(0x33);
            let permitted_requests =
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
            let access = access_permitting(allowed, permitted_requests);
            let node = AnnounceNode::new(Ok(()));
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch_with_node(
                    &access,
                    &node,
                    Some(allowed.identity_hash()),
                    &announce_self_request(),
                    &mut response,
                )
                .await,
                Err(Decline::Ignore),
            );
            assert!(response.is_empty());
            assert!(node.received.borrow().is_none());

            assert_eq!(
                dispatch_with_node(
                    &access,
                    &node,
                    Some(allowed.identity_hash()),
                    &describe_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            let description = RemoteControlDescription::try_from(permitted_requests).unwrap();
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::Describe(description)),
            );
        });
    }

    #[test]
    fn an_admission_binding_requires_every_inbound_request_fact_to_match() {
        let allowed = identity(0x34);
        let admitted_data = [0x73, RemoteControlRequestKind::Describe.wire_value()];
        let same_error_different_data = [0x73, RemoteControlRequestKind::AnnounceSelf.wire_value()];
        let admitted = InboundRequestFixture {
            destination: DestinationHash::new([0x21; 16]),
            link_id: LinkId::new([0x43; 16]),
            request_id: RequestId([0x65; 16]),
            requester: Some(allowed.identity_hash()),
            requested_at: InstantMillis(1_000),
            rtt: RttMillis::new(20),
            data: &admitted_data,
        };
        let binding = RemoteControlRequestBinding::new(&admitted.inbound()).unwrap();
        let changed_requests = [
            InboundRequestFixture {
                destination: DestinationHash::new([0x22; 16]),
                ..admitted
            },
            InboundRequestFixture {
                link_id: LinkId::new([0x44; 16]),
                ..admitted
            },
            InboundRequestFixture {
                request_id: RequestId([0x66; 16]),
                ..admitted
            },
            InboundRequestFixture {
                requester: Some(identity(0x35).identity_hash()),
                ..admitted
            },
            InboundRequestFixture {
                requested_at: InstantMillis(1_001),
                ..admitted
            },
            InboundRequestFixture {
                rtt: RttMillis::new(21),
                ..admitted
            },
            InboundRequestFixture {
                data: &same_error_different_data,
                ..admitted
            },
        ];

        assert!(binding.matches(&admitted.inbound()));
        for changed in changed_requests {
            assert!(!binding.matches(&changed.inbound()));
        }
    }

    #[test]
    fn oversized_requests_cannot_enter_the_admitted_state() {
        let allowed = identity(0x36);
        let data = [0x73; RemoteControlRequest::MAX_ENCODED_LEN + 1];
        let request = InboundRequestFixture {
            destination: DestinationHash::new([0x21; 16]),
            link_id: LinkId::new([0x43; 16]),
            request_id: RequestId([0x65; 16]),
            requester: Some(allowed.identity_hash()),
            requested_at: InstantMillis(1_000),
            rtt: RttMillis::new(20),
            data: &data,
        };

        assert!(matches!(
            RemoteControlRequestBinding::new(&request.inbound()),
            Err(Decline::Ignore),
        ));
    }

    #[test]
    fn announce_self_effect_failures_are_stable_wire_outcomes() {
        futures_executor::block_on(async {
            let allowed = identity(0x32);
            let access = access(allowed);
            let cases = [
                (
                    AnnounceNowError::NodeStopped,
                    RemoteControlAnnounceSelfOutcome::Unavailable,
                ),
                (
                    AnnounceNowError::Busy,
                    RemoteControlAnnounceSelfOutcome::Unavailable,
                ),
                (
                    AnnounceNowError::Rejected(
                        crate::engine::AnnounceNowRejection::UnknownDestination,
                    ),
                    RemoteControlAnnounceSelfOutcome::Rejected,
                ),
                (
                    AnnounceNowError::WriteFailed(crate::engine::AnnounceWriteFailure::Rejected(
                        crate::engine::AnnounceRejection::NotRegistered,
                    )),
                    RemoteControlAnnounceSelfOutcome::WriteFailed,
                ),
            ];

            for (failure, expected) in cases {
                let node = AnnounceNode::new(Err(failure));
                let mut response =
                    heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
                assert_eq!(
                    dispatch_with_node(
                        &access,
                        &node,
                        Some(allowed.identity_hash()),
                        &announce_self_request(),
                        &mut response,
                    )
                    .await,
                    Ok(()),
                );
                assert_eq!(
                    RemoteControlResponse::parse(response.as_slice()),
                    Ok(RemoteControlResponse::AnnounceSelf(expected)),
                );
            }
        });
    }

    #[test]
    fn the_endpoint_requires_an_identified_requester_before_access_is_checked() {
        assert_eq!(
            <RemoteControlRequestEndpoint as RequestEndpoint<()>>::POLICY,
            RequestEndpointPolicy::RequireIdentified,
        );
    }

    #[test]
    fn an_admitted_identity_receives_only_its_available_requests() {
        futures_executor::block_on(async {
            let allowed = identity(0x21);
            let available_requests =
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
            let access = access_permitting(allowed, available_requests);
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch(
                    &access,
                    Some(allowed.identity_hash()),
                    &describe_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            let description = RemoteControlDescription::try_from(available_requests).unwrap();
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::Describe(description)),
            );
        });
    }

    #[test]
    fn unidentified_and_unlisted_requesters_cannot_reach_remote_control() {
        futures_executor::block_on(async {
            let access = access(identity(0x43));
            let node = AnnounceNode::new(Ok(()));

            for requester in [None, Some(identity(0x65).identity_hash())] {
                let mut response =
                    heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
                assert_eq!(
                    dispatch_with_node(
                        &access,
                        &node,
                        requester,
                        &announce_self_request(),
                        &mut response,
                    )
                    .await,
                    Err(Decline::Ignore),
                );
                assert!(response.is_empty());
                assert!(node.received.borrow().is_none());
            }
        });
    }

    #[test]
    fn admitted_protocol_failures_receive_typed_errors() {
        futures_executor::block_on(async {
            let allowed = identity(0x87);
            let access = access(allowed);
            let unsupported_version = 0x73;
            let unknown_request_kind = 0x95;
            let cases = [
                (&[][..], RemoteControlProtocolError::MalformedRequest),
                (
                    &[
                        unsupported_version,
                        RemoteControlRequestKind::Describe.wire_value(),
                    ][..],
                    RemoteControlProtocolError::UnsupportedVersion {
                        found: unsupported_version,
                    },
                ),
                (
                    &[
                        RemoteControlProtocolVersion::V1.wire_value(),
                        unknown_request_kind,
                    ][..],
                    RemoteControlProtocolError::UnknownRequestKind {
                        found: unknown_request_kind,
                    },
                ),
            ];

            for (request, expected) in cases {
                let mut response =
                    heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
                assert_eq!(
                    dispatch(
                        &access,
                        Some(allowed.identity_hash()),
                        request,
                        &mut response,
                    )
                    .await,
                    Ok(()),
                );
                assert_eq!(
                    RemoteControlResponse::parse(response.as_slice()),
                    Ok(RemoteControlResponse::ProtocolError(expected)),
                );
            }
        });
    }
}
