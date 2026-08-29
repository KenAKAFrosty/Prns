use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Receiver;
use heapless::Vec as HeaplessVec;

use crate::engine::{InstantMillis, Journaled, RespondData};
use crate::identity::IdentityHash;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;
use crate::wire::DestinationHash;
use prns_runtime::runtime::placement::dispatch_remote_control_request;

use super::node_facade::PrnsNodeHandle;
use super::remote_control_access::{RemoteControlAccessCommand, RemoteControlAccessCompletion};
use super::request_endpoints::{
    dispatch_request, Decline, InboundRequest, RequestEndpointSet, ResponseCapacityExceeded,
    ResponseSink,
};
use super::AssembledRemoteControl;

#[allow(clippy::large_enum_variant)]
enum RunnerResponse {
    Buffered(RespondData),
    StaticBytes(&'static [u8]),
    #[cfg(feature = "large-static-responses")]
    StaticFile {
        name: &'static str,
        bytes: &'static [u8],
    },
}

impl ResponseSink for RunnerResponse {
    fn put_packed(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) => body
                .extend_from_slice(bytes)
                .map_err(|()| ResponseCapacityExceeded),
            RunnerResponse::StaticBytes(_) => Err(ResponseCapacityExceeded),
            #[cfg(feature = "large-static-responses")]
            RunnerResponse::StaticFile { .. } => Err(ResponseCapacityExceeded),
        }
    }

    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) => ResponseSink::put_bytes(body, bytes),
            RunnerResponse::StaticBytes(_) => Err(ResponseCapacityExceeded),
            #[cfg(feature = "large-static-responses")]
            RunnerResponse::StaticFile { .. } => Err(ResponseCapacityExceeded),
        }
    }

    fn put_static_bytes(&mut self, bytes: &'static [u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) if body.is_empty() => {
                *self = RunnerResponse::StaticBytes(bytes);
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }

    #[cfg(feature = "large-static-responses")]
    fn put_static_file(
        &mut self,
        name: &'static str,
        bytes: &'static [u8],
    ) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) if body.is_empty() => {
                *self = RunnerResponse::StaticFile { name, bytes };
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }
}

pub(super) struct RunnerRequest<const N: usize> {
    destination: DestinationHash,
    link_id: LinkId,
    request_id: RequestId,
    requester: Option<IdentityHash>,
    path_hash: RequestPathHash,
    requested_at: InstantMillis,
    rtt: RttMillis,
    data: HeaplessVec<u8, N>,
}

impl<const N: usize> RunnerRequest<N> {
    pub(super) fn copy_from(journaled: &Journaled<'_>) -> Option<Self> {
        let Journaled::RequestReceived {
            destination,
            link_id,
            request_id,
            requester,
            path_hash,
            requested_at,
            rtt,
            data,
        } = journaled
        else {
            return None;
        };
        Some(Self {
            destination: *destination,
            link_id: *link_id,
            request_id: *request_id,
            requester: *requester,
            path_hash: *path_hash,
            requested_at: *requested_at,
            rtt: *rtt,
            data: HeaplessVec::from_slice(data).ok()?,
        })
    }
}

pub(super) async fn run_router<
    St,
    R,
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
    const REQUESTS: usize,
    const REQUEST_BYTES: usize,
