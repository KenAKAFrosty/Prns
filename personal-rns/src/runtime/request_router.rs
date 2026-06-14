//! The typed request router — the consumer-API layer over the engine's parity-faithful request
//! handler registry. App developers declare a compile-time set of routes; the registry the engine
//! gates against is *derived* from that set (so the two can't drift), and a request is dispatched
//! to the matching route's `async fn handle` without the app ever touching `link_id`/`request_id`
//! or the packet-vs-resource decision.
//!
//! Three things define a route: its `PATH` (the contract string — it never crosses the wire; both
//! ends meet at `RequestPathHash::of(PATH)`), its `POLICY`, and its `handle`. Handlers take `&S`
//! (shared app state — concurrency is cooperative, so mutation rides interior mutability, never a
//! `Mutex`) and *fill* their answer into the grant the runner hands them — `cx.reply(bytes)` for a
//! one-shot body, `cx.write(..)` chained for a built-up one — then return [`Response::Reply`]. The
//! runtime ships what was written through the engine's auto-upgrading respond (a packet under the
//! link MDU, a resource past it). Grant-then-fill, like the egress lanes: the runner owns the
//! buffer, the handler fills it, and the answer reaches the wire in one copy with no per-request
//! alloc in the handler — the single shape both the tokio heap and an embedded bounded buffer honor.
//!
//! Composition is a `routes!` set, not a `dyn` table: each arm awaits a concrete handler future,
//! so nothing is boxed and the whole thing stays `no_std`.

use core::marker::PhantomData;

use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};

/// A route's access policy — the consumer-API shape, richer than the engine's fieldless
/// [`RequestPolicy`]: an `AllowList` carries the identities it admits *at compile time*. The
/// recipe seeds the engine's gate from this at registration; [`AllowRequester`] adds more at
/// runtime.
///
/// [`AllowRequester`]: crate::engine::AllowRequester
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutePolicy {
    /// Registered but answers no one (RNS `ALLOW_NONE`, the reference default).
    AllowNone,
    /// Answers anyone the link delivers — identified or anonymous (RNS `ALLOW_ALL`).
    AllowAll,
    /// Answers only the listed identities (RNS `ALLOW_LIST`); the slice seeds the gate at startup.
    AllowList(&'static [IdentityHash]),
}

impl RoutePolicy {
    /// The engine gate this maps to. The seed identities ride [`Self::seed_list`] separately —
    /// the engine registers the policy, then admits each seed.
    #[must_use]
    pub fn engine_policy(self) -> RequestPolicy {
        match self {
            RoutePolicy::AllowNone => RequestPolicy::AllowNone,
            RoutePolicy::AllowAll => RequestPolicy::AllowAll,
            RoutePolicy::AllowList(_) => RequestPolicy::AllowList,
        }
    }

    /// The identities to admit at registration — non-empty only for [`RoutePolicy::AllowList`].
    #[must_use]
    pub fn seed_list(self) -> &'static [IdentityHash] {
        match self {
            RoutePolicy::AllowList(list) => list,
            _ => &[],
        }
    }
}

/// Whether a handler answered. The bytes themselves go into the grant via [`RequestCx::reply`] /
/// [`RequestCx::write`]; this only says send-or-not, so it carries no lifetime. `Reply` ships
/// whatever was written (writing nothing is a valid empty answer); `None` is a deliberate
/// non-answer — fire-and-forget, or "I kept the [`Responder`] and will answer later".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    None,
    Reply,
}

/// The buffer the runner grants a handler to fill — grant-then-fill, so the handler never allocates
/// and its bytes reach the engine in one move. `put` appends; a bounded sink (an embedded buffer)
/// appends what fits and may drop the rest. The tokio runner grants a `Vec<u8>`; an embassy runner
/// grants a fixed buffer. Handlers reach it only through [`RequestCx::reply`] / [`RequestCx::write`],
/// never directly.
pub trait ResponseSink {
    fn put(&mut self, bytes: &[u8]);
}

#[cfg(feature = "alloc")]
impl ResponseSink for alloc::vec::Vec<u8> {
    fn put(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

/// A `Copy` token naming the request to answer, lifted out of the handler's view so the common
/// case never threads `link_id`/`request_id`. Keep it to answer later (offload / defer): the
/// platform command surface turns it back into the engine's respond.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Responder {
    pub link_id: LinkId,
    pub request_id: RequestId,
}

/// One inbound request as the *runtime* assembles it from a journaled `RequestReceived` — the raw
/// data, who asked (`None` = an anonymous initiator, the RNS default), and when. The runner builds
/// this and hands it to [`Router::dispatch`], which pairs it with the app state into a
/// [`RequestCx`] for the handler. `link_id`/`request_id` ride [`responder`](Self::responder).
pub struct InboundRequest<'a> {
    pub data: &'a [u8],
    pub requester: Option<IdentityHash>,
    pub requested_at: InstantMillis,
    responder: Responder,
}

