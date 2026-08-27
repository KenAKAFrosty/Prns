use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, SendRequestFailure};
use crate::remote_control::{
    RemoteControlAccessTable, RemoteControlAnnounceOutcome, RemoteControlDescription,
    RemoteControlDescriptionError, RemoteControlMessageWriteError, RemoteControlProtocolError,
    RemoteControlRequest, RemoteControlRequestParseError, RemoteControlRequestSet,
    RemoteControlResponse, RemoteControlResponseKind, RemoteControlResponseParseError,
    REMOTE_CONTROL_REQUEST_ENDPOINT_ID,
};
use crate::units::ByteLimit;

use super::request_endpoints::{
    Decline, InboundRequest, RequestContext, RequestEndpoint, RequestEndpointPolicy, ResponseSink,
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
    Announce(RemoteControlAnnounceFailure),
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
            Self::Announce(failure) => {
                write!(formatter, "remote control announce failed: {failure:?}")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RemoteControlError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlAnnounceFailure {
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

pub struct RemoteControlAnnounce;

impl RemoteControlAnnounce {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::Announce;
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
            RemoteControlResponse::Announce(RemoteControlAnnounceOutcome::Announced) => Ok(()),
            RemoteControlResponse::Announce(RemoteControlAnnounceOutcome::Unavailable) => Err(
                RemoteControlError::Announce(RemoteControlAnnounceFailure::Unavailable),
            ),
            RemoteControlResponse::Announce(RemoteControlAnnounceOutcome::Rejected) => Err(
                RemoteControlError::Announce(RemoteControlAnnounceFailure::Rejected),
            ),
            RemoteControlResponse::Announce(RemoteControlAnnounceOutcome::WriteFailed) => Err(
                RemoteControlError::Announce(RemoteControlAnnounceFailure::WriteFailed),
            ),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::Announce,
                found: response.kind(),
            }),
        }
    }
}

struct RemoteControlRequestEndpoint;

impl RemoteControlRequestEndpoint {
    async fn handle_parsed<AppState>(
        mut context: RequestContext<'_, AppState>,
        node: &impl PrnsNodeApi,
        request: Result<RemoteControlRequest, RemoteControlRequestParseError>,
        available_requests: RemoteControlRequestSet,
    ) -> Result<(), Decline> {
        let response = match request {
            Ok(RemoteControlRequest::Describe) => {
                let description = RemoteControlDescription::try_from(available_requests).map_err(
                    |RemoteControlDescriptionError::DescribeUnavailable| Decline::Ignore,
                )?;
                RemoteControlResponse::Describe(description)
            }
            Ok(RemoteControlRequest::Announce) => {
                let outcome = match node
                    .announce_now(AnnounceNow {
                        destination: context.destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    })
                    .await
                {
                    Ok(()) => RemoteControlAnnounceOutcome::Announced,
                    Err(AnnounceNowError::NodeStopped | AnnounceNowError::Busy) => {
                        RemoteControlAnnounceOutcome::Unavailable
                    }
                    Err(AnnounceNowError::Rejected(_)) => RemoteControlAnnounceOutcome::Rejected,
                    Err(AnnounceNowError::WriteFailed(_)) => {
                        RemoteControlAnnounceOutcome::WriteFailed
                    }
                };
                RemoteControlResponse::Announce(outcome)
            }
            Err(error) => {
                RemoteControlResponse::ProtocolError(RemoteControlProtocolError::from(error))
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
        let request = RemoteControlRequest::parse(context.data);
        Self::handle_parsed(context, node, request, RemoteControlRequestSet::all()).await
    }
}

pub async fn dispatch_remote_control_request<'a, AppState, Access>(
    state: &'a AppState,
    access: &Access,
    node: &impl PrnsNodeApi,
    request: InboundRequest<'a>,
    sink: &'a mut dyn ResponseSink,
) -> Result<(), Decline>
where
    Access: RemoteControlAccessTable,
{
    let Some(requester) = request.requester else {
        return Err(Decline::Ignore);
    };
    let Some(grant) = access.grant_for(&requester) else {
        return Err(Decline::Ignore);
    };
    let parsed = RemoteControlRequest::parse(request.data);
    if parsed
        .as_ref()
        .is_ok_and(|request| !grant.permits(request.kind()))
    {
        return Err(Decline::Ignore);
    }
    let available_requests =
        RemoteControlRequestSet::all().intersection(grant.permitted_requests());
    RemoteControlRequestEndpoint::handle_parsed(
        RequestContext::from_inbound(state, request, sink),
        node,
        parsed,
        available_requests,
    )
    .await
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
        let request = InboundRequest::new(
            DestinationHash::new([0x21; 16]),
            LinkId::new([0x43; 16]),
            RequestId([0x65; 16]),
            requester,
            InstantMillis(1_000),
            RttMillis::new(20),
            data,
        );
        dispatch_remote_control_request(&(), access, node, request, sink).await
    }

