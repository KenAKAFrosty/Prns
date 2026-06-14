//! The typed request router — the consumer-API layer over the engine's parity-faithful request
//! handler registry. App developers declare a compile-time set of routes; the registry the engine
//! gates against is *derived* from that set (so the two can't drift), and a request is dispatched
//! to the matching route's `async fn handle` without the app ever touching `link_id`/`request_id`
//! or the packet-vs-resource decision.
//!
//! A route is its `PATH` (the contract string — it never crosses the wire; both ends meet at
//! `RequestPathHash::of(PATH)`), its `POLICY` (who may ask), and its `handle`. A handler takes `&S`
//! (shared app state — concurrency is cooperative, so mutation rides interior mutability, never a
//! `Mutex`), builds its answer however it likes — a stack buffer, a computed value, a slice of
//! state — and hands it over with `cx.respond(bytes)`, which copies it into the runner's grant and
//! returns `Ok(())`. The runtime ships it through the engine's auto-upgrading respond (a packet
//! under the link MDU, a resource past it).
//!
//! The handler returns `Result<(), Decline>`. `Ok(())` *always* answers — even an empty body is a
//! valid answer — so the return is the seal: the type system won't let a handler forget to answer
//! or refuse. `Err(Decline)` is the only non-data outcome, a deliberate refusal. App-level failures
//! do not live here; they ride the response *data* (HTTP-style), and `Decline`'s lack of any `From`
//! impl keeps a stray `?` from coercing an ordinary error into a refusal.
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

/// A handler's only non-data outcome: a deliberate refusal to answer. App-level failures do *not*
/// belong here — they ride the response data (`cx.respond(error_bytes)`), HTTP-style. `Decline` is
/// reachable only by naming it; with no `From` impls, an ordinary `?` can never coerce a database
/// or parse error into a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decline {
    /// Send no response and leave the link open — the requester's request times out. RNS 1.3.1's
    /// `None` return, exactly: that one request fails, the link survives for other traffic.
    Drop,
    /// Send no response and sever the link.
    CloseLink,
}

/// The buffer the runner grants a handler to fill — grant-then-fill, so the handler never allocates
/// and its bytes reach the engine in one move. `put` appends. The tokio runner grants a `Vec<u8>`;
/// an embassy runner grants a fixed buffer. Handlers reach it only through [`RequestCx::respond`] /
/// [`RequestCx::write`], never directly.
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
/// grant it hands its answer to. The single lifetime is why the handler signature elides it —
/// `async fn handle(cx: RequestCx<'_, S>) -> Result<(), Decline>`. Mutation of `state` rides
/// interior mutability (the dispatch task is cooperative, so a `RefCell`/atomic suffices — never a
/// `Mutex`); the answer rides [`respond`](Self::respond).
pub struct RequestCx<'a, S> {
    pub state: &'a S,
    pub data: &'a [u8],
    pub requester: Option<IdentityHash>,
    pub requested_at: InstantMillis,
    responder: Responder,
    sink: &'a mut dyn ResponseSink,
}

impl<S> RequestCx<'_, S> {
    /// Answer with `bytes` and finish — copies them into the grant and returns `Ok(())`, so the
    /// whole happy path is the one expression `cx.respond(data)`. An empty `bytes` is a valid empty
    /// answer; to refuse instead, return `Err(Decline::…)`.
    pub fn respond(&mut self, bytes: &[u8]) -> Result<(), Decline> {
        self.sink.put(bytes);
        Ok(())
    }

    /// Append `bytes` without finishing — for assembling a multi-part body (a header then a
    /// payload) straight into the grant when you'd otherwise build it in your own buffer first.
    /// Finish with a bare `Ok(())`. Most handlers never need this; reach for
    /// [`respond`](Self::respond).
    pub fn write(&mut self, bytes: &[u8]) -> &mut Self {
        self.sink.put(bytes);
        self
    }

    /// The token to answer this request later — keep it, return `Err(Decline::Drop)` now, and
    /// answer from another task through the platform command handle.
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
    async fn handle(cx: RequestCx<'_, S>) -> Result<(), Decline>;
}

