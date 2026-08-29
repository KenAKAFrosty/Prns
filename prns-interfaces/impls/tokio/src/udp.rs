use std::io;
use std::net::{IpAddr, SocketAddr};

use tokio::net::UdpSocket;

use prns_core::engine::InstantMillis;
use prns_core::interfaces::udp;
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

pub struct UdpInterface {
    id: InterfaceId,
    socket: UdpSocket,
    flow: UdpSocketFlow,
    policy: EffectiveInterfacePolicy,
    channel_tag: heapless::Vec<u8, 19>,
    status: TokioInterfaceStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UdpSocketFlow {
    ReceiveOnly,
    SendOnly { peer: SocketAddr },
    Bidirectional { peer: SocketAddr },
}

fn udp_channel_tag(flow: UdpSocketFlow, local: SocketAddr) -> heapless::Vec<u8, 19> {
    let mut tag = heapless::Vec::new();
    let (kind, endpoint) = match flow {
        UdpSocketFlow::ReceiveOnly => (0, local),
        UdpSocketFlow::SendOnly { peer } => (1, peer),
        UdpSocketFlow::Bidirectional { peer } => (2, peer),
    };
    let _ = tag.push(kind);
    match endpoint.ip() {
        IpAddr::V4(v4) => {
            let _ = tag.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            let _ = tag.extend_from_slice(&v6.octets());
        }
    }
    let _ = tag.extend_from_slice(&endpoint.port().to_be_bytes());
    tag
}

impl UdpInterface {
    pub async fn bind(
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
        bitrate: BitrateBps,
    ) -> io::Result<Self> {
        Self::bind_with_policy(local, peer, udp::policy_for_bitrate(bitrate)).await
    }

    pub async fn bind_with_policy(
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
    ) -> io::Result<Self> {
        let (socket, peer) = Self::bind_peer_socket(local, peer).await?;
        Self::assemble(None, socket, UdpSocketFlow::Bidirectional { peer }, policy)
    }

    pub async fn bind_receive_with_policy(
        local: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
    ) -> io::Result<Self> {
        let socket = Self::bind_socket(local).await?;
        Self::assemble(None, socket, UdpSocketFlow::ReceiveOnly, policy)
    }

    pub async fn bind_send_with_policy(
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
    ) -> io::Result<Self> {
        let (socket, peer) = Self::bind_peer_socket(local, peer).await?;
        Self::assemble(None, socket, UdpSocketFlow::SendOnly { peer }, policy)
    }

    pub async fn bind_with_id(
        id: InterfaceId,
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
        bitrate: BitrateBps,
    ) -> io::Result<Self> {
        Self::bind_with_id_and_policy(id, local, peer, udp::policy_for_bitrate(bitrate)).await
    }

    pub async fn bind_with_id_and_policy(
        id: InterfaceId,
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
    ) -> io::Result<Self> {
        let (socket, peer) = Self::bind_peer_socket(local, peer).await?;
        Self::assemble(
            Some(id),
            socket,
            UdpSocketFlow::Bidirectional { peer },
            policy,
        )
    }

