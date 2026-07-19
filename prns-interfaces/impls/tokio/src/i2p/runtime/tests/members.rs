use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc;

use prns_core::interfaces::i2p::core;
use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use prns_core::interfaces::{
    ConfiguredInterfacePolicy, ConnectionState, FrameSink, InterfaceStatus,
};
use prns_runtime::reactor::driver::{tokio_grant_lane, TokioGrantConsumer};
use prns_runtime::reactor::interface_seam::{Interface, InterfaceSeam};

use super::super::super::{I2pPeerAddress, I2pRetryPolicy};
use super::super::member::{I2pAcceptedPeer, I2pConfiguredPeer, I2pMemberEvent};
use super::fake_sam::{public_destination, FakeSamBridge, FakeSamError};

struct MockSeam {
    inbound: mpsc::UnboundedSender<Vec<u8>>,
    sink: Vec<u8>,
    outbound: TokioGrantConsumer,
    tunnel_requests: Arc<AtomicUsize>,
}

impl InterfaceSeam for MockSeam {
    fn fill_entropy(&mut self, bytes: &mut [u8]) {
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

    fn try_next_outbound(&mut self) -> Option<&[u8]> {
        self.outbound.release();
        Some(self.outbound.try_peek()?.frame())
    }

    async fn request_tunnel_synthesis(&mut self) {
        self.tunnel_requests.fetch_add(1, Ordering::Relaxed);
    }
}

fn retry_policy() -> I2pRetryPolicy {
    I2pRetryPolicy::new(
        Duration::from_millis(1),
        Duration::from_millis(1),
        Duration::from_millis(1),
    )
    .expect("the test retry policy is non-zero")
}

fn mock_seam() -> (
    MockSeam,
    prns_runtime::reactor::driver::TokioGrantProducer,
    mpsc::UnboundedReceiver<Vec<u8>>,
    Arc<AtomicUsize>,
) {
    let (outbound, consumer) = tokio_grant_lane(core::FRAME_LEN, 8);
    let (inbound, inbound_rx) = mpsc::unbounded_channel();
    let tunnel_requests = Arc::new(AtomicUsize::new(0));
    (
        MockSeam {
            inbound,
            sink: Vec::new(),
            outbound: consumer,
            tunnel_requests: tunnel_requests.clone(),
        },
        outbound,
        inbound_rx,
        tunnel_requests,
    )
}

async fn write_framed(stream: &mut DuplexStream, payload: &[u8]) {
    let mut framed = vec![0u8; rns_serial_framing::max_encoded_len(payload.len())];
    let encoded = rns_serial_framing::encode(payload, &mut framed).expect("the payload frames");
    stream
        .write_all(&framed[..encoded])
        .await
        .expect("the fake peer writes the frame");
}

async fn read_deframed(stream: &mut DuplexStream) -> Vec<u8> {
    let mut decoder = RnsSerialDecoder::<{ core::FRAME_LEN }>::new();
    let mut buffer = [0u8; 512];
    loop {
        let read = stream
            .read(&mut buffer)
            .await
            .expect("the fake peer reads the wire");
        assert_ne!(read, 0);
        for byte in &buffer[..read] {
            if let Ok(Some(frame)) = decoder.feed(*byte) {
                if !frame.is_empty() {
                    return frame.to_vec();
                }
            }
        }
    }
}

async fn next_connected_stream(bridge: &FakeSamBridge) -> DuplexStream {
    tokio::time::timeout(Duration::from_secs(1), bridge.next_connected_stream())
        .await
        .expect("the fake connection attempt finishes")
}

#[tokio::test]
async fn configured_peer_frames_both_directions_and_synthesizes_a_tunnel() {
    let bridge = FakeSamBridge::new();
    let peer = I2pPeerAddress::new(public_destination(0x73).as_str())
        .expect("the peer destination is valid");
    let policy = core::configured_policy(ConfiguredInterfacePolicy::default());
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let member = I2pConfiguredPeer::new(bridge.clone(), peer, policy, retry_policy(), events_tx);
    let id = member.id();
    let status = member.status();
    let (seam, mut outbound, mut inbound, tunnel_requests) = mock_seam();
    let task = tokio::spawn(async move { member.run(seam).await });
    let mut remote = next_connected_stream(&bridge).await;
    let event = events_rx
        .recv()
        .await
        .expect("the initial attempt is reported");

    assert!(matches!(event, I2pMemberEvent::InitialAttempt(event_id) if event_id == id));
    tokio::task::yield_now().await;
    assert_eq!(status.connection(), ConnectionState::Connected);
    assert_eq!(tunnel_requests.load(Ordering::Relaxed), 1);

    write_framed(&mut remote, b"from-i2p").await;
    assert_eq!(
        inbound
            .recv()
            .await
            .expect("the inbound frame is delivered"),
        b"from-i2p"
    );

    outbound
        .try_grant()
        .expect("the outbound lane has capacity")
        .fill(b"to-i2p");
    outbound.commit();
    assert_eq!(read_deframed(&mut remote).await, b"to-i2p");

    drop(remote);
    let _reconnected = next_connected_stream(&bridge).await;
    assert_eq!(bridge.session_attempts(), 1);
    assert_eq!(bridge.connect_attempts(), 2);

    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn failed_stream_connect_resets_the_reference_client_tunnel() {
    let bridge = FakeSamBridge::new();
    bridge.queue_connect_result(Err(FakeSamError::SessionLost));
    let peer = I2pPeerAddress::new(public_destination(0x75).as_str())
        .expect("the peer destination is valid");
    let policy = core::configured_policy(ConfiguredInterfacePolicy::default());
    let (events_tx, _events_rx) = mpsc::unbounded_channel();
    let member = I2pConfiguredPeer::new(bridge.clone(), peer, policy, retry_policy(), events_tx);
    let (seam, _outbound, _inbound, _tunnel_requests) = mock_seam();
    let task = tokio::spawn(async move { member.run(seam).await });

    let _connected = next_connected_stream(&bridge).await;
    assert_eq!(bridge.session_attempts(), 2);
    assert_eq!(bridge.connect_attempts(), 2);

    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn unavailable_sam_bridge_retries_transient_session_setup() {
    let bridge = FakeSamBridge::new();
    bridge.queue_session_result(Err(FakeSamError::BridgeUnavailable));
    let peer = I2pPeerAddress::new(public_destination(0x76).as_str())
        .expect("the peer destination is valid");
    let policy = core::configured_policy(ConfiguredInterfacePolicy::default());
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let member = I2pConfiguredPeer::new(bridge.clone(), peer, policy, retry_policy(), events_tx);
    let id = member.id();
    let (seam, _outbound, _inbound, _tunnel_requests) = mock_seam();
    let task = tokio::spawn(async move { member.run(seam).await });

    let event = events_rx
        .recv()
        .await
        .expect("the failed initial attempt is reported");
    let _connected = next_connected_stream(&bridge).await;
    assert!(matches!(event, I2pMemberEvent::InitialAttempt(event_id) if event_id == id));
    assert_eq!(bridge.session_attempts(), 2);
    assert_eq!(bridge.connect_attempts(), 1);

    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn peer_reachability_failures_also_reset_the_reference_client_tunnel() {
    let bridge = FakeSamBridge::new();
    bridge.queue_connect_result(Err(FakeSamError::PeerUnreachable));
    let peer = I2pPeerAddress::new("named-peer.i2p").expect("the peer name is valid");
    let policy = core::configured_policy(ConfiguredInterfacePolicy::default());
    let (events_tx, _events_rx) = mpsc::unbounded_channel();
    let member = I2pConfiguredPeer::new(bridge.clone(), peer, policy, retry_policy(), events_tx);
    let (seam, _outbound, _inbound, _tunnel_requests) = mock_seam();
    let task = tokio::spawn(async move { member.run(seam).await });

    let _connected = next_connected_stream(&bridge).await;
    assert_eq!(bridge.session_attempts(), 2);
    assert_eq!(bridge.connect_attempts(), 2);
    assert_eq!(
        bridge.resolved_names(),
        vec![
            String::from("named-peer.i2p"),
            String::from("named-peer.i2p")
        ]
    );

    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn accepted_peers_do_not_synthesize_tunnels_and_report_clean_close() {
    let policy = core::configured_policy(ConfiguredInterfacePolicy::default());
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (local, mut remote) = tokio::io::duplex(64 * 1024);
    let member = I2pAcceptedPeer::new(
        public_destination(0x79),
        1,
        tokio::io::BufReader::new(local),
        policy,
        events_tx,
    );
    let id = member.id();
    let (seam, _outbound, mut inbound, tunnel_requests) = mock_seam();
    let task = tokio::spawn(async move { member.run(seam).await });

    write_framed(&mut remote, b"accepted-inbound").await;
    assert_eq!(
        inbound
            .recv()
            .await
            .expect("the inbound frame is delivered"),
        b"accepted-inbound"
    );
    assert_eq!(tunnel_requests.load(Ordering::Relaxed), 0);

    drop(remote);
    let event = tokio::time::timeout(Duration::from_secs(1), events_rx.recv())
        .await
        .expect("the accepted member closes")
        .expect("the close event is delivered");
    assert!(matches!(event, I2pMemberEvent::Closed(event_id) if event_id == id));
    task.await.expect("the accepted member exits cleanly");
}
