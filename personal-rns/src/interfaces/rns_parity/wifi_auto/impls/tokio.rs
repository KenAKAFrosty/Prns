use std::net::{SocketAddr, SocketAddrV6};
use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::engine::InstantMillis;
use crate::interfaces::rns_parity::wifi_auto::core;
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId};
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;

/// One confirmed peer on the WiFi/LAN `AutoInterface`, as a flat engine interface. Unlike
/// [`UdpInterface`](crate::interfaces::rns_parity::udp::impls::tokio::UdpInterface) it owns no
/// socket of its own: RNS addresses every peer at the one fixed data port, so the supervisor binds
/// the single shared data socket and demuxes inbound datagrams by source address, handing each
/// member the frames from its peer over the `inbound` channel. The member forwards the engine's
/// outbound frames to its peer's link-local `:42671` through that shared socket. Built by the
/// supervisor inside `fleet.add` once a beacon's peering token validates, never by the app directly
/// (the app attaches the supervisor, not a peer).
pub struct AutoWifiPeer {
    id: InterfaceId,
    socket: Arc<UdpSocket>,
    peer: SocketAddrV6,
    inbound: UnboundedReceiver<std::vec::Vec<u8>>,
    bitrate_bps: u32,
    status: TokioInterfaceStatus,
}

impl AutoWifiPeer {
    /// Wire up a member for `peer`, transmitting through the supervisor's shared `socket` and
    /// receiving the datagrams the supervisor demuxes to it over `inbound`. The `id` is minted by
    /// the supervisor (through `fleet.add`) so the engine can name this peer's interface; a member
    /// is born connected because the supervisor only stands it up after the peering ack.
    pub fn new(
        id: InterfaceId,
        socket: Arc<UdpSocket>,
        peer: SocketAddrV6,
        inbound: UnboundedReceiver<std::vec::Vec<u8>>,
        bitrate_bps: u32,
    ) -> Self {
        Self {
            id,
            socket,
            peer,
            inbound,
            bitrate_bps,
            status: TokioInterfaceStatus::new(id, ConnectionState::Connected),
        }
    }

    /// This member's id, minted by the supervisor that stood it up.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// A clone of this member's live-status handle for the app to read on its own render cadence.
    /// Call before [`run`](Interface::run) consumes the member.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl Interface for AutoWifiPeer {
    const HW_MTU: usize = core::WIFI_HW_MTU_CAP;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, self.bitrate_bps)
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        loop {
            tokio::select! {
                inbound = self.inbound.recv() => {
                    let Some(frame) = inbound else { return };
                    if frame.is_empty() {
                        continue;
                    }
                    self.status.add_rx(frame.len() as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_rx(now, frame.len() as u64);
                    self.status.set_transfer_rates(throughput.rates(now));
                    seam.next_inbound(&frame).await;
                }
                outbound = seam.next_outbound() => {
                    if outbound.is_empty() || outbound.len() > core::HARDWARE_MTU {
                        continue;
                    }
                    let Ok(sent) = self.socket.send_to(outbound, SocketAddr::V6(self.peer)).await
                    else {
                        continue;
                    };
                    self.status.add_tx(sent as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_tx(now, sent as u64);
                    self.status.set_transfer_rates(throughput.rates(now));
                    let frame_airtime = frame_airtime_us(sent, self.bitrate_bps);
                    self.status.set_airtime(airtime.record_tx(now, frame_airtime));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor::grant::{GrantConsumer, GrantProducer};
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use std::net::Ipv6Addr;
    use std::time::Duration;
    use tokio::sync::mpsc::{self, UnboundedSender};

    const TEST_FRAME_CAP: usize = 2_048;

    /// A hand-driven seam, as in the UDP tests: captures every `next_inbound`, supplies
    /// `next_outbound` from a grant lane the test fills.
    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        outbound: TokioGrantConsumer<TEST_FRAME_CAP>,
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

    fn v6(addr: SocketAddr) -> SocketAddrV6 {
        match addr {
            SocketAddr::V6(a) => a,
            SocketAddr::V4(_) => unreachable!("bound an IPv6 socket"),
        }
    }

    #[tokio::test]
    async fn frames_ride_the_shared_socket_out_and_the_demux_channel_in() {
        let loopback = SocketAddr::from((Ipv6Addr::LOCALHOST, 0));

        // The peer this member talks to: a plain socket the test owns, standing in for the far node.
        let peer = UdpSocket::bind(loopback)
            .await
            .expect("binds the test peer");
        let peer_addr = v6(peer.local_addr().expect("the peer address is known"));

        // The supervisor's single shared data socket, here owned by the test.
        let shared = Arc::new(
            UdpSocket::bind(loopback)
                .await
                .expect("binds the shared socket"),
        );
        let shared_addr = shared.local_addr().expect("the shared address is known");

        let (demux_tx, demux_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let member = AutoWifiPeer::new(
            InterfaceId::mint(),
            shared.clone(),
            peer_addr,
            demux_rx,
            core::WIFI_BITRATE_GUESS_BPS,
        );

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane::<TEST_FRAME_CAP>(2);
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };
        tokio::spawn(member.run(seam));

        // Inbound: the supervisor would demux a datagram from this peer to the member's channel; the
        // test plays the supervisor and pushes one directly. The member hands it up the seam whole.
        let payload = [0x7Eu8, 0x01, 0x7D, 0x02, 0x7E];
        demux_tx
            .send(payload.to_vec())
            .expect("the member is receiving");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the member hands the frame up within the window")
            .expect("the member task is alive");
        assert_eq!(received, payload, "raw bytes, exactly as demuxed");

        // Outbound: the seam yields a frame; it leaves the shared socket bound for the peer.
        let out_payload = [0xAAu8, 0x7E, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let mut buf = [0u8; 64];
        let (len, from) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
            .await
            .expect("the datagram arrives within the window")
            .expect("the test peer receives");
        assert_eq!(&buf[..len], out_payload, "raw bytes, exactly as granted");
        assert_eq!(
            from, shared_addr,
            "sent from the supervisor's shared data socket"
        );
    }
}
