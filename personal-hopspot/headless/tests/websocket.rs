#![cfg(feature = "websocket")]

use std::process::Stdio;
use std::time::Duration;

use futures_util::SinkExt;
use personal_hopspot_core::node_pages;
use personal_rns::interfaces::websocket::{
    WebSocketFramingSelection, WebSocketWireFraming, WEBSOCKET_BITRATE_ESTIMATE,
};
use personal_rns::prelude::*;
use personal_rns::routing::links::request::{
    write_packed_binary_header, MAX_PACKED_BINARY_HEADER_LEN,
};
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::storage::GrowableHeap;
use personal_rns::websocket::WebSocketServerConnection;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio_tungstenite::{connect_async, tungstenite::Message};

async fn host(state: &std::path::Path) -> (Child, DestinationHash, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_personal-hopspot-headless"))
        .arg("--state-dir")
        .arg(state)
        .args([
            "--listen",
            "127.0.0.1:0",
            "--websocket-listen",
            "127.0.0.1:0",
            "--websocket-connections",
            "1",
            "--run-for",
            "30",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).await.unwrap();
    let destination = line.split("node_page=").nth(1).expect("host ready").trim();
    let mut bytes = [0; 16];
    hex::decode_to_slice(destination, &mut bytes).unwrap();
    loop {
        line.clear();
        assert_ne!(
            output.read_line(&mut line).await.unwrap(),
            0,
            "host stopped before WebSocket ready"
        );
        if line.starts_with("hopspot_websocket_ready listen=") {
            break;
        }
    }
    let address = line
        .strip_prefix("hopspot_websocket_ready listen=")
        .expect("WebSocket ready")
        .split_whitespace()
        .next()
        .unwrap();
    let url = format!("ws://{address}/");
    (child, DestinationHash::new(bytes), url)
}

#[tokio::test]
async fn websocket_page_transfer_and_connection_capacity_recovery() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let state = tempfile::tempdir().unwrap();
        let (_child, destination, url) = host(state.path()).await;
        let incomplete = tokio::net::TcpStream::connect(
            url.strip_prefix("ws://").unwrap().trim_end_matches('/'),
        )
        .await
        .unwrap();
        let opening = connect_async(&url);
        tokio::pin!(opening);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut opening)
                .await
                .is_err(),
            "incomplete handshakes also consume the admission budget"
        );
        drop(incomplete);
        let (mut first, _) = tokio::time::timeout(Duration::from_secs(2), opening)
            .await
            .unwrap()
            .unwrap();
        let waiting = connect_async(&url);
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut waiting)
                .await
                .is_err(),
            "occupied capacity must hold the next handshake"
        );
        first.send(Message::Close(None)).await.unwrap();
        drop(first);
        let (socket, _) = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap();
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: personal_rns::runtime::NoRemoteControlHostControls,
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            remote_control: personal_rns::remote_control::RemoteControlService::Unavailable.into(),
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: |_, _| {},
        });
        let handle = node.handle();
        handle.add_interface(WebSocketServerConnection::new(
            b"probe".to_vec(),
            socket,
            WEBSOCKET_BITRATE_ESTIMATE,
            WebSocketFramingSelection::Fixed(WebSocketWireFraming::RawPacket),
        ));
        let exercise = async {
            handle.request_path(destination).await.unwrap();
            let link = handle.establish_link(destination).await.unwrap();
            let (page, _) = handle
                .request(link, RequestPathHash::of(node_pages::INDEX_PATH), &[])
                .await
                .unwrap();
            let mut header = [0; MAX_PACKED_BINARY_HEADER_LEN];
            let length =
                write_packed_binary_header(node_pages::HOPSPOT_INDEX_PAGE.len(), &mut header)
                    .unwrap();
            let expected: Vec<_> = header[..length]
                .iter()
                .chain(node_pages::HOPSPOT_INDEX_PAGE)
                .copied()
                .collect();
            assert_eq!(page, expected);
        };
        tokio::select! {
            () = exercise => {},
            result = node.run() => panic!("probe stopped: {result:?}"),
        }
    })
    .await
    .expect("WebSocket qualification timed out");
}
