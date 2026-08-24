use crate::engine::SendRequestFailure;
use crate::remote_control::{
    RemoteControlAccessTable, RemoteControlDescription, RemoteControlMessageWriteError,
    RemoteControlProtocolError, RemoteControlRequest, RemoteControlResponse,
    RemoteControlResponseParseError,
};
use crate::units::ByteLimit;

use super::request_endpoints::{Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy};
use super::SendError;

pub const REMOTE_CONTROL_ENDPOINT_ID: &str = "/remote-control";

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlError {
    Encode(RemoteControlMessageWriteError),
    Request(SendError<SendRequestFailure>),
    Response(RemoteControlResponseParseError),
    Remote(RemoteControlProtocolError),
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
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RemoteControlError {}

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
        }
    }
}

pub trait RemoteControlEndpointState {
    type AccessTable: RemoteControlAccessTable;

    fn remote_control_access(&self) -> &Self::AccessTable;
    fn remote_control_description(&self) -> RemoteControlDescription;
}

pub struct RemoteControlEndpoint;

impl<AppState: RemoteControlEndpointState> RequestEndpoint<AppState> for RemoteControlEndpoint {
    const ENDPOINT_ID: &'static str = REMOTE_CONTROL_ENDPOINT_ID;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::RequireIdentified;

    async fn handle(mut context: RequestContext<'_, AppState>) -> Result<(), Decline> {
        let Some(requester) = context.requester else {
            return Err(Decline::Ignore);
        };
        if !context.state.remote_control_access().contains(&requester) {
            return Err(Decline::Ignore);
        }
        let response = match RemoteControlRequest::parse(context.data) {
            Ok(RemoteControlRequest::Describe) => {
                RemoteControlResponse::Describe(context.state.remote_control_description())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityHash, IdentityPublicKeys, IdentitySigningPublicKey,
    };
    use crate::persistence::{
        read_remote_control_access_snapshot, write_remote_control_access_snapshot,
    };
    use crate::remote_control::{
        FixedRemoteControlAccessTable, RemoteControlIdentity, RemoteControlProtocolVersion,
        RemoteControlRequestKind,
    };
    use crate::routing::links::request::RequestId;
    use crate::routing::links::LinkId;
    use crate::routing::request_handlers::RequestPathHash;
    use crate::runtime::request_endpoints::{dispatch_request, InboundRequest, RequestEndpointSet};
    use crate::units::{InstantMillis, RttMillis};
    use crate::wire::DestinationHash;

    struct State {
        access: FixedRemoteControlAccessTable<2>,
        description: RemoteControlDescription,
    }

    impl RemoteControlEndpointState for State {
        type AccessTable = FixedRemoteControlAccessTable<2>;

        fn remote_control_access(&self) -> &Self::AccessTable {
            &self.access
        }

        fn remote_control_description(&self) -> RemoteControlDescription {
            self.description
        }
    }

    fn identity(fill: u8) -> RemoteControlIdentity {
        RemoteControlIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        })
    }

    fn state(allowed: RemoteControlIdentity) -> State {
        let mut persisted = [0u8; 82];
        let persisted_len =
            write_remote_control_access_snapshot(core::iter::once(allowed), &mut persisted)
                .unwrap();
        let mut access = FixedRemoteControlAccessTable::default();
        for identity in read_remote_control_access_snapshot(&persisted[..persisted_len]).unwrap() {
            access.upsert(identity).unwrap();
        }
        State {
            access,
            description: RemoteControlDescription::default(),
        }
    }

    async fn dispatch<R: RequestEndpointSet<State>>(
        _endpoints: &R,
        state: &State,
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
        dispatch_request::<State, R>(
            state,
            RequestPathHash::of(REMOTE_CONTROL_ENDPOINT_ID),
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

        let response = RemoteControlResponse::Describe(RemoteControlDescription::default());
        let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let encoded_len = response.write_into(&mut encoded).unwrap();
        assert_eq!(
            RemoteControlDescribe::parse_response(&encoded[..encoded_len]),
            Ok(RemoteControlDescription::default()),
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
    fn the_endpoint_requires_an_identified_requester_at_registration() {
        assert_eq!(
            <RemoteControlEndpoint as RequestEndpoint<State>>::POLICY,
            RequestEndpointPolicy::RequireIdentified,
        );
    }

    #[test]
    fn a_restored_authorized_identity_receives_the_description() {
        futures_executor::block_on(async {
            let allowed = identity(0x21);
            let state = state(allowed);
            let endpoints = crate::request_endpoints![RemoteControlEndpoint];
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch(
                    &endpoints,
                    &state,
                    Some(allowed.identity_hash()),
                    &describe_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            let RemoteControlResponse::Describe(description) =
                RemoteControlResponse::parse(response.as_slice()).unwrap()
            else {
                panic!("describe response");
            };
            assert!(description
                .supported_requests()
                .supports(RemoteControlRequestKind::Describe));
        });
    }

    #[test]
    fn unidentified_and_unlisted_requesters_are_silently_refused() {
        futures_executor::block_on(async {
            let state = state(identity(0x43));
            let endpoints = crate::request_endpoints![RemoteControlEndpoint];

            for requester in [None, Some(identity(0x65).identity_hash())] {
                let mut response =
                    heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
                assert_eq!(
                    dispatch(
                        &endpoints,
                        &state,
                        requester,
                        &describe_request(),
                        &mut response,
                    )
                    .await,
                    Err(Decline::Ignore),
                );
                assert!(response.is_empty());
            }
        });
    }

    #[test]
    fn authorized_protocol_failures_receive_typed_errors() {
        futures_executor::block_on(async {
            let allowed = identity(0x87);
            let state = state(allowed);
            let endpoints = crate::request_endpoints![RemoteControlEndpoint];
            let cases = [
                (&[][..], RemoteControlProtocolError::MalformedRequest),
                (
                    &[0x73, 0x01][..],
                    RemoteControlProtocolError::UnsupportedVersion { found: 0x73 },
                ),
                (
                    &[0x01, 0x95][..],
                    RemoteControlProtocolError::UnknownRequestKind { found: 0x95 },
                ),
            ];

            for (request, expected) in cases {
                let mut response =
                    heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
                assert_eq!(
                    dispatch(
                        &endpoints,
                        &state,
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
