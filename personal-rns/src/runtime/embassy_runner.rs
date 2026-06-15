//! The embassy request runner — the embedded twin of the tokio runner, driving a typed [`Router`]
//! alongside the reactor so a slow `async fn handle` never blocks the engine. Where the host runner
//! leans on the heap (`FuturesUnordered`, a `Vec` grant, an unbounded `mpsc`), this stays no_std and
//! alloc-free: inbound requests fork into a `static`-free stack [`Channel`], a fixed pool of `SLOTS`
//! worker futures pull from it through [`join_array`] — the fixed pool of future-holders, the
//! embedded stand-in for the host's unbounded in-flight set — and each handler fills a fixed
//! [`RespondData`] grant the runner ships by move through [`EmbassyCommands::respond_owned`].
//!
//! Because every worker is driven by the one runner task, they all park on the request channel
//! behind a single waker — no multi-receiver wakeup to lose. The fixed grant is the one embedded
//! divergence: a host auto-upgrades an over-MDU answer to a resource, but an embedded responder is
//! packet-only, so an answer that overflows [`MAX_RESPOND_DATA_LEN`] is dropped (the requester
//! times out) rather than truncated onto the wire.
//!
//! [`Prns::serve`] is the one call an app makes — it joins the [`Router`] with the reactor
//! [`Prns::run`] drives, so requests are answered by the router while every other event still
//! reaches the app's own callback, exactly as on the host.
//!
//! [`MAX_RESPOND_DATA_LEN`]: crate::engine::commands::MAX_RESPOND_DATA_LEN

use embassy_futures::join::{join, join_array};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, Receiver};

use crate::engine::{InstantMillis, RespondData, SendRequestData};
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::storage::StorageLayout;

use super::{
    Decline, EmbassyBind, EmbassyCommands, InboundRequest, Message, Prns, PrnsEvent, Recipe,
    ResponseSink, RouteSet, Router, StartingDestination,
};

/// How many requests can wait for the pool before new ones are dropped. Drop-on-full *is* the
/// backpressure, exactly as on the host: a dropped request is one the requester retries or times
/// out. Kept small — the queue holds a [`SendRequestData`] each, so its footprint is real on an MCU.
const REQUEST_QUEUE_DEPTH: usize = 4;

/// One inbound request, owned. The runner copies the borrowed reaction bytes into a fixed buffer so a
/// dispatch — which may await across many reactor cycles — never holds a borrow into the frame that
/// surfaced it. The buffer is sized to a single-packet request; a larger one never reaches the pool.
struct RunnerRequest {
    link_id: LinkId,
    request_id: RequestId,
    path_hash: RequestPathHash,
    requested_at: InstantMillis,
    data: SendRequestData,
}

/// The grant a handler fills — the embedded peer of the host runner's `Vec`, a fixed [`RespondData`]
/// plus an overflow flag. A handler that writes past the single-packet ceiling sets `overflowed`,
/// and the runner drops the answer rather than ship a truncated packet.
struct FixedResponse {
    data: RespondData,
    overflowed: bool,
}

impl FixedResponse {
    fn new() -> Self {
        Self {
            data: RespondData::new(),
            overflowed: false,
        }
    }
}

impl ResponseSink for FixedResponse {
    fn put(&mut self, bytes: &[u8]) {
        if self.overflowed {
            return;
        }
        if self.data.extend_from_slice(bytes).is_err() {
            self.overflowed = true;
        }
    }
}

/// One worker in the fixed pool: pull a request off the shared channel and dispatch it, forever. The
/// `SLOTS` workers run under the one runner task, so they share a single waker on the channel — when
/// a request lands, the task wakes and whichever worker polls first takes it.
async fn worker<S, R, M, const COMMANDS: usize, const N: usize>(
    router: &Router<S, R>,
    requests: Receiver<'_, M, RunnerRequest, REQUEST_QUEUE_DEPTH>,
    commands: EmbassyCommands<'_, M, COMMANDS, N>,
) where
    R: RouteSet<S>,
    M: RawMutex,
{
    loop {
        let request = requests.receive().await;
        dispatch(router, commands, request).await;
    }
}

/// Route one request: grant the handler a fixed buffer, then ship its answer, drop it, or sever the
/// link per the handler's `Result`. Borrows `router` for the future's life — every worker shares the
/// one router, no clone per request.
async fn dispatch<S, R, M, const COMMANDS: usize, const N: usize>(
    router: &Router<S, R>,
    commands: EmbassyCommands<'_, M, COMMANDS, N>,
    request: RunnerRequest,
) where
    R: RouteSet<S>,
    M: RawMutex,
{
    let inbound = InboundRequest::new(
        request.link_id,
        request.request_id,
        None,
        request.requested_at,
        &request.data,
    );
    let responder = inbound.responder();
    let mut body = FixedResponse::new();
    match router.dispatch(request.path_hash, inbound, &mut body).await {
        Ok(()) => {
            if !body.overflowed {
                commands.respond_owned(responder, body.data);
            }
        }
        Err(Decline::Drop) => {}
        Err(Decline::CloseLink) => {
            commands.close_link(responder.link_id);
        }
    }
}

