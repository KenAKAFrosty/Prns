use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};
use crate::units::Rtt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutePolicy {
    AllowNone,
    AllowAll,
    AllowList(&'static [IdentityHash]),
}

impl RoutePolicy {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decline {
    /// Send no confirmation at all. This will contribute to a timeout on the Link if not handled yourself.
    ///
    /// See [`respond_token`](RequestContext::respond_token)
    Ignore,
    CloseLink,
}

pub trait ResponseSink {
    fn put(&mut self, bytes: &[u8]);
}

#[cfg(feature = "alloc")]
impl ResponseSink for alloc::vec::Vec<u8> {
    fn put(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

/// Only needed if you don't respond to the request inside your [`handle`](RequestRoute::handle) function.
/// See [`respond_token`](RequestContext::respond_token).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RespondToken {
    pub link_id: LinkId,
    pub request_id: RequestId,
    /// The link's measured round trip when the request arrived.
    pub rtt: Rtt,
}

pub struct InboundRequest<'a> {
    pub data: &'a [u8],
    pub requester: Option<IdentityHash>,
    pub requested_at: InstantMillis,
    respond_token: RespondToken,
}

impl<'a> InboundRequest<'a> {
    #[must_use]
    pub fn new(
        link_id: LinkId,
        request_id: RequestId,
        requester: Option<IdentityHash>,
        requested_at: InstantMillis,
        rtt: Rtt,
        data: &'a [u8],
    ) -> Self {
        Self {
            data,
            requester,
            requested_at,
            respond_token: RespondToken {
                link_id,
                request_id,
                rtt,
            },
        }
    }

    #[must_use]
    pub fn respond_token(&self) -> RespondToken {
        self.respond_token
    }
}

pub struct RequestContext<'a, S> {
    pub state: &'a S,
    pub data: &'a [u8],
    pub requester: Option<IdentityHash>,
    pub requested_at: InstantMillis,
    respond_token: RespondToken,
    sink: &'a mut dyn ResponseSink,
}

impl<S> RequestContext<'_, S> {
    pub fn respond(&mut self, bytes: &[u8]) -> Result<(), Decline> {
        self.sink.put(bytes);
        Ok(())
    }

    /// Append `bytes` without finishing. Used to assemble a multi-part body (a header then a
    /// payload) straight into the grant when you'd otherwise build it in your own buffer first.
    /// Finish with a bare `Ok(())`.
    ///
    /// This is a more advanced use for constrained targets or perf.
    /// If you're unsure, you should probably reach for [`respond`](Self::respond).
    pub fn write(&mut self, bytes: &[u8]) -> &mut Self {
        self.sink.put(bytes);
        self
    }

    /// The token to answer this request later. You can keep it, return `Err(Decline::Ignore)` now, and
    /// answer from another task through the platform command handle.
    #[must_use]
    pub fn respond_token(&self) -> RespondToken {
        self.respond_token
    }
}

#[allow(async_fn_in_trait)]
pub trait RequestRoute<AppState> {
    const PATH: &'static str;
    const POLICY: RoutePolicy;
    async fn handle(context: RequestContext<'_, AppState>) -> Result<(), Decline>;
}

/// A compile-time set of routes, produced by [`routes!`](crate::routes).
///
/// You probably should be using that macro instead of this trait directly.
#[allow(async_fn_in_trait)]
pub trait RouteSet<S> {
    const REGISTRATIONS: &'static [(&'static str, RoutePolicy)];
    async fn dispatch(cx: RequestContext<'_, S>, path_hash: RequestPathHash)
        -> Result<(), Decline>;
}

/// The empty route set — what [`routes!`](crate::routes) with no arms hands back, and what a node
/// that serves no requests carries. It registers nothing and declines every request as `Ignore`.
impl<S> RouteSet<S> for () {
    const REGISTRATIONS: &'static [(&'static str, RoutePolicy)] = &[];
    async fn dispatch(
        _cx: RequestContext<'_, S>,
        _path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        Err(Decline::Ignore)
    }
}

/// The value [`routes!`](crate::routes) hands back when given no routes — the empty [`RouteSet`].
/// A named constructor so the macro needn't expand to a bare `()`, which `clippy::unused_unit`
/// flags at every call site.
pub const fn no_routes() {}

/// Route one request to the handler its `path_hash` selects, building the [`RequestContext`] over
/// the app's shared `state` and the runner's grant `sink`. `RouteSet::dispatch` is a static fn, so
/// the runner dispatches with only `&state` and the route-set type `R` — no `Router` wrapper.
pub async fn dispatch_request<'a, S, R: RouteSet<S>>(
    state: &'a S,
    path_hash: RequestPathHash,
    request: InboundRequest<'a>,
    sink: &'a mut dyn ResponseSink,
) -> Result<(), Decline> {
    let cx = RequestContext {
        state,
        data: request.data,
        requester: request.requester,
        requested_at: request.requested_at,
        respond_token: request.respond_token(),
        sink,
    };
    R::dispatch(cx, path_hash).await
}

