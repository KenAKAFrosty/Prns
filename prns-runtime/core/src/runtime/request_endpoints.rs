use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::routing::links::request::{
    packed_binary_len, write_packed_binary_header, RequestId, MAX_PACKED_BINARY_HEADER_LEN,
};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};
use crate::units::RttMillis;
use crate::wire::DestinationHash;

use super::PrnsNodeApi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestEndpointPolicy {
    AllowNone,
    AllowAll,
    RequireIdentified,
    AllowList(&'static [IdentityHash]),
}

impl RequestEndpointPolicy {
    #[must_use]
    pub fn engine_policy(self) -> RequestPolicy {
        match self {
            RequestEndpointPolicy::AllowNone => RequestPolicy::AllowNone,
            RequestEndpointPolicy::AllowAll => RequestPolicy::AllowAll,
            RequestEndpointPolicy::RequireIdentified => RequestPolicy::RequireIdentified,
            RequestEndpointPolicy::AllowList(_) => RequestPolicy::AllowList,
        }
    }

    /// The identities to admit at registration — non-empty only for [`RequestEndpointPolicy::AllowList`].
    #[must_use]
    pub fn seed_list(self) -> &'static [IdentityHash] {
        match self {
            RequestEndpointPolicy::AllowList(list) => list,
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
    ResponseTooLarge,
}

pub trait ResponseSink {
    fn put_packed(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded>;

    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded>;

    fn put_static_bytes(&mut self, bytes: &'static [u8]) -> Result<(), ResponseCapacityExceeded> {
        self.put_bytes(bytes)
    }

    fn put_static_file(
        &mut self,
        _name: &'static str,
        _bytes: &'static [u8],
    ) -> Result<(), ResponseCapacityExceeded> {
        Err(ResponseCapacityExceeded)
    }

    #[cfg(feature = "std")]
    fn put_open_bytes(
        &mut self,
        _file: std::fs::File,
        _byte_len: u64,
    ) -> Result<(), ResponseCapacityExceeded> {
        Err(ResponseCapacityExceeded)
    }

    #[cfg(feature = "std")]
    fn put_open_file(
        &mut self,
        _name: &str,
        _file: std::fs::File,
        _byte_len: u64,
    ) -> Result<(), ResponseCapacityExceeded> {
        Err(ResponseCapacityExceeded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseCapacityExceeded;

#[cfg(feature = "alloc")]
impl ResponseSink for alloc::vec::Vec<u8> {
    fn put_packed(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        self.extend_from_slice(bytes);
        Ok(())
    }

    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(bytes.len(), &mut header)
            .map_err(|_| ResponseCapacityExceeded)?;
        self.reserve(header_len + bytes.len());
        self.extend_from_slice(&header[..header_len]);
        self.extend_from_slice(bytes);
        Ok(())
    }
}

impl<const N: usize> ResponseSink for heapless::Vec<u8, N> {
    fn put_packed(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        self.extend_from_slice(bytes)
            .map_err(|_| ResponseCapacityExceeded)
    }

    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        let packed_len = packed_binary_len(bytes.len()).ok_or(ResponseCapacityExceeded)?;
        if self.capacity() - self.len() < packed_len {
            return Err(ResponseCapacityExceeded);
        }
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(bytes.len(), &mut header)
            .map_err(|_| ResponseCapacityExceeded)?;
        self.extend_from_slice(&header[..header_len])
            .map_err(|_| ResponseCapacityExceeded)?;
        self.extend_from_slice(bytes)
            .map_err(|_| ResponseCapacityExceeded)
    }
}

/// Only needed if you don't respond to the request inside your [`handle`](RequestEndpoint::handle) function.
/// See [`respond_token`](RequestContext::respond_token).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RespondToken {
    pub link_id: LinkId,
    pub request_id: RequestId,
    /// The link's measured round trip when the request arrived.
    pub rtt: RttMillis,
}

pub struct InboundRequest<'a> {
    pub destination: DestinationHash,
    pub data: &'a [u8],
    pub requester: Option<IdentityHash>,
    pub requested_at: InstantMillis,
    respond_token: RespondToken,
}

impl<'a> InboundRequest<'a> {
    #[must_use]
    pub fn new(
        destination: DestinationHash,
        link_id: LinkId,
        request_id: RequestId,
        requester: Option<IdentityHash>,
        requested_at: InstantMillis,
        rtt: RttMillis,
        data: &'a [u8],
    ) -> Self {
        Self {
            destination,
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
    pub destination: DestinationHash,
    pub data: &'a [u8],
    pub requester: Option<IdentityHash>,
    pub requested_at: InstantMillis,
    respond_token: RespondToken,
    sink: &'a mut dyn ResponseSink,
}

impl<S> RequestContext<'_, S> {
    /// Send a normal application response exactly as supplied.
    ///
    /// This is the default for text, protocol messages, and arbitrary byte payloads:
    ///
    /// ```ignore
    /// context.respond("pong")
    /// ```
    ///
    /// RNS calls this payload "packed", but it does not require MessagePack. Use
    /// [`respond_messagepack_bytes`](Self::respond_messagepack_bytes) only when the peer
    /// specifically expects a MessagePack `bin` value, and the file methods only for
    /// named-file/resource semantics.
    pub fn respond(&mut self, data: impl AsRef<[u8]>) -> Result<(), Decline> {
        self.sink
            .put_packed(data.as_ref())
            .map_err(|_| Decline::ResponseTooLarge)
    }

    /// Legacy Reticulum spelling for [`respond`](Self::respond).
    #[deprecated(note = "use RequestContext::respond for exact application payloads")]
    #[doc(hidden)]
    pub fn respond_packed(&mut self, bytes: &[u8]) -> Result<(), Decline> {
        self.respond(bytes)
    }

    /// Encode `bytes` as one MessagePack `bin` value before responding.
    pub fn respond_messagepack_bytes(&mut self, bytes: &[u8]) -> Result<(), Decline> {
        self.sink
            .put_bytes(bytes)
            .map_err(|_| Decline::ResponseTooLarge)
    }

    /// Legacy ambiguous spelling for
    /// [`respond_messagepack_bytes`](Self::respond_messagepack_bytes).
    #[deprecated(
        note = "use RequestContext::respond_messagepack_bytes for a MessagePack bin value"
    )]
    #[doc(hidden)]
    pub fn respond_bytes(&mut self, bytes: &[u8]) -> Result<(), Decline> {
        self.respond_messagepack_bytes(bytes)
    }

    /// Encode static bytes as one MessagePack `bin` value without first copying the source.
    pub fn respond_static_messagepack_bytes(
        &mut self,
        bytes: &'static [u8],
    ) -> Result<(), Decline> {
        self.sink
            .put_static_bytes(bytes)
            .map_err(|_| Decline::ResponseTooLarge)
    }

    /// Legacy ambiguous spelling for
    /// [`respond_static_messagepack_bytes`](Self::respond_static_messagepack_bytes).
    #[deprecated(
        note = "use RequestContext::respond_static_messagepack_bytes for a static MessagePack bin value"
    )]
    #[doc(hidden)]
    pub fn respond_static_bytes(&mut self, bytes: &'static [u8]) -> Result<(), Decline> {
        self.respond_static_messagepack_bytes(bytes)
    }

    /// Respond with a Reticulum Resource whose metadata names the file for native clients.
    ///
    /// The bytes remain borrowed from static storage; resource segmentation copies only the
    /// current transfer window into the outgoing resource buffer.
    pub fn respond_static_file(
        &mut self,
        name: &'static str,
        bytes: &'static [u8],
    ) -> Result<(), Decline> {
        self.sink
            .put_static_file(name, bytes)
            .map_err(|_| Decline::ResponseTooLarge)
    }

    /// Respond with bytes from an already-open regular file.
    ///
    /// Host runtimes keep the handle open until its response lane is available, then read it in
    /// bounded segments. Opening and validating the handle before calling this method keeps path
    /// policy in the application and avoids retaining the complete payload per queued request.
    #[cfg(feature = "std")]
    #[doc(hidden)]
    pub fn respond_open_bytes(
        &mut self,
        file: std::fs::File,
        byte_len: u64,
    ) -> Result<(), Decline> {
        self.sink
            .put_open_bytes(file, byte_len)
            .map_err(|_| Decline::ResponseTooLarge)
    }

    /// Respond with an already-open regular file and Reticulum filename metadata.
    ///
    /// The host runtime streams the file after acquiring the response lane, so queued requests
    /// retain one handle and a small descriptor rather than a copy of the file.
    #[cfg(feature = "std")]
    #[doc(hidden)]
    pub fn respond_open_file(
        &mut self,
        name: &str,
        file: std::fs::File,
        byte_len: u64,
    ) -> Result<(), Decline> {
        self.sink
            .put_open_file(name, file, byte_len)
            .map_err(|_| Decline::ResponseTooLarge)
    }

    pub fn write_packed(&mut self, bytes: &[u8]) -> Result<&mut Self, ResponseCapacityExceeded> {
        self.sink.put_packed(bytes)?;
        Ok(self)
    }

    /// The token to answer this request later. You can keep it, return `Err(Decline::Ignore)` now, and answer from another task through the platform command handle.
    ///
    /// In this context, "keeping it" usually means capturing it somewhere in your AppState
    #[must_use]
    pub fn respond_token(&self) -> RespondToken {
        self.respond_token
    }
}

