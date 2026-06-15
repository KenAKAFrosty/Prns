//! The typed request router, end to end over real TCP: a responder stood up by `Prns::new` with a
//! one-route set answers a live request from an initiator. The handler *computes* its answer (it
//! echoes the request bytes and appends a suffix, assembling them straight into the grant), so this
//! exercises the whole dogfood path — `run` drives the reactor and the runner together, a
//! `RequestReceived` is forked to the runner, the route's `handle` fills the grant and returns
//! `Ok(())`, and the auto-upgrading respond carries the bytes back to the initiator. An integration
//! test, so it builds against the public API.

#![cfg(feature = "tcp")]

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EstablishLink,
    RatchetPolicy, SendRequest, SendRequestData, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::reactor::interfaces::tcp::impls::tokio::{
    TcpClientInterface, TcpServerInterface,
};
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::request_router::{Decline, RequestContext, RequestRoute, RoutePolicy};
use personal_rns::runtime::{
    Diagnostic, Message, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;
use personal_rns::{interfaces, routes};

const BITRATE: u32 = 1_000_000;
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

/// What the initiator pulls out of the curated event lane.
enum Heard {
    Destination(DestinationHash),
    Settled(CommandId, Settlement),
    Response(std::vec::Vec<u8>),
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_router_answers_a_live_request_over_tcp() {
    let responder_dest = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects: &["link"],
        identity: secret(0xA1),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    };
    let dest_a = responder_dest
        .destination_hash()
        .expect("the test destination name is valid");

    // Responder node: a TCP server plus the route-backed Single it answers requests on. `Prns::new`
    // registers the routes' handlers on every Single it stands up, so the destination carries them.
    let server = TcpServerInterface::bind("127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [responder_dest],
        app_state: Responder,
        storage: GrowableHeap,
        routes: routes![Echo],
        on_event: |_event, _state| {},
        interfaces: interfaces![server],
    });

    // The responder announces on a cadence so the initiator can find it — pure app policy.
    let announcer = node_a.handle();
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

    // Initiator node: a TCP client to the responder; reports what it hears on the event lane.
    let client = TcpClientInterface::new(addr, BITRATE, Duration::from_millis(100));
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            app_name: "bench",
            aspects: &["link"],
            identity: secret(0xB2),
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::NoRatchets,
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
        interfaces: interfaces![client],
    });
    let commands_b = node_b.handle();

    // The initiator: hear the responder, establish a link, ask once, and check the answer.
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