impl<'a> InboundRequest<'a> {
    /// Build the runtime's parts of a journaled request. The runner calls this; a test can too.
    #[must_use]
    pub fn new(
        link_id: LinkId,
        request_id: RequestId,
        requester: Option<IdentityHash>,
        requested_at: InstantMillis,
        data: &'a [u8],
    ) -> Self {
        Self {
            data,
            requester,
            requested_at,
            responder: Responder {
                link_id,
                request_id,
            },
        }
    }

    /// The token to answer this request — needed only to defer the answer past `handle`.
    #[must_use]
    pub fn responder(&self) -> Responder {
        self.responder
    }
}

/// Everything a handler needs, in one borrow: the shared app `state`, the inbound request, and the
/// grant it fills its answer into. The single lifetime is why the handler signature elides it —
/// `async fn handle(cx: RequestCx<'_, S>) -> Response`. Mutation of `state` rides interior
/// mutability (the dispatch task is cooperative, so a `RefCell`/atomic suffices — never a `Mutex`);
/// the answer rides [`reply`](Self::reply) / [`write`](Self::write), never an owned return value.
pub struct RequestCx<'a, S> {
    pub state: &'a S,
    pub data: &'a [u8],
    pub requester: Option<IdentityHash>,
    pub requested_at: InstantMillis,
    responder: Responder,
    sink: &'a mut dyn ResponseSink,
}

impl<S> RequestCx<'_, S> {
    /// Answer with `bytes` and send it — the one-shot case, equivalent to a [`write`](Self::write)
    /// then returning [`Response::Reply`]: `cx.reply(body)`.
    pub fn reply(&mut self, bytes: &[u8]) -> Response {
        self.sink.put(bytes);
        Response::Reply
    }

    /// Append `bytes` to the answer, returning `self` so a multi-part body chains — a header then a
    /// payload reads `cx.write(&header).reply(&body)`. Nothing is sent until the handler returns
    /// [`Response::Reply`] (or a terminal [`reply`](Self::reply)).
    pub fn write(&mut self, bytes: &[u8]) -> &mut Self {
        self.sink.put(bytes);
        self
    }

    /// The token to answer this request later — when `handle` returns [`Response::None`] now and
    /// the answer comes from an offloaded task.
    #[must_use]
    pub fn responder(&self) -> Responder {
        self.responder
    }
}

/// One route: a contract path, an access policy, and an async handler over shared `&S`.
#[allow(async_fn_in_trait)]
pub trait RequestRoute<S> {
    const PATH: &'static str;
    const POLICY: RoutePolicy;
    async fn handle(cx: RequestCx<'_, S>) -> Response;
}

/// A compile-time set of routes, produced by [`routes!`]. The registrations the recipe stands up
/// are *this* set's, so they can't drift from what dispatch matches.
///
/// [`routes!`]: crate::routes
#[allow(async_fn_in_trait)]
pub trait RouteSet<S> {
    /// `(path, policy)` per route — the recipe registers each, seeding any `AllowList`.
    const REGISTRATIONS: &'static [(&'static str, RoutePolicy)];
    /// Run the route whose path hashes to `path_hash`, or `None` if the set doesn't match it
    /// (a gate-admitted path the set somehow misses — with [`Self::REGISTRATIONS`] deriving the
    /// gate, a near-dead branch).
    async fn dispatch(cx: RequestCx<'_, S>, path_hash: RequestPathHash) -> Option<Response>;
}

/// Owns the app state and a [`RouteSet`]; the app's request-handling surface. Built once with
/// [`routes!`], it hands its [`registrations`] to the recipe and answers requests through
/// [`dispatch`] — driven by the platform runner on its own task, so a slow handler never blocks
/// the engine.
///
/// [`routes!`]: crate::routes
/// [`registrations`]: Self::registrations
/// [`dispatch`]: Self::dispatch
pub struct Router<S, R: RouteSet<S>> {
    state: S,
    _routes: PhantomData<R>,
}

impl<S, R: RouteSet<S>> Router<S, R> {
    /// Build a router over `state` and a `routes!` set.
    #[must_use]
    pub fn new(state: S, _routes: R) -> Self {
        Self {
            state,
            _routes: PhantomData,
        }
    }

    /// The `(path, policy)` set to register — feed this to the recipe so the gate is the set.
    #[must_use]
    pub fn registrations(&self) -> &'static [(&'static str, RoutePolicy)] {
        R::REGISTRATIONS
    }

    /// Shared access to the app state — for an offloaded answer that reads `S` after `handle`.
    #[must_use]
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Route `request` to its handler, which fills its answer into `sink`. `&self`, so many requests
    /// dispatch concurrently against shared `&S`. Returns the [`Responder`] to ship the filled
    /// `sink` to, or `None` when the handler declined (or the set didn't match the path).
    pub async fn dispatch<'a>(
        &'a self,
        path_hash: RequestPathHash,
        request: InboundRequest<'a>,
        sink: &'a mut dyn ResponseSink,
    ) -> Option<Responder> {
        let responder = request.responder();
        let cx = RequestCx {
            state: &self.state,
            data: request.data,
            requester: request.requester,
            requested_at: request.requested_at,
            responder,
            sink,
        };
        match R::dispatch(cx, path_hash).await {
            Some(Response::Reply) => Some(responder),
            Some(Response::None) | None => None,
        }
    }
}