/// What a requester names to reach a [`RequestEndpoint`]: the stable hash of its `ENDPOINT_ID` string.
pub type RequestEndpointId = RequestPathHash;

#[allow(async_fn_in_trait)]
pub trait RequestEndpoint<AppState = ()> {
    /// You can use whatever string value you like (it's hashed and truncated so the wire length will be stable), but it's common convention to use URL/filesystem-like syntax, e.g., "/example/thing"
    const ENDPOINT_ID: &'static str;
    const POLICY: RequestEndpointPolicy;
    async fn handle(
        context: RequestContext<'_, AppState>,
        node: &impl PrnsNodeApi,
    ) -> Result<(), Decline>;
}

/// A compile-time set of endpoints, produced by [`request_endpoints!`](crate::request_endpoints); you probably want
/// that macro rather than this trait directly.
#[allow(async_fn_in_trait)]
pub trait RequestEndpointSet<S> {
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)];
    async fn dispatch(
        cx: RequestContext<'_, S>,
        node: &impl PrnsNodeApi,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline>;
}

/// The empty route set — what [`request_endpoints!`](crate::request_endpoints) with no arms hands back, and what a node
/// that serves no requests carries. It registers nothing and declines every request as `Ignore`.
impl<S> RequestEndpointSet<S> for () {
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[];
    async fn dispatch(
        _cx: RequestContext<'_, S>,
        _node: &impl PrnsNodeApi,
        _path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        Err(Decline::Ignore)
    }
}

