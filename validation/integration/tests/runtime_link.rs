use core::time::Duration;
use personal_rns::runtime::NoPersistence;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::udp::UDP_BITRATE_ESTIMATE;
use personal_rns::request_endpoints;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{
    Diagnostic, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::udp::UdpInterface;

const QUERY_PATH: &str = "/test/echo";

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

struct Responder;

struct Echo;
impl RequestEndpoint<Responder> for Echo {
    const ENDPOINT_ID: &'static str = QUERY_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;
    async fn handle(
        mut cx: RequestContext<'_, Responder>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        let asked = cx.data;
        let _ = cx.write_packed(asked);
        cx.respond(b"-pong")
    }
}

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
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::Yes,
    };
    let dest_a = responder_dest
        .destination_hash()
        .expect("the test destination name is valid");

    let (addr_a, addr_b) = two_free_udp_ports().await.expect("probes two free ports");
    let udp_a = UdpInterface::bind(addr_a, addr_b, UDP_BITRATE_ESTIMATE)
        .await
        .expect("binds the responder socket");
    let udp_b = UdpInterface::bind(addr_b, addr_a, UDP_BITRATE_ESTIMATE)
        .await
        .expect("binds the initiator socket");

    let node_a = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [responder_dest],
        app_state: Responder,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![Echo],
        on_event: |_event, _state| {},
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(udp_a);
        },
        persistence: NoPersistence,
    });

    let announcer = node_a.handle();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
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
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "bench",
            aspects: &["link"],
            identity: secret(0xB8),
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        }],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(udp_b);
        },
        persistence: NoPersistence,
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
        result = node_a.run() => panic!("the responder's run loop ended unexpectedly: {result:?}"),
        result = node_b.run() => panic!("the initiator's run loop ended unexpectedly: {result:?}"),
    }
}
