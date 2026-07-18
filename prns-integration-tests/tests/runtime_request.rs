//! The typed request router, end to end over real TCP: a responder stood up by `PrnsNode::new` with a
//! one-route set answers a live request from an initiator. The handler *computes* its answer (it
//! echoes the request bytes and appends a suffix, assembling them straight into the grant), so this
//! exercises the whole dogfood path — `run` drives the reactor and the runner together, a
//! `RequestReceived` is forked to the runner, the route's `handle` fills the grant and returns
//! `Ok(())`, and the auto-upgrading respond carries the bytes back to the initiator. An integration
//! test, so it builds against the public API.

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EstablishLink,
    RatchetPolicy, SendRequest, SendRequestData, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::BitrateBps;
use personal_rns::routes;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_router::{Decline, RequestContext, RequestRoute, RoutePolicy};
use personal_rns::runtime::{
    Diagnostic, Manual, Message, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, RequestHandlerRegistration,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::client::TcpClientInterface;
use personal_rns::tcp::server::TcpServer;
use personal_rns::wire::DestinationHash;

const BITRATE: BitrateBps = BitrateBps::guess(1_000_000);
const QUERY_PATH: &str = "/test/echo";

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

/// The responder's app state — nothing to hold; the answer is computed from the request.
struct Responder;

/// One route: echo the request bytes back with a suffix, assembled into the grant — `write` then a
/// terminal `respond` that seals with `Ok(())`.
struct Echo;
impl RequestRoute<Responder> for Echo {
    const PATH: &'static str = QUERY_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;
    async fn handle(mut cx: RequestContext<'_, Responder>) -> Result<(), Decline> {
        let asked = cx.data;
        cx.write(asked);
        cx.respond(b"-pong")
    }
}

/// The split rung's route: a five-byte ask, an answer that outgrows a single resource segment.
struct Fat;
impl RequestRoute<Responder> for Fat {
    const PATH: &'static str = "/test/fat";
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;
    async fn handle(mut cx: RequestContext<'_, Responder>) -> Result<(), Decline> {
        cx.respond(&fat_body())
    }
}

fn fat_body() -> std::vec::Vec<u8> {
    b"the response outgrows a single segment ".repeat(30_000)
}

/// What the initiator pulls out of the curated event lane.
enum Heard {
    Destination(DestinationHash),
    Settled(CommandId, Settlement),
    Response(std::vec::Vec<u8>),
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_router_answers_a_live_request_over_tcp() {
    let responder_dest = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "bench",
        aspects: &["link"],
        identity: secret(0xA1),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_handlers: RequestHandlerRegistration::NodeRouteSet,
    };
    let dest_a = responder_dest
        .destination_hash()
        .expect("the test destination name is valid");

    // Responder node: a TCP server plus the route-backed Single it answers requests on.
    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [responder_dest],
        app_state: Responder,
        storage: GrowableHeap,
        routes: routes![Echo],
        on_event: |_event, _state| {},
        interfaces: Manual,
    });

    let announcer = node_a.handle();
    let _server_sup = announcer.supervise(server);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    let client = TcpClientInterface::new(addr, BITRATE, Duration::from_millis(100));
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "bench",
            aspects: &["link"],
            identity: secret(0xB2),
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            request_handlers: RequestHandlerRegistration::None,
        }],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: move |event, _state| {
            let mapped = match event {
                PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                    Some(Heard::Destination(destination))
                }
                PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                    Some(Heard::Settled(id, settlement))
                }
                PrnsEvent::Message(Message::Response { data, .. }) => {
                    Some(Heard::Response(data.to_vec()))
                }
                _ => None,
            };
            if let Some(event) = mapped {
                let _ = heard_tx.send(event);
            }
        },
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
    });
    let commands_b = node_b.handle();

    let conversation = async {
        let destination = loop {
            if let Heard::Destination(destination) =
                heard_rx.recv().await.expect("initiator stays alive")
            {
                break destination;
            }
        };
        assert_eq!(destination, dest_a, "heard the responder's destination");

        let link_cmd = commands_b
            .issue(EngineCommand::EstablishLink(EstablishLink { destination }))
            .expect("the initiator node is running");
        let link_id = loop {
            match heard_rx.recv().await.expect("initiator stays alive") {
                Heard::Settled(id, Settlement::EstablishLink(Ok(established)))
                    if id == link_cmd =>
                {
                    break established.link_id;
                }
                Heard::Settled(id, Settlement::EstablishLink(Err(failure))) if id == link_cmd => {
                    panic!("link refused: {failure:?}");
                }
                _ => {}
            }
        };

        commands_b
            .issue(EngineCommand::SendRequest(SendRequest {
                link_id,
                path_hash: RequestPathHash::of(QUERY_PATH),
                data: SendRequestData::from_slice(b"ping").expect("request fits a single packet"),
            }))
            .expect("the initiator node is running");
        loop {
            if let Heard::Response(data) = heard_rx.recv().await.expect("initiator stays alive") {
                break data;
            }
        }
    };

    // Both nodes' `run` loops are `!Send` and never return, so they're driven on this task and raced
    // against the initiator's conversation; assert on whichever the timeout resolves.
    tokio::select! {
        biased;
        answer = tokio::time::timeout(Duration::from_secs(10), conversation) => {
            let answer = answer.expect("the request round-trips within 10s");
            assert_eq!(answer.as_slice(), b"ping-pong", "the router computed and returned the answer");
        }
        () = node_a.run() => panic!("the responder's run loop ended unexpectedly"),
        () = node_b.run() => panic!("the initiator's run loop ended unexpectedly"),
    }
}

