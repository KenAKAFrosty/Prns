use std::io;
use std::net::{IpAddr, SocketAddr};

use tokio::net::UdpSocket;

use personal_rns::engine::InstantMillis;
use personal_rns::interfaces::udp::core;
use personal_rns::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use personal_rns::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::reactor::interface_seam::{Interface, InterfaceSeam};
use personal_rns::reactor::throughput::ThroughputLedger;

/// One end of an RNS UDP pair (`UDPInterface` parity): bind the local address up front —
/// so the bound port is readable and a bind refusal surfaces here — and forward every
/// outbound frame to the fixed `peer`, exactly as the reference forwards to its
/// configured `forward_ip:forward_port`. Datagrams arriving on the bound port are taken
/// as whole wire frames regardless of source (reference parity: its handler never
/// inspects the sender). Connectionless, so there is no reconnect machinery to mirror;
/// `bitrate_bps` is the host's claim about its pipe and sets the declared hardware MTU
/// through the reference's tier table, datagram-clamped.
pub struct UdpInterface {
    id: InterfaceId,
    socket: UdpSocket,
    peer: SocketAddr,
    bitrate_bps: u32,
    channel_tag: heapless::Vec<u8, 18>,
    status: TokioInterfaceStatus,
}

/// The bytes that tag this UDP channel: the fixed forward target it speaks to (RNS forwards every
/// datagram to one `forward_ip:forward_port`), the same target across a reconnect.
fn udp_channel_tag(peer: SocketAddr) -> heapless::Vec<u8, 18> {
    let mut tag = heapless::Vec::new();
    match peer.ip() {
        IpAddr::V4(v4) => {
            let _ = tag.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            let _ = tag.extend_from_slice(&v6.octets());
        }
    }
    let _ = tag.extend_from_slice(&peer.port().to_be_bytes());
    tag
}

impl UdpInterface {
    pub async fn bind(
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
        bitrate_bps: u32,
    ) -> io::Result<Self> {
        let (socket, peer) = Self::bind_socket(local, peer).await?;
        Ok(Self::assemble(None, socket, peer, bitrate_bps))
    }

    /// Bind with a caller-chosen id instead of one derived from the forward target — for advanced
    /// setups that drive the reactor by hand and must pin the interface to a routing key their own
    /// wiring references. Ordinary nodes call [`bind`](Self::bind).
    pub async fn bind_with_id(
        id: InterfaceId,
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
        bitrate_bps: u32,
    ) -> io::Result<Self> {
        let (socket, peer) = Self::bind_socket(local, peer).await?;
        Ok(Self::assemble(Some(id), socket, peer, bitrate_bps))
    }

    async fn bind_socket(
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
    ) -> io::Result<(UdpSocket, SocketAddr)> {
        let socket = UdpSocket::bind(local).await?;
        let peer = tokio::net::lookup_host(peer)
            .await?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "peer resolved to nothing"))?;
        Ok((socket, peer))
    }

    fn assemble(
        id_override: Option<InterfaceId>,
        socket: UdpSocket,
        peer: SocketAddr,
        bitrate_bps: u32,
    ) -> Self {
        let channel_tag = udp_channel_tag(peer);
        let id = id_override
            .unwrap_or_else(|| InterfaceId::from_channel_tag(InterfaceKind::Udp, &channel_tag));
        Self {
            id,
            socket,
            peer,
            bitrate_bps,
            channel_tag,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        }
    }

    /// This interface's id: derived from its forward target by [`bind`](Self::bind), or the one
    /// handed to [`bind_with_id`](Self::bind_with_id). For the app that wants to name it (an
    /// [`AnnounceTarget::Interface`](personal_rns::engine::AnnounceTarget), a log line).
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// A clone of this interface's live-status handle for the app to read on its own render
    /// cadence. Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl Interface for UdpInterface {
    const HW_MTU: usize = core::UDP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::Udp;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, self.bitrate_bps)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut recv_buf = std::vec![0u8; core::RECV_BUF_LEN].into_boxed_slice();
        self.status.set_connection(ConnectionState::Connected);
        loop {
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
                    seam.next_inbound(&recv_buf[..len]).await;
                }
                outbound = seam.next_outbound() => {
                    if outbound.is_empty() || outbound.len() > core::UDP_DATAGRAM_MAX {
                        continue;
                    }
                    let Ok(sent) = self.socket.send_to(outbound, self.peer).await else {
                        continue;
                    };
                    self.status.add_tx(sent as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_tx(now, sent as u64);
                    self.status.set_transfer_rates(throughput.rates());
                    let frame_airtime = frame_airtime_us(sent, self.bitrate_bps);
                    self.status.set_airtime(airtime.record_tx(now, frame_airtime));
                }
            }
        }
    }
}

impl personal_rns::interfaces::ReportsStatus for UdpInterface {
    fn status_view(&self) -> Option<personal_rns::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![personal_rns::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use std::time::Duration;
    use tokio::sync::mpsc::{self, UnboundedSender};

    const TEST_FRAME_CAP: usize = 2_048;

    /// A hand-driven seam, as in the TCP tests: captures every `next_inbound`, supplies
    /// `next_outbound` from a grant lane the test fills.
    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        outbound: TokioGrantConsumer,
    }

    impl InterfaceSeam for MockSeam {
        async fn next_inbound(&mut self, frame: &[u8]) {
            let _ = self.inbound.send(frame.to_vec());
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

        let interface = UdpInterface::bind("127.0.0.1:0", far_addr, core::UDP_BITRATE_GUESS_BPS)
            .await
            .expect("binds an ephemeral local port");
        let near_addr = interface.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };
        tokio::spawn(interface.run(seam));

        // Inbound: one datagram is one frame, byte-for-byte — no framing to strip.
        let payload = [0x7Eu8, 0x01, 0x7D, 0x02, 0x7E];
        far.send_to(&payload, near_addr)
            .await
            .expect("the test peer transmits");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface hands the datagram up within the window")
            .expect("the interface task is alive");
        assert_eq!(received, payload, "raw bytes, exactly as sent");

        // Outbound: the seam yields a frame; it arrives at the fixed peer as one datagram.
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
}
