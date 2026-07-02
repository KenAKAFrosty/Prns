use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc;

use crate::engine::InstantMillis;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::Rtt;

use super::request_router::{dispatch_request, Decline, InboundRequest, RouteSet};
use super::tokio_bind::TokioPrnsHandle;

pub(super) const REQUEST_QUEUE_DEPTH: usize = 1024;
const MAX_IN_FLIGHT: usize = 256;

pub(super) struct RunnerRequest {
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub path_hash: RequestPathHash,
    pub requested_at: InstantMillis,
    pub rtt: Rtt,
    pub data: std::vec::Vec<u8>,
}

pub(super) async fn run_router<St, R: RouteSet<St>>(
    state: &St,
    mut requests: mpsc::Receiver<RunnerRequest>,
    commands: TokioPrnsHandle,
) {
    let mut in_flight = FuturesUnordered::new();
    loop {
        let accepting = in_flight.len() < MAX_IN_FLIGHT;
        tokio::select! {
            biased;
            Some(()) = in_flight.next(), if !in_flight.is_empty() => {}
            request = requests.recv(), if accepting => match request {
                Some(request) => in_flight.push(dispatch::<St, R>(state, &commands, request)),
                None => break,
            },
        }
    }
}

async fn dispatch<St, R: RouteSet<St>>(
    state: &St,
    commands: &TokioPrnsHandle,
    request: RunnerRequest,
) {
    let inbound = InboundRequest::new(
        request.link_id,
        request.request_id,
        None,
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
