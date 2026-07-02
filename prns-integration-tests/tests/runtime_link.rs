//! A link establishes and carries data across two live nodes over UDP — the public-API successor
//! to the two-reactor UDP capstone that lived beside the interface impl. Two `Prns` nodes talk
//! through a fixed-peer `UdpInterface` pair (one raw wire packet per datagram, no framing); the
//! responder announces, the initiator hears it, establishes a link, and round-trips a request over
//! it. The TCP leg of the same claim lives in `runtime_request.rs`.

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::udp::core::UDP_BITRATE_GUESS_BPS;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::request_router::{Decline, RequestContext, RequestRoute, RoutePolicy};
use personal_rns::runtime::{Diagnostic, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe};
use personal_rns::storage::GrowableHeap;
use personal_rns::{interfaces, routes};
use personal_rns::udp::UdpInterface;

const QUERY_PATH: &str = "/test/echo";

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

/// The responder's app state — nothing to hold; the answer is computed from the request.
struct Responder;

/// One route: echo the request bytes back with a suffix.
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

/// Two loopback ports free right now: bind ephemeral sockets to learn them and drop both, so the
/// fixed-peer interfaces can rebind them pointed at each other. A small TOCTOU window, fine for a
/// single-process test.
async fn two_free_udp_ports() -> std::io::Result<(std::net::SocketAddr, std::net::SocketAddr)> {
    let probe_a = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let addr_a = probe_a.local_addr()?;
    let probe_b = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let addr_b = probe_b.local_addr()?;
    drop(probe_a);
    drop(probe_b);
    Ok((addr_a, addr_b))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_establishes_and_carries_data_across_two_nodes_over_udp() {
    let responder_dest = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "bench",
        aspects: &["link"],
        identity: secret(0xA7),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    };
    let dest_a = responder_dest
        .destination_hash()
        .expect("the test destination name is valid");

    let (addr_a, addr_b) = two_free_udp_ports().await.expect("probes two free ports");
    let udp_a = UdpInterface::bind(addr_a, addr_b, UDP_BITRATE_GUESS_BPS)
        .await
        .expect("binds the responder socket");
    let udp_b = UdpInterface::bind(addr_b, addr_a, UDP_BITRATE_GUESS_BPS)
        .await
        .expect("binds the initiator socket");

    let node_a = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [responder_dest],
        app_state: Responder,
        storage: GrowableHeap,
        routes: routes![Echo],
        on_event: |_event, _state| {},
        interfaces: interfaces![udp_a],
    });

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

    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "bench",
            aspects: &["link"],
            identity: secret(0xB8),
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::NoRatchets,
        }],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: interfaces![udp_b],
    });
    let handle = node_b.handle();

    let conversation = async {
        loop {
            if heard_rx.recv().await.expect("initiator stays alive") == dest_a {
                break;
            }
        }
        let link_id = handle
            .establish_link(dest_a)
            .await
            .expect("the link establishes over UDP");
        let (answer, _rtt) = handle
            .request(link_id, RequestPathHash::of(QUERY_PATH), b"ping")
            .await
            .expect("the request round-trips over the link");
        assert_eq!(
            answer.as_slice(),
            b"ping-pong",
            "link data crossed both ways over the UDP pair",
        );
    };

    tokio::select! {
        biased;
        outcome = tokio::time::timeout(Duration::from_secs(10), conversation) => {
            outcome.expect("the link establishes and the request round-trips within 10s");
        }
        () = node_a.run() => panic!("the responder's run loop ended unexpectedly"),
        () = node_b.run() => panic!("the initiator's run loop ended unexpectedly"),
    }
}
