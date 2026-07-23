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

use super::node_facade::PrnsNodeHandle;
use super::request_router::{
    dispatch_request, Decline, InboundRequest, ResponseCapacityExceeded, ResponseSink, RouteSet,
};

#[allow(clippy::large_enum_variant)]
enum RunnerResponse {
    Buffered(RespondData),
    StaticBytes(&'static [u8]),
}

impl ResponseSink for RunnerResponse {
    fn put_packed(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) => body
                .extend_from_slice(bytes)
                .map_err(|()| ResponseCapacityExceeded),
            RunnerResponse::StaticBytes(_) => Err(ResponseCapacityExceeded),
        }
    }

    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) => ResponseSink::put_bytes(body, bytes),
            RunnerResponse::StaticBytes(_) => Err(ResponseCapacityExceeded),
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
    const REQUESTS: usize,
    const REQUEST_BYTES: usize,
>(
    state: &St,
    requests: Receiver<'_, M, RunnerRequest<REQUEST_BYTES>, REQUESTS>,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS>,
) where
    R: RouteSet<St>,
    M: RawMutex,
{
    loop {
        dispatch::<St, R, M, COMMANDS, COMPLETIONS, REQUEST_BYTES>(
            state,
            commands,
            requests.receive().await,
        )
        .await;
    }
}

async fn dispatch<
    St,
    R,
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_BYTES: usize,
>(
    state: &St,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS>,
    request: RunnerRequest<REQUEST_BYTES>,
) where
    R: RouteSet<St>,
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
    match dispatch_request::<St, R>(state, request.path_hash, inbound, &mut body).await {
        Ok(()) => match body {
            RunnerResponse::Buffered(body) => {
                commands.respond_owned_packed(responder, body);
            }
            RunnerResponse::StaticBytes(bytes) => {
                commands.respond_static_bytes(responder, bytes);
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
    use crate::engine::EngineCommand;
    use crate::runtime::request_router::{RequestContext, RequestRoute, RoutePolicy};
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;

    struct DestinationEcho;
    struct DestinationRoutes;

    impl RequestRoute<()> for DestinationEcho {
        const PATH: &'static str = "/destination";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;

        async fn handle(mut context: RequestContext<'_, ()>) -> Result<(), Decline> {
            let destination = context.destination;
            context.respond_packed(destination.as_bytes())
        }
    }

    impl RouteSet<()> for DestinationRoutes {
        const REGISTRATIONS: &'static [(&'static str, RoutePolicy)] =
            &[(DestinationEcho::PATH, DestinationEcho::POLICY)];

        async fn dispatch(
            context: RequestContext<'_, ()>,
            path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            if path_hash == RequestPathHash::of(DestinationEcho::PATH) {
                DestinationEcho::handle(context).await
            } else {
                Err(Decline::Ignore)
            }
        }
    }

    struct StaticPage;
    struct StaticRoutes;
    static PAGE: [u8; 1200] = [0x21; 1200];

    impl RequestRoute<()> for StaticPage {
        const PATH: &'static str = "/page";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;

        async fn handle(mut context: RequestContext<'_, ()>) -> Result<(), Decline> {
            context.respond_static_bytes(&PAGE)
        }
    }

    impl RouteSet<()> for StaticRoutes {
        const REGISTRATIONS: &'static [(&'static str, RoutePolicy)] =
            &[(StaticPage::PATH, StaticPage::POLICY)];

        async fn dispatch(
            context: RequestContext<'_, ()>,
            path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            if path_hash == RequestPathHash::of(StaticPage::PATH) {
                StaticPage::handle(context).await
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

        block_on(dispatch::<(), StaticRoutes, M, 1, 1, 16>(
            &(),
            handle,
            request,
        ));

        let Ok(issued) = channel.try_receive() else {
            panic!("response command");
        };
        let EngineCommand::Respond(response) = issued.command else {
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

        block_on(dispatch::<(), DestinationRoutes, M, 1, 1, 16>(
            &(),
            handle,
            request,
        ));

        let Ok(issued) = channel.try_receive() else {
            panic!("response command");
        };
        let EngineCommand::Respond(response) = issued.command else {
            panic!("respond command");
        };
        let crate::engine::RespondPayload::Packed(data) = response.payload else {
            panic!("packed response");
        };
        assert_eq!(data.as_slice(), destination.as_bytes());
    }
}