/// Compose route types into a [`RouteSet`] value: `routes![Health, Echo, Status]`. Each arm awaits
/// a concrete handler future, so the set is monomorphized — no boxing, `no_std`-clean — and its
/// registrations are derived from the same types, so the gate can't drift from dispatch.
#[macro_export]
macro_rules! routes {
    ($($route:ty),+ $(,)?) => {{
        struct RouteSetImpl;
        impl<S> $crate::runtime::RouteSet<S> for RouteSetImpl
        where
            $($route: $crate::runtime::RequestRoute<S>,)+
        {
            const REGISTRATIONS: &'static [(&'static str, $crate::runtime::RoutePolicy)] = &[
                $((
                    <$route as $crate::runtime::RequestRoute<S>>::PATH,
                    <$route as $crate::runtime::RequestRoute<S>>::POLICY,
                ),)+
            ];

            async fn dispatch(
                cx: $crate::runtime::RequestCx<'_, S>,
                path_hash: $crate::routing::request_handlers::RequestPathHash,
            ) -> ::core::option::Option<$crate::runtime::Response> {
                $(
                    if path_hash
                        == $crate::routing::request_handlers::RequestPathHash::of(
                            <$route as $crate::runtime::RequestRoute<S>>::PATH,
                        )
                    {
                        return ::core::option::Option::Some(
                            <$route as $crate::runtime::RequestRoute<S>>::handle(cx).await,
                        );
                    }
                )+
                ::core::option::Option::None
            }
        }
        RouteSetImpl
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    struct App {
        greeting: &'static [u8],
    }

    struct Health;
    impl RequestRoute<App> for Health {
        const PATH: &'static str = "/health";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;
        async fn handle(mut cx: RequestCx<'_, App>) -> Response {
            cx.reply(b"ok")
        }
    }

    struct Greet;
    impl RequestRoute<App> for Greet {
        const PATH: &'static str = "/greet";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;
        async fn handle(mut cx: RequestCx<'_, App>) -> Response {
            let greeting = cx.state.greeting;
            cx.reply(greeting)
        }
    }

    const ADMIN: IdentityHash = IdentityHash::new([0xAD; 16]);

    struct Admin;
    impl RequestRoute<App> for Admin {
        const PATH: &'static str = "/admin";
        const POLICY: RoutePolicy = RoutePolicy::AllowList(&[ADMIN]);
        async fn handle(_cx: RequestCx<'_, App>) -> Response {
            Response::None
        }
    }

    fn bench_router() -> Router<App, impl RouteSet<App>> {
        Router::new(
            App { greeting: b"hi" },
            crate::routes![Health, Greet, Admin],
        )
    }

    #[test]
    fn the_route_set_is_the_registration_set_the_recipe_stands_up() {
        let router = bench_router();
        let registrations = router.registrations();
        assert_eq!(registrations.len(), 3);
        assert_eq!(registrations[0], ("/health", RoutePolicy::AllowAll));
        assert_eq!(registrations[2].0, "/admin");
        // The compile-time AllowList becomes an engine AllowList plus its seed identities.
        assert_eq!(registrations[2].1.engine_policy(), RequestPolicy::AllowList);
        assert_eq!(registrations[2].1.seed_list(), &[ADMIN]);
        assert_eq!(registrations[0].1.engine_policy(), RequestPolicy::AllowAll);
        assert!(registrations[0].1.seed_list().is_empty());
    }

    #[cfg(feature = "tokio-host")]
    #[tokio::test]
    async fn dispatch_routes_by_path_and_fills_the_grant() {
        let router = bench_router();
        let request = || {
            InboundRequest::new(
                LinkId::new([1; 16]),
                RequestId([2; 16]),
                None,
                InstantMillis(0),
                b"",
            )
        };

        let mut greet = std::vec::Vec::new();
        let answered = router
            .dispatch(RequestPathHash::of("/greet"), request(), &mut greet)
            .await;
        assert!(answered.is_some());
        assert_eq!(greet.as_slice(), b"hi");

        let mut health = std::vec::Vec::new();
        let answered = router
            .dispatch(RequestPathHash::of("/health"), request(), &mut health)
            .await;
        assert!(answered.is_some());
        assert_eq!(health.as_slice(), b"ok");

        // A route that answers `None` fills nothing and ships nothing.
        let mut admin = std::vec::Vec::new();
        let answered = router
            .dispatch(RequestPathHash::of("/admin"), request(), &mut admin)
            .await;
        assert!(answered.is_none());
        assert!(admin.is_empty());

        // A path the set does not carry ships nothing (the gate keeps unknowns silent).
        let mut miss = std::vec::Vec::new();
        let answered = router
            .dispatch(RequestPathHash::of("/nope"), request(), &mut miss)
            .await;
        assert!(answered.is_none());
    }
}