/// The value [`request_endpoints!`](crate::request_endpoints) hands back when given no endpoints — the empty [`RequestEndpointSet`].
/// A named constructor so the macro needn't expand to a bare `()`, which `clippy::unused_unit`
/// flags at every call site.
pub const fn no_request_endpoints() {}

/// Route one request to the handler its `path_hash` selects, building the [`RequestContext`] over
/// the app's shared `state` and the runner's grant `sink`. `RequestEndpointSet::dispatch` is a static fn, so
/// the runner dispatches with only `&state` and the endpoint-set type `R` — no `Router` wrapper.
pub async fn dispatch_request<'a, S, R: RequestEndpointSet<S>>(
    state: &'a S,
    node: &impl PrnsNodeApi,
    path_hash: RequestPathHash,
    request: InboundRequest<'a>,
    sink: &'a mut dyn ResponseSink,
) -> Result<(), Decline> {
    let cx = RequestContext {
        state,
        destination: request.destination,
        data: request.data,
        requester: request.requester,
        requested_at: request.requested_at,
        respond_token: request.respond_token(),
        sink,
    };
    R::dispatch(cx, node, path_hash).await
}

/// Compose route types into a [`RequestEndpointSet`] value, e.g., `request_endpoints![Health, Echo, Status]`. Each arm awaits
/// a concrete handler future, so the set is monomorphized. There's no boxing and it's`no_std`-clean.
#[macro_export]
macro_rules! request_endpoints {
    () => {
        $crate::runtime::request_endpoints::no_request_endpoints()
    };
    ($($endpoint:ty),+ $(,)?) => {{
        struct RequestEndpointSetImpl;
        impl<S> $crate::runtime::request_endpoints::RequestEndpointSet<S> for RequestEndpointSetImpl
        where
            $($endpoint: $crate::runtime::request_endpoints::RequestEndpoint<S>,)+
        {
            const REGISTRATIONS: &'static [(&'static str, $crate::runtime::request_endpoints::RequestEndpointPolicy)] = &[
                $((
                    <$endpoint as $crate::runtime::request_endpoints::RequestEndpoint<S>>::ENDPOINT_ID,
                    <$endpoint as $crate::runtime::request_endpoints::RequestEndpoint<S>>::POLICY,
                ),)+
            ];

            async fn dispatch(
                cx: $crate::runtime::request_endpoints::RequestContext<'_, S>,
                node: &impl $crate::runtime::PrnsNodeApi,
                path_hash: $crate::routing::request_handlers::RequestPathHash,
            ) -> ::core::result::Result<(), $crate::runtime::request_endpoints::Decline> {
                $(
                    if path_hash
                        == $crate::routing::request_handlers::RequestPathHash::of(
                            <$endpoint as $crate::runtime::request_endpoints::RequestEndpoint<S>>::ENDPOINT_ID,
                        )
                    {
                        return <$endpoint as $crate::runtime::request_endpoints::RequestEndpoint<S>>::handle(cx, node).await;
                    }
                )+
                ::core::result::Result::Err($crate::runtime::request_endpoints::Decline::Ignore)
            }
        }
        RequestEndpointSetImpl
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    struct App {
        greeting: &'static [u8],
    }

    struct Health;
    impl RequestEndpoint<App> for Health {
        const ENDPOINT_ID: &'static str = "/health";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;
        async fn handle(
            mut cx: RequestContext<'_, App>,
            _node: &impl PrnsNodeApi,
        ) -> Result<(), Decline> {
            cx.respond("ok")
        }
    }

    struct Greet;
    impl RequestEndpoint<App> for Greet {
        const ENDPOINT_ID: &'static str = "/greet";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;
        async fn handle(
            mut cx: RequestContext<'_, App>,
            _node: &impl PrnsNodeApi,
        ) -> Result<(), Decline> {
            let greeting = cx.state.greeting;
            cx.respond(greeting)
        }
    }

    const ADMIN: IdentityHash = IdentityHash::new([0xAD; 16]);

    struct Admin;
    impl RequestEndpoint<App> for Admin {
        const ENDPOINT_ID: &'static str = "/admin";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[ADMIN]);
        async fn handle(
            _cx: RequestContext<'_, App>,
            _node: &impl PrnsNodeApi,
        ) -> Result<(), Decline> {
            Err(Decline::CloseLink)
        }
    }

    struct Identified;
    impl RequestEndpoint<App> for Identified {
        const ENDPOINT_ID: &'static str = "/identified";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::RequireIdentified;
        async fn handle(
            _cx: RequestContext<'_, App>,
            _node: &impl PrnsNodeApi,
        ) -> Result<(), Decline> {
            Ok(())
        }
    }

    struct Ack;
    impl RequestEndpoint<App> for Ack {
        const ENDPOINT_ID: &'static str = "/ack";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;
        async fn handle(
            mut cx: RequestContext<'_, App>,
            _node: &impl PrnsNodeApi,
        ) -> Result<(), Decline> {
            cx.respond([0u8; 0])
        }
    }

    fn registrations<R: RequestEndpointSet<App>>(
        _endpoints: R,
    ) -> &'static [(&'static str, RequestEndpointPolicy)] {
        R::REGISTRATIONS
    }

    #[test]
    fn the_endpoint_set_is_the_registration_set_the_recipe_stands_up() {
        let registrations = registrations(crate::request_endpoints![
            Health, Greet, Admin, Identified, Ack
        ]);
        assert_eq!(registrations.len(), 5);
        assert_eq!(
            registrations[0],
            ("/health", RequestEndpointPolicy::AllowAll)
        );
        assert_eq!(registrations[2].0, "/admin");
        assert_eq!(registrations[2].1.engine_policy(), RequestPolicy::AllowList);
        assert_eq!(registrations[2].1.seed_list(), &[ADMIN]);
        assert_eq!(
            registrations[3].1.engine_policy(),
            RequestPolicy::RequireIdentified,
        );
        assert!(registrations[3].1.seed_list().is_empty());
        assert_eq!(registrations[0].1.engine_policy(), RequestPolicy::AllowAll);
        assert!(registrations[0].1.seed_list().is_empty());
    }

    #[test]
    fn messagepack_binary_sinks_frame_atomically() {
        let mut exact = heapless::Vec::<u8, 7>::new();
        exact.put_bytes(b"hello").unwrap();
        assert_eq!(exact.as_slice(), &[0xC4, 5, b'h', b'e', b'l', b'l', b'o']);

        let mut short = heapless::Vec::<u8, 6>::new();
        assert_eq!(short.put_bytes(b"hello"), Err(ResponseCapacityExceeded));
        assert!(short.is_empty());
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn dispatch_endpoints_by_path_then_answers_or_declines() {
        futures_executor::block_on(async {
            async fn dispatch<R: RequestEndpointSet<App>>(
                _endpoints: &R,
                state: &App,
                path: &str,
                sink: &mut dyn ResponseSink,
            ) -> Result<(), Decline> {
                let request = InboundRequest::new(
                    DestinationHash::new([3; 16]),
                    LinkId::new([1; 16]),
                    RequestId([2; 16]),
                    None,
                    InstantMillis(0),
                    RttMillis::new(0),
                    b"",
                );
                dispatch_request::<App, R>(state, &(), RequestPathHash::of(path), request, sink)
                    .await
            }

            let endpoints = crate::request_endpoints![Health, Greet, Admin, Ack];
            let state = App { greeting: b"hi" };

            let mut greet = std::vec::Vec::new();
            assert_eq!(
                dispatch(&endpoints, &state, "/greet", &mut greet).await,
                Ok(())
            );
            assert_eq!(greet.as_slice(), b"hi");

            let mut health = std::vec::Vec::new();
            assert_eq!(
                dispatch(&endpoints, &state, "/health", &mut health).await,
                Ok(())
            );
            assert_eq!(health.as_slice(), b"ok");

            let mut ack = std::vec::Vec::new();
            assert_eq!(dispatch(&endpoints, &state, "/ack", &mut ack).await, Ok(()));
            assert!(ack.is_empty());

            let mut admin = std::vec::Vec::new();
            assert_eq!(
                dispatch(&endpoints, &state, "/admin", &mut admin).await,
                Err(Decline::CloseLink)
            );

            let mut miss = std::vec::Vec::new();
            assert_eq!(
                dispatch(&endpoints, &state, "/nope", &mut miss).await,
                Err(Decline::Ignore)
            );
        });
    }
}
