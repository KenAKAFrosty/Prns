//! The typed request router, end to end over real TCP: a responder stood up by `Prns::serve` with
//! a one-route `Router` answers a live request from an initiator. The handler *computes* its answer
//! (it echoes the request bytes and appends a suffix, assembling them straight into the grant), so
//! this exercises the whole dogfood path — `serve` drives the reactor and the runner together, a
//! `RequestReceived` is forked to the runner, the route's `handle` fills the grant and returns
//! `Ok(())`, and the auto-upgrading respond carries the bytes back to the initiator. An integration
//! test, so it builds against the public API.

#![cfg(feature = "tcp")]

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EstablishLink,
    IssuedCommand, RatchetPolicy, SendRequest, SendRequestData, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::impls::tokio_reactor::TokioHost;
use personal_rns::reactor::interface_seam::Interface;
use personal_rns::reactor::interfaces::tcp::impls::tokio::{
    TcpClientInterface, TcpServerInterface,
};
use personal_rns::routes;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    Decline, Diagnostic, Message, Prns, PrnsEvent, Recipe, RequestCx, RequestRoute, RoutePolicy,
    Router, StartingDestination, TokioBind,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;

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
    async fn handle(mut cx: RequestCx<'_, Responder>) -> Result<(), Decline> {
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
    let router = Router::new(Responder, routes![Echo]);
    let responder_dest = StartingDestination::Single {
        app_name: "bench",
        aspects: &["link"],
        identity: secret(0xA1),
        app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_handlers: router.registrations(),
    };
    let dest_a = responder_dest.address();

    // Responder node: a TCP server plus the router-backed Single it answers requests on.
    let (mut bind_a, commands_a) = TokioBind::<GrowableHeap>::new(TokioHost::new());
    let server = TcpServerInterface::bind(InterfaceId::new([0xA0; 16]), "127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let seam_a = bind_a.attach(server.descriptor());
    tokio::spawn(server.run(seam_a));

    // The responder announces on a cadence so the initiator can find it — pure app policy.
    let announcer = commands_a.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        let mut id = 1u64;
        loop {
            ticker.tick().await;
            let issued = announcer.issue(IssuedCommand {
                id: CommandId(id),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            });
            if !issued {
                break;
            }
            id += 1;
        }
    });

    // Initiator node: a TCP client to the responder; reports what it hears on the event lane.
    let initiator_dest = StartingDestination::Single {
        app_name: "bench",
        aspects: &["link"],
        identity: secret(0xB2),
        app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_handlers: &[],
    };
    let (mut bind_b, commands_b) = TokioBind::<GrowableHeap>::new(TokioHost::new());
    let client = TcpClientInterface::new(
        InterfaceId::new([0xB0; 16]),
        addr,
        BITRATE,
        Duration::from_millis(100),
    );
    let seam_b = bind_b.attach(client.descriptor());
    tokio::spawn(client.run(seam_b));
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(Prns::run(
        Recipe {
            transport: None,
            destinations: [initiator_dest],
            bind: bind_b,
        },
        move |event| {
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
    ));

    // The initiator: hear the responder, establish a link, ask once, and check the answer.
    let conversation = async {
        let destination = loop {
            match heard_rx.recv().await.expect("initiator stays alive") {
                Heard::Destination(destination) => break destination,
                _ => {}
            }
        };
        assert_eq!(destination, dest_a, "heard the responder's destination");

        commands_b.issue(IssuedCommand {
            id: CommandId(1),
            command: EngineCommand::EstablishLink(EstablishLink { destination }),
        });
        let link_id = loop {
            match heard_rx.recv().await.expect("initiator stays alive") {
                Heard::Settled(CommandId(1), Settlement::EstablishLink(Ok(established))) => {
                    break established.link_id;
                }
                Heard::Settled(CommandId(1), Settlement::EstablishLink(Err(failure))) => {
                    panic!("link refused: {failure:?}");
                }
                _ => {}
            }
        };

        commands_b.issue(IssuedCommand {
            id: CommandId(2),
            command: EngineCommand::SendRequest(SendRequest {
                link_id,
                path_hash: RequestPathHash::of(QUERY_PATH),
                data: SendRequestData::from_slice(b"ping").expect("request fits a single packet"),
            }),
        });
        loop {
            match heard_rx.recv().await.expect("initiator stays alive") {
                Heard::Response(data) => break data,
                _ => {}
            }
        }
    };

    // `Prns::serve` is the responder's whole loop; it never returns, so race it against the
    // initiator's conversation and assert on whichever the timeout resolves.
    let serve = Prns::serve(
        Recipe {
            transport: None,
            destinations: [responder_dest],
            bind: bind_a,
        },
        router,
        commands_a,
        |_event| {},
    );
    tokio::select! {
        _ = serve => panic!("the responder's serve loop ended unexpectedly"),
        answer = tokio::time::timeout(Duration::from_secs(10), conversation) => {
            let answer = answer.expect("the request round-trips within 10s");
            assert_eq!(answer.as_slice(), b"ping-pong", "the router computed and returned the answer");
        }
    }
}
