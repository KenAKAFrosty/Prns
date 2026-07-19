use std::panic::AssertUnwindSafe;

use futures_util::stream::{FuturesUnordered, StreamExt};
use futures_util::FutureExt;
use tokio::sync::mpsc;

use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;

use super::node_facade::PrnsNodeHandle;
use super::request_router::{dispatch_request, Decline, InboundRequest, RouteSet};

pub(super) const REQUEST_QUEUE_DEPTH: usize = 1024;
const MAX_IN_FLIGHT: usize = 256;

pub(super) struct RunnerRequest {
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub requester: Option<IdentityHash>,
    pub path_hash: RequestPathHash,
    pub requested_at: InstantMillis,
    pub rtt: RttMillis,
    pub data: std::vec::Vec<u8>,
}

pub(super) async fn run_router<St, R: RouteSet<St>>(
    state: &St,
    mut requests: mpsc::Receiver<RunnerRequest>,
    commands: PrnsNodeHandle,
) {
    let mut in_flight = FuturesUnordered::new();
    loop {
        let accepting = in_flight.len() < MAX_IN_FLIGHT;
        tokio::select! {
            biased;
            Some(()) = in_flight.next(), if !in_flight.is_empty() => {}
            request = requests.recv(), if accepting => match request {
                Some(request) => in_flight.push(dispatch_guarded::<St, R>(state, &commands, request)),
                None => break,
            },
        }
    }
}

async fn dispatch_guarded<St, R: RouteSet<St>>(
    state: &St,
    commands: &PrnsNodeHandle,
    request: RunnerRequest,
) {
    let link_id = request.link_id;
    if AssertUnwindSafe(dispatch::<St, R>(state, commands, request))
        .catch_unwind()
        .await
        .is_err()
    {
        commands.close_link(link_id);
    }
}

async fn dispatch<St, R: RouteSet<St>>(
    state: &St,
    commands: &PrnsNodeHandle,
    request: RunnerRequest,
) {
    let inbound = InboundRequest::new(
        request.link_id,
        request.request_id,
        request.requester,
        request.requested_at,
        request.rtt,
        &request.data,
    );
    let responder = inbound.respond_token();
    let mut body = std::vec::Vec::new();
    match dispatch_request::<St, R>(state, request.path_hash, inbound, &mut body).await {
        Ok(()) => {
            commands.respond_owned(responder, body);
        }
        Err(Decline::Ignore) => {}
        Err(Decline::CloseLink) => {
            commands.close_link(responder.link_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineCommand, IssuedCommand};
    use crate::reactor::driver::HostCommand;
    use crate::routing::request_handlers::RequestPathHash;
    use crate::runtime::request_router::{RequestContext, RoutePolicy};

    struct PanickingRouteSet;

    impl RouteSet<()> for PanickingRouteSet {
        const REGISTRATIONS: &'static [(&'static str, RoutePolicy)] = &[];

        async fn dispatch(
            _context: RequestContext<'_, ()>,
            _path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            std::panic::panic_any("request handler")
        }
    }

    #[tokio::test]
    async fn a_panicking_request_handler_closes_its_link() {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let handle = PrnsNodeHandle::over(commands);
        let link_id = LinkId::new([0x44; 16]);
        dispatch_guarded::<(), PanickingRouteSet>(
            &(),
            &handle,
            RunnerRequest {
                link_id,
                request_id: RequestId([0x55; 16]),
                path_hash: RequestPathHash::new([0x66; 16]),
                requested_at: InstantMillis(700),
                rtt: RttMillis::new(80),
                data: std::vec::Vec::new(),
            },
        )
        .await;

        assert!(matches!(
            command_rx.recv().await,
            Some(HostCommand::Engine(IssuedCommand {
                command: EngineCommand::CloseLink(close),
                ..
            })) if close.link_id == link_id
        ));
    }
}