    fn describe_request() -> [u8; RemoteControlRequest::Describe.encoded_len()] {
        let mut request = [0u8; RemoteControlRequest::Describe.encoded_len()];
        RemoteControlRequest::Describe
            .write_into(&mut request)
            .unwrap();
        request
    }

    fn announce_request() -> [u8; RemoteControlRequest::Announce.encoded_len()] {
        let mut request = [0u8; RemoteControlRequest::Announce.encoded_len()];
        RemoteControlRequest::Announce
            .write_into(&mut request)
            .unwrap();
        request
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
    fn announce_exchange_owns_its_wire_contract() {
        let mut request = [0u8; RemoteControlAnnounce::REQUEST.encoded_len()];
        assert_eq!(
            RemoteControlAnnounce::write_request(&mut request),
            Ok(request.len()),
        );
        assert_eq!(
            request,
            [
                RemoteControlProtocolVersion::V1.wire_value(),
                RemoteControlAnnounce::REQUEST.kind().wire_value(),
            ],
        );
        assert_eq!(
            RemoteControlAnnounce::MAXIMUM_RESPONSE_BYTES,
            ByteLimit::Maximum(
                RemoteControlAnnounce::REQUEST.maximum_response_encoded_len() as u64,
            ),
        );

        let cases = [
            (RemoteControlAnnounceOutcome::Announced, Ok(())),
            (
                RemoteControlAnnounceOutcome::Unavailable,
                Err(RemoteControlError::Announce(
                    RemoteControlAnnounceFailure::Unavailable,
                )),
            ),
            (
                RemoteControlAnnounceOutcome::Rejected,
                Err(RemoteControlError::Announce(
                    RemoteControlAnnounceFailure::Rejected,
                )),
            ),
            (
                RemoteControlAnnounceOutcome::WriteFailed,
                Err(RemoteControlError::Announce(
                    RemoteControlAnnounceFailure::WriteFailed,
                )),
            ),
        ];
        for (outcome, expected) in cases {
            let response = RemoteControlResponse::Announce(outcome);
            let mut encoded = [0u8; RemoteControlAnnounce::RESPONSE_CAPACITY];
            let encoded_len = response.write_into(&mut encoded).unwrap();
            assert_eq!(
                RemoteControlAnnounce::parse_response(&encoded[..encoded_len]),
                expected,
            );
        }
    }

    #[test]
    fn an_admitted_announce_waits_for_the_exact_destination_effect() {
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
                    &announce_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            assert_eq!(
                node.received.take(),
                Some(AnnounceNow {
                    destination: DestinationHash::new([0x21; 16]),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            );
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::Announce(
                    RemoteControlAnnounceOutcome::Announced,
                )),
            );
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
                    &announce_request(),
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
    fn announce_effect_failures_are_stable_wire_outcomes() {
        futures_executor::block_on(async {
            let allowed = identity(0x32);
            let access = access(allowed);
            let cases = [
                (
                    AnnounceNowError::NodeStopped,
                    RemoteControlAnnounceOutcome::Unavailable,
                ),
                (
                    AnnounceNowError::Busy,
                    RemoteControlAnnounceOutcome::Unavailable,
                ),
                (
                    AnnounceNowError::Rejected(
                        crate::engine::AnnounceNowRejection::UnknownDestination,
                    ),
                    RemoteControlAnnounceOutcome::Rejected,
                ),
                (
                    AnnounceNowError::WriteFailed(crate::engine::AnnounceWriteFailure::Rejected(
                        crate::engine::AnnounceRejection::NotRegistered,
                    )),
                    RemoteControlAnnounceOutcome::WriteFailed,
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
                        &announce_request(),
                        &mut response,
                    )
                    .await,
                    Ok(()),
                );
                assert_eq!(
                    RemoteControlResponse::parse(response.as_slice()),
                    Ok(RemoteControlResponse::Announce(expected)),
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
                        &announce_request(),
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