/// A compile-time set of routes, produced by [`routes!`]. The registrations the recipe stands up
/// are *this* set's, so they can't drift from what dispatch matches.
///
/// [`routes!`]: crate::routes
#[allow(async_fn_in_trait)]
pub trait RouteSet<S> {
    /// `(path, policy)` per route — the recipe registers each, seeding any `AllowList`.
    const REGISTRATIONS: &'static [(&'static str, RoutePolicy)];
    /// Run the route whose path hashes to `path_hash` and return its outcome. A `path_hash` the
    /// set doesn't carry yields `Err(Decline::Drop)` — a near-dead branch, since
    /// [`Self::REGISTRATIONS`] *is* the gate the engine admits against.
    async fn dispatch(cx: RequestCx<'_, S>, path_hash: RequestPathHash) -> Result<(), Decline>;
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
    /// dispatch concurrently against shared `&S`. Returns the handler's outcome: `Ok(())` to ship
    /// the filled `sink`, `Err(Decline)` to refuse (drop the request, or sever the link).
    pub async fn dispatch<'a>(
        &'a self,
        path_hash: RequestPathHash,
        request: InboundRequest<'a>,
        sink: &'a mut dyn ResponseSink,
    ) -> Result<(), Decline> {
        let cx = RequestCx {
            state: &self.state,
            data: request.data,
            requester: request.requester,
            requested_at: request.requested_at,
            responder: request.responder(),
            sink,
        };
        R::dispatch(cx, path_hash).await
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
            ) -> ::core::result::Result<(), $crate::runtime::Decline> {
                $(
                    if path_hash
                        == $crate::routing::request_handlers::RequestPathHash::of(
                            <$route as $crate::runtime::RequestRoute<S>>::PATH,
                        )
                    {
                        return <$route as $crate::runtime::RequestRoute<S>>::handle(cx).await;
                    }
                )+
                ::core::result::Result::Err($crate::runtime::Decline::Drop)
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
        async fn handle(mut cx: RequestCx<'_, App>) -> Result<(), Decline> {
            cx.respond(b"ok")
        }
    }

    struct Greet;
    impl RequestRoute<App> for Greet {
        const PATH: &'static str = "/greet";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;
        async fn handle(mut cx: RequestCx<'_, App>) -> Result<(), Decline> {
            let greeting = cx.state.greeting;
            cx.respond(greeting)
        }
    }

    const ADMIN: IdentityHash = IdentityHash::new([0xAD; 16]);

    struct Admin;
    impl RequestRoute<App> for Admin {
        const PATH: &'static str = "/admin";
        const POLICY: RoutePolicy = RoutePolicy::AllowList(&[ADMIN]);
        async fn handle(_cx: RequestCx<'_, App>) -> Result<(), Decline> {
            Err(Decline::CloseLink)
        }
    }

    struct Ack;
    impl RequestRoute<App> for Ack {
        const PATH: &'static str = "/ack";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;
        async fn handle(mut cx: RequestCx<'_, App>) -> Result<(), Decline> {
            cx.respond(&[])
        }
    }

    fn bench_router() -> Router<App, impl RouteSet<App>> {
        Router::new(
            App { greeting: b"hi" },
            crate::routes![Health, Greet, Admin, Ack],
        )
    }

    #[test]
    fn the_route_set_is_the_registration_set_the_recipe_stands_up() {
        let router = bench_router();
        let registrations = router.registrations();
        assert_eq!(registrations.len(), 4);
        assert_eq!(registrations[0], ("/health", RoutePolicy::AllowAll));
        assert_eq!(registrations[2].0, "/admin");
        assert_eq!(registrations[2].1.engine_policy(), RequestPolicy::AllowList);
        assert_eq!(registrations[2].1.seed_list(), &[ADMIN]);
        assert_eq!(registrations[0].1.engine_policy(), RequestPolicy::AllowAll);
        assert!(registrations[0].1.seed_list().is_empty());
    }

    #[cfg(feature = "tokio-host")]
    #[tokio::test]
    async fn dispatch_routes_by_path_then_answers_or_declines() {
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
        assert_eq!(
            router
                .dispatch(RequestPathHash::of("/greet"), request(), &mut greet)
                .await,
            Ok(())
        );
        assert_eq!(greet.as_slice(), b"hi");

        let mut health = std::vec::Vec::new();
        assert_eq!(
            router
                .dispatch(RequestPathHash::of("/health"), request(), &mut health)
                .await,
            Ok(())
        );
        assert_eq!(health.as_slice(), b"ok");

        let mut ack = std::vec::Vec::new();
        assert_eq!(
            router
                .dispatch(RequestPathHash::of("/ack"), request(), &mut ack)
                .await,
            Ok(())
        );
        assert!(ack.is_empty());

        let mut admin = std::vec::Vec::new();
        assert_eq!(
            router
                .dispatch(RequestPathHash::of("/admin"), request(), &mut admin)
                .await,
            Err(Decline::CloseLink)
        );

        let mut miss = std::vec::Vec::new();
        assert_eq!(
            router
                .dispatch(RequestPathHash::of("/nope"), request(), &mut miss)
                .await,
            Err(Decline::Drop)
        );
    }
}