/// `request().await` auto-negotiating up, end to end over real TCP: the same Echo responder, but the
/// initiator drives the high-level `request` verb and exercises *both* rungs on one link — a small
/// payload that rides a single packet, and one too fat for the MDU that auto-promotes to a resource
/// in *both* directions (a request resource out, a response resource back). Proves the consumer
/// surface, the reactor's packet-vs-resource decision, and the response demux together — a consumer
/// never meets a size limit and the answer comes back with its round trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_auto_negotiates_both_rungs_over_tcp() {
    let responder_dest = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "bench",
        aspects: &["link"],
        identity: secret(0xC3),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_handlers: RequestHandlerRegistration::NodeRouteSet,
    };
    let dest_a = responder_dest
        .destination_hash()
        .expect("the test destination name is valid");

    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [responder_dest],
        app_state: Responder,
        storage: GrowableHeap,
        routes: routes![Echo],
        on_event: |_event, _state| {},
        interfaces: Manual,
    });

    let announcer = node_a.handle();
    let _server_sup = announcer.supervise(server);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    // The initiator's event lane carries only the announce it needs to find the responder;
    // `establish_link` and `request` return their own results (the demux suppresses them here).
    let client = TcpClientInterface::new(addr, BITRATE, Duration::from_millis(100));
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "bench",
            aspects: &["link"],
            identity: secret(0xD4),
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            request_handlers: RequestHandlerRegistration::None,
        }],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
    });
    let handle = node_b.handle();

    let conversation = async {
        let destination = loop {
            if heard_rx.recv().await.expect("initiator stays alive") == dest_a {
                break dest_a;
            }
        };
        let link_id = handle
            .establish_link(destination)
            .await
            .expect("the link establishes");

        // Packet rung: a small request rides a single packet.
        let (small, _rtt) = handle
            .request(link_id, RequestPathHash::of(QUERY_PATH), b"ping")
            .await
            .expect("the small request round-trips");
        assert_eq!(
            small.as_slice(),
            b"ping-pong",
            "the packet rung round-trips through request()",
        );

        // Resource rung: a request too fat for a packet auto-promotes both ways.
        let big = std::vec![0x5au8; 2000];
        let (large, _rtt) = handle
            .request(link_id, RequestPathHash::of(QUERY_PATH), &big)
            .await
            .expect("the big request round-trips");
        let mut expected = big.clone();
        expected.extend_from_slice(b"-pong");
        assert_eq!(
            large, expected,
            "the resource rung round-trips through request(), auto-promoted both directions",
        );
    };

    tokio::select! {
        biased;
        outcome = tokio::time::timeout(Duration::from_secs(10), conversation) => {
            outcome.expect("both requests round-trip within 10s");
        }
        () = node_a.run() => panic!("the responder's run loop ended unexpectedly"),
        () = node_b.run() => panic!("the initiator's run loop ended unexpectedly"),
    }
}

/// The split rung, end to end over real TCP: a five-byte request whose answer is bigger than one
/// resource segment (`MAX_EFFICIENT_SIZE`), so the responder chains it across segments and the
/// initiator's `request().await` still returns the whole body — the receive gate admits every
/// response segment past the default `AcceptNone` strategy, the segments re-assemble host-side in
/// arrival order, and the pending request settles at final assembly instead of timing out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_split_response_answers_a_small_request_over_tcp() {
    let responder_dest = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "bench",
        aspects: &["link"],
        identity: secret(0xE5),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_handlers: RequestHandlerRegistration::NodeRouteSet,
    };
    let dest_a = responder_dest
        .destination_hash()
        .expect("the test destination name is valid");

    let server = TcpServer::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [responder_dest],
        app_state: Responder,
        storage: GrowableHeap,
        routes: routes![Fat],
        on_event: |_event, _state| {},
        interfaces: Manual,
    });

    let announcer = node_a.handle();
    let _server_sup = announcer.supervise(server);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    let client = TcpClientInterface::new(addr, BITRATE, Duration::from_millis(100));
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "bench",
            aspects: &["link"],
            identity: secret(0xF6),
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            request_handlers: RequestHandlerRegistration::None,
        }],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
    });
    let handle = node_b.handle();

    let conversation = async {
        let destination = loop {
            if heard_rx.recv().await.expect("initiator stays alive") == dest_a {
                break dest_a;
            }
        };
        let link_id = handle
            .establish_link(destination)
            .await
            .expect("the link establishes");

        let (answer, _rtt) = handle
            .request(link_id, RequestPathHash::of("/test/fat"), b"gimme")
            .await
            .expect("the split response round-trips");
        assert_eq!(
            answer,
            fat_body(),
            "the whole chained response lands, in order, and settles the request",
        );
    };

    tokio::select! {
        biased;
        outcome = tokio::time::timeout(Duration::from_secs(30), conversation) => {
            outcome.expect("the split response round-trips within 30s");
        }
        () = node_a.run() => panic!("the responder's run loop ended unexpectedly"),
        () = node_b.run() => panic!("the initiator's run loop ended unexpectedly"),
    }
}