    async fn bind_peer_socket(
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
    ) -> io::Result<(UdpSocket, SocketAddr)> {
        let socket = Self::bind_socket(local).await?;
        let peer = tokio::net::lookup_host(peer)
            .await?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "peer resolved to nothing"))?;
        Ok((socket, peer))
    }

    async fn bind_socket(local: impl tokio::net::ToSocketAddrs) -> io::Result<UdpSocket> {
        let socket = UdpSocket::bind(local).await?;
        socket.set_broadcast(true)?;
        Ok(socket)
    }

    fn assemble(
        id_override: Option<InterfaceId>,
        socket: UdpSocket,
        flow: UdpSocketFlow,
        policy: EffectiveInterfacePolicy,
    ) -> io::Result<Self> {
        let channel_tag = udp_channel_tag(flow, socket.local_addr()?);
        let id = id_override
            .unwrap_or_else(|| InterfaceId::from_channel_tag(InterfaceKind::Udp, &channel_tag));
        Ok(Self {
            id,
            socket,
            flow,
            policy,
            channel_tag,
            status: TokioInterfaceStatus::new_accounted(id, ConnectionState::Initializing),
        })
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl Interface for UdpInterface {
    const HW_MTU: usize = udp::UDP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::Udp;

    fn descriptor(&self) -> InterfaceDescriptor {
        udp::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut recv_buf = std::vec![0u8; udp::RECV_BUF_LEN].into_boxed_slice();
        self.status.set_connection(ConnectionState::Connected);
        match self.flow {
            UdpSocketFlow::ReceiveOnly => loop {
                let Ok((len, _)) = self.socket.recv_from(&mut recv_buf).await else {
                    continue;
                };
                if len == 0 {
                    continue;
                }
                self.status.add_rx(len as u64);
                let now = InstantMillis(started.elapsed().as_millis() as u64);
                throughput.record_rx(now, len as u64);
                self.status.set_transfer_rates(throughput.rates());
                self.status.count_frame_in();
                seam.next_inbound(&recv_buf[..len]).await;
                self.status.count_frame_delivered();
            },
            UdpSocketFlow::SendOnly { peer } => loop {
                let outbound = seam.next_outbound().await;
                if outbound.is_empty() || outbound.len() > udp::UDP_DATAGRAM_MAX {
                    continue;
                }
                let Ok(sent) = self.socket.send_to(outbound, peer).await else {
                    continue;
                };
                self.status.add_tx(sent as u64);
                let now = InstantMillis(started.elapsed().as_millis() as u64);
                throughput.record_tx(now, sent as u64);
                self.status.set_transfer_rates(throughput.rates());
                let frame_airtime = frame_airtime_us(sent, self.policy.bitrate);
                self.status
                    .set_airtime(airtime.record_tx(now, frame_airtime));
            },
            UdpSocketFlow::Bidirectional { peer } => loop {
                tokio::select! {
                    received = self.socket.recv_from(&mut recv_buf) => {
                        let Ok((len, _)) = received else { continue };
                        if len == 0 {
                            continue;
                        }
                        self.status.add_rx(len as u64);
                        let now = InstantMillis(started.elapsed().as_millis() as u64);
                        throughput.record_rx(now, len as u64);
                        self.status.set_transfer_rates(throughput.rates());
                        self.status.count_frame_in();
                        seam.next_inbound(&recv_buf[..len]).await;
                        self.status.count_frame_delivered();
                    }
                    outbound = seam.next_outbound() => {
                        if outbound.is_empty() || outbound.len() > udp::UDP_DATAGRAM_MAX {
                            continue;
                        }
                        let Ok(sent) = self.socket.send_to(outbound, peer).await else {
                            continue;
                        };
                        self.status.add_tx(sent as u64);
                        let now = InstantMillis(started.elapsed().as_millis() as u64);
                        throughput.record_tx(now, sent as u64);
                        self.status.set_transfer_rates(throughput.rates());
                        let frame_airtime = frame_airtime_us(sent, self.policy.bitrate);
                        self.status.set_airtime(airtime.record_tx(now, frame_airtime));
                    }
                }
            },
        }
    }
}

impl prns_core::interfaces::ReportsStatus for UdpInterface {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }

    fn frame_accounting_recorder(&self) -> Option<prns_core::interfaces::FrameAccountingRecorder> {
        prns_core::interfaces::FrameAccountingRecorder::of(self.status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
    use std::time::Duration;
    use tokio::sync::mpsc::{self, UnboundedSender};

    const TEST_FRAME_CAP: usize = 2_048;

    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        sink: std::vec::Vec<u8>,
        outbound: TokioGrantConsumer,
    }

    use prns_core::interfaces::FrameSink;

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
    }

    #[tokio::test]
    async fn datagrams_cross_unframed_in_both_directions_over_real_sockets() {
        let far = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("binds the test peer");
        let far_addr = far.local_addr().expect("the peer address is known");

        let interface = UdpInterface::bind("127.0.0.1:0", far_addr, udp::UDP_BITRATE_ESTIMATE)
            .await
            .expect("binds an ephemeral local port");
        let status = interface.status();
        let near_addr = interface.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };
        tokio::spawn(interface.run(seam));

        let payload = [0x7Eu8, 0x01, 0x7D, 0x02, 0x7E];
        far.send_to(&[], near_addr)
            .await
            .expect("the test peer transmits an empty datagram");
        far.send_to(&payload, near_addr)
            .await
            .expect("the test peer transmits");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface hands the datagram up within the window")
            .expect("the interface task is alive");
        assert_eq!(received, payload, "raw bytes, exactly as sent");
        assert_eq!(
            prns_core::interfaces::InterfaceStatus::frame_accounting(&status),
            Some(prns_core::interfaces::FrameAccounting {
                frames_in: 1,
                malformed: 0,
                protocol_violations: 0,
                undecodable: 0,
                delivered: 1,
            }),
        );

        let out_payload = [0xAAu8, 0x7E, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let mut buf = [0u8; 64];
        let (len, from) = tokio::time::timeout(Duration::from_secs(2), far.recv_from(&mut buf))
            .await
            .expect("the datagram arrives within the window")
            .expect("the test peer receives");
        assert_eq!(&buf[..len], out_payload, "raw bytes, exactly as granted");
        assert_eq!(from, near_addr, "sent from the interface's bound port");
    }

    #[tokio::test]
    async fn receive_only_never_requires_a_forward_target() {
        let interface = UdpInterface::bind_receive_with_policy(
            "127.0.0.1:0",
            udp::policy_for_bitrate(udp::UDP_BITRATE_ESTIMATE),
        )
        .await
        .expect("binds a receive-only socket");
        let near_addr = interface.local_addr().expect("the bound address is known");
        let far = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("binds the sender");
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (_out_tx, out_rx) = tokio_grant_lane(TEST_FRAME_CAP, 1);
        tokio::spawn(interface.run(MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        }));

        far.send_to(b"receive-only", near_addr)
            .await
            .expect("sends to the receive-only socket");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the frame arrives")
            .expect("the interface task is alive");
        assert_eq!(received, b"receive-only");
    }

    #[tokio::test]
    async fn send_only_uses_an_ephemeral_bind_and_never_requires_ingress() {
        let far = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("binds the receiver");
        let far_addr = far.local_addr().expect("the receiver address is known");
        let interface = UdpInterface::bind_send_with_policy(
            "127.0.0.1:0",
            far_addr,
            udp::policy_for_bitrate(udp::UDP_BITRATE_ESTIMATE),
        )
        .await
        .expect("binds a send-only socket");
        assert_ne!(
            interface.local_addr().expect("the bind is visible").port(),
            0
        );
        let (in_tx, _in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(TEST_FRAME_CAP, 1);
        tokio::spawn(interface.run(MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        }));

        out_tx
            .try_grant()
            .expect("the outbound lane has room")
            .fill(b"send-only");
        out_tx.commit();
        let mut received = [0u8; 32];
        let (len, _) = tokio::time::timeout(Duration::from_secs(2), far.recv_from(&mut received))
            .await
            .expect("the frame leaves")
            .expect("the receiver reads it");
        assert_eq!(&received[..len], b"send-only");
    }
}
