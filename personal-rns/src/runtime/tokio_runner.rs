//! The tokio request runner — drives a typed [`Router`] on its own cooperative task so a slow
//! `async fn handle` never blocks the engine. Inbound requests are forked off the event stream
//! into a bounded channel (the runner copies the request bytes, so a dispatch outlives the
//! borrowed reaction that surfaced it), and the runner multiplexes every in-flight handler with a
//! `FuturesUnordered`: a handler that awaits — a database round trip, an outbound HTTP call — yields
//! and the rest keep moving. The runner grants each handler a `Vec` to fill, then ships it by move
//! through [`TokioCommands::respond_owned`] — the auto-upgrading answer (a packet under the link
//! MDU, a resource past it), in one copy with no clone of the body.
//!
//! [`Prns::serve`] is the one call an app makes: it pairs a [`Router`] with the reactor
//! [`Prns::run`] drives, so requests are answered by the router while every other event still
//! reaches the app's own callback — the data and control planes it already sees from `Prns::run`.

use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc;

use crate::engine::InstantMillis;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::storage::StorageLayout;

use super::request_router::{Decline, InboundRequest, RouteSet, Router};
use super::{Message, Prns, PrnsEvent, Recipe, StartingDestination, TokioBind, TokioCommands};

/// How many requests can wait for the runner before new ones are dropped. Drop-on-full *is* the
/// backpressure: a dropped request is one the requester retries or times out, exactly as it would
/// against any overloaded responder — far better than growing the backlog without bound under a
/// firehose.
const REQUEST_QUEUE_DEPTH: usize = 1024;

/// How many handlers run at once on the runner's task. The queue bounds the backlog; this bounds
/// the concurrency, so a burst of slow handlers can't pile up unbounded in flight either.
const MAX_IN_FLIGHT: usize = 256;

/// One inbound request, owned. The runner copies the borrowed reaction bytes so a dispatch — which
/// may await across many reactor cycles — never holds a borrow into the frame that surfaced it.
struct RunnerRequest {
    link_id: LinkId,
    request_id: RequestId,
    path_hash: RequestPathHash,
    requested_at: InstantMillis,
    data: std::vec::Vec<u8>,
}

/// Drive `router` against the request stream until the node stops, multiplexing in-flight handlers
/// so a slow one never blocks the rest, and shipping each answer the handler fills through
/// `commands`. `biased` drains ready answers before accepting new work, keeping the in-flight set
/// tight; a handler still awaiting simply polls `Pending` and the runner accepts more meanwhile.
async fn run_router<S, R>(
    router: Router<S, R>,
    mut requests: mpsc::Receiver<RunnerRequest>,
    commands: TokioCommands,
) where
    R: RouteSet<S>,
{
    let mut in_flight = FuturesUnordered::new();
    loop {
        let accepting = in_flight.len() < MAX_IN_FLIGHT;
        tokio::select! {
            biased;
            Some(()) = in_flight.next(), if !in_flight.is_empty() => {}
            request = requests.recv(), if accepting => match request {
                Some(request) => in_flight.push(dispatch(&router, &commands, request)),
                None => break,
            },
        }
    }
}

/// Route one request: grant the handler a buffer, then ship its answer, drop it, or sever the link
/// per the handler's `Result`. Borrows `router` and `commands` for the life of the future, so the
/// runner owns each once and every in-flight handler shares them — no `Arc`, no clone per request.
async fn dispatch<S, R>(router: &Router<S, R>, commands: &TokioCommands, request: RunnerRequest)
where
    R: RouteSet<S>,
{
    let inbound = InboundRequest::new(
        request.link_id,
        request.request_id,
        None,
        request.requested_at,
        &request.data,
    );
    let responder = inbound.respond_token();
    let mut body = std::vec::Vec::new();
    match router.dispatch(request.path_hash, inbound, &mut body).await {
        Ok(()) => {
            commands.respond_owned(responder, body);
        }
        Err(Decline::Ignore) => {}
        Err(Decline::CloseLink) => {
            commands.close_link(responder.link_id);
        }
    }
}

impl Prns {
    /// Stand a node up from `recipe` and answer its requests with `router`. The reactor and the
    /// runner run together: every inbound request is forked to the runner (dropped only if its
    /// queue is full), while that event — and every other — still reaches `on_event`, so an app
    /// observes exactly what [`Prns::run`] gives it and simply lets the router answer. Like
    /// `Prns::run`, this drives forever; the app issues other commands through the `commands`
    /// handle it kept from [`TokioBind::new`] (clone it before calling — the runner takes one).
    pub async fn serve<'a, St, D, AppState, R>(
        recipe: Recipe<TokioBind<St>, D>,
        router: Router<AppState, R>,
        commands: TokioCommands,
        mut on_event: impl FnMut(PrnsEvent<'_>),
    ) where
        St: StorageLayout,
        D: IntoIterator<Item = StartingDestination<'a>>,
        R: RouteSet<AppState>,
    {
        let (tx, rx) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let fork = move |event: PrnsEvent<'_>| {
            if let PrnsEvent::Message(Message::Request {
                link_id,
                request_id,
                path_hash,
                requested_at,
                data,
            }) = &event
            {
                let _ = tx.try_send(RunnerRequest {
                    link_id: *link_id,
                    request_id: *request_id,
                    path_hash: *path_hash,
                    requested_at: *requested_at,
                    data: data.to_vec(),
                });
            }
            on_event(event);
        };
        tokio::join!(Prns::run(recipe, fork), run_router(router, rx, commands));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor::impls::tokio_reactor::HostCommand;
    use crate::runtime::request_router::{RequestContext, RequestRoute, RoutePolicy};

    struct App {
        body: &'static [u8],
    }

    struct Echo;
    impl RequestRoute<App> for Echo {
        const PATH: &'static str = "/echo";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;
        async fn handle(mut cx: RequestContext<'_, App>) -> Result<(), Decline> {
            let body = cx.state.body;
            cx.respond(body)
        }
    }

    #[tokio::test]
    async fn the_runner_routes_a_request_and_issues_the_auto_upgrading_respond() {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
        let commands = TokioCommands::over(cmd_tx);
        let router = Router::new(App { body: b"pong" }, crate::routes![Echo]);
        let (req_tx, req_rx) = mpsc::channel(8);

        let driver = async {
            req_tx
                .send(RunnerRequest {
                    link_id: LinkId::new([7; 16]),
                    request_id: RequestId([9; 16]),
                    path_hash: RequestPathHash::of("/echo"),
                    requested_at: InstantMillis(0),
                    data: std::vec::Vec::new(),
                })
                .await
                .expect("runner accepts the request");

            let command = cmd_rx
                .recv()
                .await
                .expect("the runner issues a response command");
            match command {
                HostCommand::RespondAny(respond) => {
                    assert_eq!(respond.data.as_slice(), b"pong");
                    assert_eq!(respond.link_id, LinkId::new([7; 16]));
                    assert_eq!(respond.request_id, RequestId([9; 16]));
                }
                _ => panic!("the runner must issue its answer as RespondAny"),
            }

            drop(req_tx);
        };

        // join! rather than spawn: a dispatch future borrows the grant (`&mut dyn ResponseSink`),
        // so it is not `Send` — exactly as it runs joined with the reactor under `Prns::serve`.
        tokio::join!(run_router(router, req_rx, commands), driver);
    }
}