/// Drive `router` against the request stream until the node stops. `SLOTS` worker futures share the
/// channel through [`join_array`] — the fixed pool of future-holders — so a slow handler occupies its
/// own slot and the rest keep pulling work.
async fn run_router<const SLOTS: usize, S, R, M, const COMMANDS: usize, const N: usize>(
    router: &Router<S, R>,
    requests: Receiver<'_, M, RunnerRequest, REQUEST_QUEUE_DEPTH>,
    commands: EmbassyCommands<'_, M, COMMANDS, N>,
) where
    R: RouteSet<S>,
    M: RawMutex,
{
    let workers: [_; SLOTS] = core::array::from_fn(|_| worker(router, requests, commands));
    join_array(workers).await;
}

impl Prns {
    /// Stand a node up from `recipe` and answer its requests with `router`, on the embedded reactor.
    /// The embedded peer of the host [`serve`](Self::serve) — the reactor and a fixed pool of `SLOTS`
    /// handler slots run joined: every inbound request is forked to the pool (dropped only if its
    /// queue is full), while that event — and every other — still reaches `on_event`. The app picks
    /// the pool size at the call site (`Prns::serve::<4>(…)`); the request channel lives on this
    /// future's stack, so unlike the command channel the app provides no `static` for it.
    pub async fn serve<
        'a,
        const SLOTS: usize,
        S,
        E,
        M,
        const NOTIFY: usize,
        const COMMANDS: usize,
        const N: usize,
        D,
        AppState,
        R,
    >(
        recipe: Recipe<EmbassyBind<'a, S, E, M, NOTIFY, COMMANDS, N>, D>,
        router: Router<AppState, R>,
        commands: EmbassyCommands<'a, M, COMMANDS, N>,
        on_event: impl FnMut(PrnsEvent<'_>),
    ) where
        S: StorageLayout,
        E: FnMut(&mut [u8]),
        M: RawMutex,
        D: IntoIterator<Item = StartingDestination<'a>>,
        R: RouteSet<AppState>,
    {
        let channel = Channel::<M, RunnerRequest, REQUEST_QUEUE_DEPTH>::new();
        let sender = channel.sender();
        let mut on_event = on_event;
        let fork = move |event: PrnsEvent<'_>| {
            if let PrnsEvent::Message(Message::Request {
                link_id,
                request_id,
                path_hash,
                requested_at,
                data,
            }) = &event
            {
                if let Ok(data) = SendRequestData::from_slice(data) {
                    let _ = sender.try_send(RunnerRequest {
                        link_id: *link_id,
                        request_id: *request_id,
                        path_hash: *path_hash,
                        requested_at: *requested_at,
                        data,
                    });
                }
            }
            on_event(event);
        };
        join(
            Prns::run(recipe, fork),
            run_router::<SLOTS, _, _, _, _, _>(&router, channel.receiver(), commands),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineCommand, IssuedCommand};
    use crate::runtime::{CompletionPool, RequestCx, RequestRoute, RoutePolicy};
    use embassy_futures::block_on;
    use embassy_futures::select::{select, Either};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;

    struct App {
        body: &'static [u8],
    }

    struct Echo;
    impl RequestRoute<App> for Echo {
        const PATH: &'static str = "/echo";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;
        async fn handle(mut cx: RequestCx<'_, App>) -> Result<(), Decline> {
            let body = cx.state.body;
            cx.respond(body)
        }
    }

    #[test]
    fn the_pool_routes_a_request_and_issues_the_respond() {
        let command_channel: Channel<CriticalSectionRawMutex, IssuedCommand, 4> = Channel::new();
        let pool: CompletionPool<CriticalSectionRawMutex, 1> = CompletionPool::new();
        let commands = EmbassyCommands::new(command_channel.sender(), &pool);
        let router = Router::new(App { body: b"pong" }, crate::routes![Echo]);
        let requests: Channel<CriticalSectionRawMutex, RunnerRequest, REQUEST_QUEUE_DEPTH> =
            Channel::new();

        requests
            .try_send(RunnerRequest {
                link_id: LinkId::new([7; 16]),
                request_id: RequestId([9; 16]),
                path_hash: RequestPathHash::of("/echo"),
                requested_at: InstantMillis(0),
                data: SendRequestData::new(),
            })
            .ok()
            .expect("the request channel accepts the request");

        let driver = async {
            let issued = command_channel.receive().await;
            match issued.command {
                EngineCommand::Respond(respond) => {
                    assert_eq!(respond.data.as_slice(), b"pong");
                    assert_eq!(respond.link_id, LinkId::new([7; 16]));
                    assert_eq!(respond.request_id, RequestId([9; 16]));
                }
                _ => panic!("the runner must answer with Respond"),
            }
        };

        match block_on(select(
            run_router::<2, _, _, _, _, _>(&router, requests.receiver(), commands),
            driver,
        )) {
            Either::Second(()) => {}
            Either::First(()) => panic!("the runner loop ended before the answer was observed"),
        }
    }
}
