#![allow(clippy::expect_used)]

use core::time::Duration;
use std::ffi::OsString;
use std::process::Stdio;

use personal_rns::interfaces::websocket::{
    WebSocketFramingSelection, FRAME_CAP, WEBSOCKET_BITRATE_ESTIMATE,
};
use personal_rns::interfaces::FrameSink;
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use personal_rns::manifold::tokio::{tokio_grant_lane, TokioGrantConsumer};
use prns_interfaces_tokio::websocket::WebSocketServerConnection;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_tungstenite::accept_async;

const PEER_TIMEOUT: Duration = Duration::from_secs(10);

struct HarnessSeam {
    sink: Vec<u8>,
    inbound: UnboundedSender<Vec<u8>>,
    outbound: TokioGrantConsumer,
}

impl InterfaceSeam for HarnessSeam {
    fn fill_random(&mut self, bytes: &mut [u8]) {
        bytes.fill(0);
    }

    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        &mut self.sink
    }

    async fn commit_inbound(&mut self) {
        if !self.sink.is_empty() {
            let _ = self.inbound.send(std::mem::take(&mut self.sink));
        }
    }

    async fn next_outbound(&mut self) -> &[u8] {
        self.outbound.release();
        self.outbound.peek().await.frame()
    }
}

fn packet(address: u8, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(19 + payload.len());
    packet.extend_from_slice(&[0, 0]);
    packet.extend_from_slice(&[address; 16]);
    packet.push(0);
    packet.extend_from_slice(payload);
    packet
}

fn required_argument(arguments: &mut impl Iterator<Item = OsString>, name: &str) -> OsString {
    arguments.next().unwrap_or_else(|| {
        eprintln!("missing {name} argument");
        std::process::exit(2);
    })
}

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let repository = required_argument(&mut arguments, "repository");
    let adapter = required_argument(&mut arguments, "adapter");
    let framing = required_argument(&mut arguments, "framing")
        .into_string()
        .unwrap_or_else(|_| {
            eprintln!("framing argument is not UTF-8");
            std::process::exit(2);
        });
    assert!(framing == "raw" || framing == "kiss");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the Prns harness binds");
    let address = listener.local_addr().expect("the listener has an address");
    let target = format!("ws://{address}/prns");

    let mut command = Command::new("node");
    command
        .arg(adapter)
        .arg(repository)
        .arg(target)
        .arg(&framing)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let peer = command.spawn().expect("the Bergie peer starts");

    let (stream, peer_address) = tokio::time::timeout(PEER_TIMEOUT, listener.accept())
        .await
        .expect("Bergie connects within the deadline")
        .expect("the Prns harness accepts Bergie");
    let socket = accept_async(stream)
        .await
        .expect("the WebSocket handshake completes");

    let first_packet = packet(0x31, b"provisional-raw");
    let evidence_packet = packet(0x32, b"bergie-evidence");
    let returned_packet = packet(0x33, b"resolved-egress");
    let (mut outbound, outbound_consumer) = tokio_grant_lane(FRAME_CAP, 2);
    outbound
        .try_grant()
        .expect("the first outbound slot is available")
        .fill(&first_packet);
    outbound.commit();
    let (inbound_sender, mut inbound) = mpsc::unbounded_channel();
    let seam = HarnessSeam {
        sink: Vec::new(),
        inbound: inbound_sender,
        outbound: outbound_consumer,
    };
    let connection = WebSocketServerConnection::new(
        peer_address.to_string().into_bytes(),
        socket,
        WEBSOCKET_BITRATE_ESTIMATE,
        WebSocketFramingSelection::Auto,
    );
    let connection_task = tokio::spawn(connection.run(seam));

    let received = tokio::time::timeout(PEER_TIMEOUT, inbound.recv())
        .await
        .expect("Bergie sends framing evidence within the deadline")
        .expect("the Prns connection remains alive");
    assert_eq!(received, evidence_packet);
    outbound
        .try_grant()
        .expect("the returned outbound slot is available")
        .fill(&returned_packet);
    outbound.commit();

    let output = tokio::time::timeout(PEER_TIMEOUT, peer.wait_with_output())
        .await
        .expect("the Bergie peer exits within the deadline")
        .expect("the Bergie peer output is collected");
    connection_task.abort();
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(1);
    }
    let stdout = String::from_utf8(output.stdout).expect("Bergie emits UTF-8");
    let expected = format!("PASS: bergie {framing} interoperated with Prns auto");
    assert_eq!(stdout.trim(), expected);
    println!("{expected}");
}