>(
    state: &St,
    remote_control: &mut AssembledRemoteControl,
    requests: Receiver<'_, M, RunnerRequest<REQUEST_BYTES>, REQUESTS>,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) where
    R: RequestEndpointSet<St>,
    M: RawMutex,
{
    loop {
        match select(
            commands.next_remote_control_access_command(),
            requests.receive(),
        )
        .await
        {
            Either::First(RemoteControlAccessCommand::SetControllerGrant { id, grant }) => {
                let outcome = remote_control.set_controller_grant(grant);
                let _settled = commands.settle_remote_control_access(
                    id,
                    RemoteControlAccessCompletion::ControllerGrantSet(outcome),
                );
            }
            Either::First(RemoteControlAccessCommand::RevokeController { id, controller }) => {
                let outcome = remote_control.revoke_controller(&controller);
                let _settled = commands.settle_remote_control_access(
                    id,
                    RemoteControlAccessCompletion::ControllerRevoked(outcome),
                );
            }
            Either::Second(request) => {
                dispatch::<
                    St,
                    R,
                    M,
                    COMMANDS,
                    COMPLETIONS,
                    REQUEST_COMPLETIONS,
                    RESPONSE_BYTES,
                    REQUEST_BYTES,
                >(state, remote_control, commands, request)
                .await;
            }
        }
    }
}

async fn dispatch<
    St,
    R,
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
    const REQUEST_BYTES: usize,
>(
    state: &St,
    remote_control: &AssembledRemoteControl,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    request: RunnerRequest<REQUEST_BYTES>,
) where
    R: RequestEndpointSet<St>,
    M: RawMutex,
{
    let inbound = InboundRequest::new(
        request.destination,
        request.link_id,
        request.request_id,
        request.requester,
        request.requested_at,
        request.rtt,
        &request.data,
    );
    let responder = inbound.respond_token();
    let mut body = RunnerResponse::Buffered(RespondData::new());
    let dispatched = if let Some((access, available_requests, self_announcement)) =
        remote_control.request_configuration(request.destination, request.path_hash)
    {
        dispatch_remote_control_request(
            state,
            access,
            available_requests,
            self_announcement,
            &commands,
            inbound,
            &mut body,
        )
        .await
    } else {
        dispatch_request::<St, R>(state, &commands, request.path_hash, inbound, &mut body).await
    };
    match dispatched {
        Ok(()) => match body {
            RunnerResponse::Buffered(body) => {
                commands.respond_owned_packed(responder, body);
            }
            RunnerResponse::StaticBytes(bytes) => {
                commands.respond_static_bytes(responder, bytes);
            }
            #[cfg(feature = "large-static-responses")]
            RunnerResponse::StaticFile { name, bytes } => {
                commands.respond_static_file(responder, name, bytes);
            }
        },
        Err(Decline::Ignore | Decline::ResponseTooLarge) => {}
        Err(Decline::CloseLink) => {
            commands.close_link(responder.link_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineState, PrnsCommand};
    use crate::runtime::request_endpoints::{
        RequestContext, RequestEndpoint, RequestEndpointPolicy,
    };
    use embassy_futures::block_on;
    use embassy_futures::select::{select, Either};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use prns_core::storage::GrowableHeap;

    fn remote_control() -> AssembledRemoteControl {
        let mut engine = EngineState::<GrowableHeap>::default();
        crate::runtime::configure_remote_control_service(
            &mut engine,
            super::super::node_facade::test_remote_control_service(),
        )
        .expect("RemoteControl fits growable storage")
    }

    struct DestinationEcho;
    struct DestinationRoutes;

    impl RequestEndpoint<()> for DestinationEcho {
        const ENDPOINT_ID: &'static str = "/destination";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

        async fn handle(
            mut context: RequestContext<'_, ()>,
            _node: &impl crate::runtime::PrnsNodeApi,
        ) -> Result<(), Decline> {
            let destination = context.destination;
            context.respond(destination.as_bytes())
        }
    }

    impl RequestEndpointSet<()> for DestinationRoutes {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
            &[(DestinationEcho::ENDPOINT_ID, DestinationEcho::POLICY)];

        async fn dispatch(
            context: RequestContext<'_, ()>,
            node: &impl crate::runtime::PrnsNodeApi,
            path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            if path_hash == RequestPathHash::of(DestinationEcho::ENDPOINT_ID) {
                DestinationEcho::handle(context, node).await
            } else {
                Err(Decline::Ignore)
            }
        }
    }

    struct StaticPage;
    struct StaticRoutes;
    static PAGE: [u8; 1200] = [0x21; 1200];

    #[cfg(feature = "large-static-responses")]
    #[test]
    fn static_file_sink_preserves_filename_and_borrowed_bytes() {
        let mut response = RunnerResponse::Buffered(RespondData::new());
        ResponseSink::put_static_file(&mut response, "source.zip", &PAGE).unwrap();
        let RunnerResponse::StaticFile { name, bytes } = response else {
            panic!("static file response");
        };
        assert_eq!(name, "source.zip");
        assert_eq!(bytes.as_ptr(), PAGE.as_ptr());
    }

    impl RequestEndpoint<()> for StaticPage {
        const ENDPOINT_ID: &'static str = "/page";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

        async fn handle(
            mut context: RequestContext<'_, ()>,
            _node: &impl crate::runtime::PrnsNodeApi,
        ) -> Result<(), Decline> {
            context.respond_static_messagepack_bytes(&PAGE)
        }
    }

    impl RequestEndpointSet<()> for StaticRoutes {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
            &[(StaticPage::ENDPOINT_ID, StaticPage::POLICY)];

        async fn dispatch(
            context: RequestContext<'_, ()>,
            node: &impl crate::runtime::PrnsNodeApi,
            path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            if path_hash == RequestPathHash::of(StaticPage::ENDPOINT_ID) {
                StaticPage::handle(context, node).await
            } else {
                Err(Decline::Ignore)
            }
        }
    }

    #[test]
    fn dispatch_hands_a_borrowed_body_to_the_borrowed_lane() {
        type M = CriticalSectionRawMutex;
        let channel = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 1>::new();
        let handle = PrnsNodeHandle::new(channel.sender(), &completions);
        let remote_control = remote_control();
        let request = RunnerRequest {
            destination: DestinationHash::new([0x5A; 16]),
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            requester: None,
            path_hash: RequestPathHash::of("/page"),
            requested_at: InstantMillis(3),
            rtt: RttMillis::new(4),
            data: HeaplessVec::<u8, 16>::new(),
        };

        block_on(dispatch::<(), StaticRoutes, M, 1, 1, 0, 0, 16>(
            &(),
            &remote_control,
            handle,
            request,
        ));

        let Ok(issued) = channel.try_receive() else {
            panic!("response command");
        };
        let PrnsCommand::Respond(response) = issued.command else {
            panic!("respond command");
        };
        assert_eq!(response.link_id, LinkId::new([1; 16]));
        assert_eq!(response.request_id, RequestId([2; 16]));
        let crate::engine::RespondPayload::StaticBytes(data) = response.payload else {
            panic!("static response");
        };
        assert_eq!(data.as_ptr(), PAGE.as_ptr());
        assert_eq!(data.len(), PAGE.len());
    }

    #[test]
    fn dispatch_answers_through_the_embassy_command_lane() {
        type M = CriticalSectionRawMutex;
        let channel = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 1>::new();
        let handle = PrnsNodeHandle::new(channel.sender(), &completions);
        let remote_control = remote_control();
        let destination = DestinationHash::new([0x5a; 16]);
        let request = RunnerRequest {
            destination,
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            requester: None,
            path_hash: RequestPathHash::of("/destination"),
            requested_at: InstantMillis(3),
            rtt: RttMillis::new(4),
            data: HeaplessVec::<u8, 16>::new(),
        };

        block_on(dispatch::<(), DestinationRoutes, M, 1, 1, 0, 0, 16>(
            &(),
            &remote_control,
            handle,
            request,
        ));

        let Ok(issued) = channel.try_receive() else {
            panic!("response command");
        };
        let PrnsCommand::Respond(response) = issued.command else {
            panic!("respond command");
        };
        let crate::engine::RespondPayload::Packed(data) = response.payload else {
            panic!("packed response");
        };
        assert_eq!(data.as_slice(), destination.as_bytes());
    }

    #[test]
    fn unavailable_remote_control_rejects_access_changes() {
        use crate::remote_control::RemoteControlRequestKind;
        use crate::runtime::{
            RemoteControlAccessControl, RevokeRemoteControlControllerControlError,
            SetRemoteControlControllerGrantControlError,
        };

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests = Channel::<M, RunnerRequest<16>, 1>::new();
        let mut engine = EngineState::<GrowableHeap>::default();
        let mut remote_control = crate::runtime::configure_remote_control_service(
            &mut engine,
            crate::remote_control::RemoteControlService::Unavailable,
        )
        .expect("unavailable RemoteControl requires no storage");
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let router = run_router::<(), (), M, 1, 0, 0, 0, 1, 16>(
            &(),
            &mut remote_control,
            requests.receiver(),
            handle,
        );
        let exercise = async {
            assert_eq!(
                handle.set_remote_control_controller_grant(grant).await,
                Err(SetRemoteControlControllerGrantControlError::Unavailable),
            );
            assert_eq!(
                handle
                    .revoke_remote_control_controller(*grant.controller())
                    .await,
                Err(RevokeRemoteControlControllerControlError::Unavailable),
            );
        };

        match block_on(select(exercise, router)) {
            Either::First(()) => {}
            Either::Second(()) => panic!("router returned"),
        }
    }

    #[test]
    fn router_applies_ready_remote_control_access_before_a_ready_request() {
        use crate::remote_control::{
            RemoteControlAccessTable, RemoteControlDescription, RemoteControlRequest,
            RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlResponse,
            RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
        };
        use crate::runtime::RemoteControlAccessControl;

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests = Channel::<M, RunnerRequest<16>, 1>::new();
        let mut remote_control = remote_control();
        let destination = remote_control.target_endpoint().unwrap().destination_hash();
        let path_hash = remote_control.request_endpoint_id().unwrap();
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let mut request_bytes = [0; RemoteControlRequest::MAX_ENCODED_LEN];
        let request_len = RemoteControlRequest::Describe
            .write_into(&mut request_bytes)
            .unwrap();
        let request = RunnerRequest {
            destination,
            link_id: LinkId::new([0x71; 16]),
            request_id: RequestId([0x72; 16]),
            requester: Some(grant.controller().identity_hash()),
            path_hash,
            requested_at: InstantMillis(73),
            rtt: RttMillis::new(74),
            data: HeaplessVec::from_slice(&request_bytes[..request_len]).unwrap(),
        };
        assert!(requests.try_send(request).is_ok());
        let router = run_router::<(), DestinationRoutes, M, 1, 0, 0, 0, 1, 16>(
            &(),
            &mut remote_control,
            requests.receiver(),
            handle,
        );
        let exercise = async {
            assert_eq!(
                handle.set_remote_control_controller_grant(grant).await,
                Ok(SetRemoteControlControllerGrantOutcome::Added),
            );
            let issued = commands.receiver().receive().await;
            let PrnsCommand::Respond(response) = issued.command else {
                panic!("RemoteControl response command")
            };
            let crate::engine::RespondPayload::Packed(data) = response.payload else {
                panic!("packed RemoteControl response")
            };
            let expected = RemoteControlDescription::try_from(RemoteControlRequestSet::only(
                RemoteControlRequestKind::Describe,
            ))
            .unwrap();
            assert_eq!(
                RemoteControlResponse::parse(data.as_slice()),
                Ok(RemoteControlResponse::Describe(expected)),
            );
            assert_eq!(
                handle
                    .revoke_remote_control_controller(*grant.controller())
                    .await,
                Ok(RevokeRemoteControlControllerOutcome::Revoked { grant }),
            );
        };

        match block_on(select(exercise, router)) {
            Either::First(()) => {}
            Either::Second(()) => panic!("router returned"),
        }
        assert!(remote_control.access().unwrap().is_empty());
    }
}
