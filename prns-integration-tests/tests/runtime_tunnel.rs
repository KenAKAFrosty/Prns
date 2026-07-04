use core::time::Duration;

use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routes;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::tunnel::{parse_synthesize_payload, SYNTHESIZE_PAYLOAD_LEN};
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{PreConfiguredDestination, Prns, PrnsRecipe, TokioPrnsHandle};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::client::TcpClientInterface;
use personal_rns::wire::{TransportId, HEADER_MIN_LEN};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

const BITRATE: u32 = 1_000_000;
const FLAG: u8 = 0x7E;
const ESC: u8 = 0x7D;
const ESC_MASK: u8 = 0x20;

async fn first_frame(socket: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut escaped = Vec::new();
    let mut in_frame = false;
    let mut buf = [0u8; 512];
    loop {
        let n = socket.read(&mut buf).await?;
        assert_ne!(n, 0, "the wire stays up while the synthesize is owed");
        for &byte in &buf[..n] {
            if byte == FLAG {
                if in_frame && !escaped.is_empty() {
                    let mut frame = Vec::new();
                    let mut i = 0;
                    while i < escaped.len() {
                        if escaped[i] == ESC && i + 1 < escaped.len() {
                            frame.push(escaped[i + 1] ^ ESC_MASK);
                            i += 2;
                        } else {
                            frame.push(escaped[i]);
                            i += 1;
                        }
                    }
                    return Ok(frame);
                }
                in_frame = true;
                escaped.clear();
            } else if in_frame {
                escaped.push(byte);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recipe_node_synthesizes_a_tunnel_when_its_transport_is_a_held_identity() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the probe listener binds");
    let addr = listener.local_addr().expect("bound address").to_string();

    let secret = Zeroizing::new([0xC3u8; IDENTITY_SECRET_KEY_LEN]);

    let client = TcpClientInterface::new(addr, BITRATE, Duration::from_millis(100));
    let node = Prns::new(PrnsRecipe {
        transport_identity: Some(secret.clone()),
        pre_configured_destinations: [PreConfiguredDestination::Single {
            app_name: "bench",
            aspects: &["tunnel"],
            identity: secret,
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
        }],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: |_event, _state| {},
        interfaces: |node: &TokioPrnsHandle| {
            node.attach(client);
        },
    });

    let probe = async move {
        let (mut socket, _) = listener.accept().await.expect("the node connects");
        let frame = first_frame(&mut socket)
            .await
            .expect("the node sends a frame");
        assert_eq!(
            frame.len(),
            HEADER_MIN_LEN + SYNTHESIZE_PAYLOAD_LEN,
            "the first frame the node sends on connect is a synthesize-sized packet",
        );
        assert!(
            parse_synthesize_payload(&frame[HEADER_MIN_LEN..]).is_some(),
            "the node auto-emitted a valid signed tunnel synthesize, no hand-rolling",
        );
    };

    tokio::select! {
        () = node.run() => unreachable!("the node's run loop returned"),
        result = tokio::time::timeout(Duration::from_secs(10), probe) => {
            result.expect("the node synthesizes within the window");
        }
    }
}