/// Compose route types into a [`RouteSet`] value, e.g., `routes![Health, Echo, Status]`. Each arm awaits
/// a concrete handler future, so the set is monomorphized. There's no boxing and it's`no_std`-clean.
#[macro_export]
macro_rules! routes {
    () => {
        $crate::runtime::request_router::no_routes()
    };
    ($($route:ty),+ $(,)?) => {{
        struct RouteSetImpl;
        impl<S> $crate::runtime::request_router::RouteSet<S> for RouteSetImpl
        where
            $($route: $crate::runtime::request_router::RequestRoute<S>,)+
        {
            const REGISTRATIONS: &'static [(&'static str, $crate::runtime::request_router::RoutePolicy)] = &[
                $((
                    <$route as $crate::runtime::request_router::RequestRoute<S>>::PATH,
                    <$route as $crate::runtime::request_router::RequestRoute<S>>::POLICY,
                ),)+
            ];

            async fn dispatch(
                cx: $crate::runtime::request_router::RequestContext<'_, S>,
                path_hash: $crate::routing::request_handlers::RequestPathHash,
            ) -> ::core::result::Result<(), $crate::runtime::request_router::Decline> {
                $(
                    if path_hash
                        == $crate::routing::request_handlers::RequestPathHash::of(
                            <$route as $crate::runtime::request_router::RequestRoute<S>>::PATH,
                        )
                    {
                        return <$route as $crate::runtime::request_router::RequestRoute<S>>::handle(cx).await;
                    }
                )+
                ::core::result::Result::Err($crate::runtime::request_router::Decline::Ignore)
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
        async fn handle(mut cx: RequestContext<'_, App>) -> Result<(), Decline> {
            cx.respond(b"ok")
        }
    }

    struct Greet;
    impl RequestRoute<App> for Greet {
        const PATH: &'static str = "/greet";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;
        async fn handle(mut cx: RequestContext<'_, App>) -> Result<(), Decline> {
            let greeting = cx.state.greeting;
            cx.respond(greeting)
        }
    }

    const ADMIN: IdentityHash = IdentityHash::new([0xAD; 16]);

    struct Admin;
    impl RequestRoute<App> for Admin {
        const PATH: &'static str = "/admin";
        const POLICY: RoutePolicy = RoutePolicy::AllowList(&[ADMIN]);
        async fn handle(_cx: RequestContext<'_, App>) -> Result<(), Decline> {
            Err(Decline::CloseLink)
        }
    }

    struct Ack;
    impl RequestRoute<App> for Ack {
        const PATH: &'static str = "/ack";
        const POLICY: RoutePolicy = RoutePolicy::AllowAll;
        async fn handle(mut cx: RequestContext<'_, App>) -> Result<(), Decline> {
            cx.respond(&[])
        }
    }

    fn registrations<R: RouteSet<App>>(_routes: R) -> &'static [(&'static str, RoutePolicy)] {
        R::REGISTRATIONS
    }

    #[test]
    fn the_route_set_is_the_registration_set_the_recipe_stands_up() {
        let registrations = registrations(crate::routes![Health, Greet, Admin, Ack]);
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
        async fn dispatch<R: RouteSet<App>>(
            _routes: &R,
            state: &App,
            path: &str,
            sink: &mut dyn ResponseSink,
        ) -> Result<(), Decline> {
            let request = InboundRequest::new(
                LinkId::new([1; 16]),
                RequestId([2; 16]),
                None,
                InstantMillis(0),
                Rtt(0),
                b"",
            );
            dispatch_request::<App, R>(state, RequestPathHash::of(path), request, sink).await
        }

        let routes = crate::routes![Health, Greet, Admin, Ack];
        let state = App { greeting: b"hi" };

        let mut greet = std::vec::Vec::new();
        assert_eq!(
            dispatch(&routes, &state, "/greet", &mut greet).await,
            Ok(())
        );
        assert_eq!(greet.as_slice(), b"hi");

        let mut health = std::vec::Vec::new();
        assert_eq!(
            dispatch(&routes, &state, "/health", &mut health).await,
            Ok(())
        );
        assert_eq!(health.as_slice(), b"ok");

        let mut ack = std::vec::Vec::new();
        assert_eq!(dispatch(&routes, &state, "/ack", &mut ack).await, Ok(()));
        assert!(ack.is_empty());

        let mut admin = std::vec::Vec::new();
        assert_eq!(
            dispatch(&routes, &state, "/admin", &mut admin).await,
            Err(Decline::CloseLink)
        );

        let mut miss = std::vec::Vec::new();
        assert_eq!(
            dispatch(&routes, &state, "/nope", &mut miss).await,
            Err(Decline::Ignore)
        );
    }
}
