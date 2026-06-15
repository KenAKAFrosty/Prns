use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::Arc;
use std::time::Duration;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::engine::InstantMillis;
use crate::interfaces::rns_parity::wifi_auto::core;
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId};
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;
use crate::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

const MAX_PEERS: usize = 8;
const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
const UNICAST_REPEER_EVERY: u32 = 3;

pub struct AutoWifiPeer {
    id: InterfaceId,
    socket: Arc<UdpSocket>,
    peer: SocketAddrV6,
    inbound: UnboundedReceiver<std::vec::Vec<u8>>,
    bitrate_bps: u32,
    status: TokioInterfaceStatus,
}

impl AutoWifiPeer {
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

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }
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

/// The RNS WiFi/LAN `AutoInterface` as an [`InterfaceSupervisor`]: it owns the multicast discovery
/// sockets and the single shared data socket, beacons its peering token, validates inbound beacons,
/// and stands up an [`AutoWifiPeer`] member per confirmed peer through the [`Fleet`] it is handed,
/// demuxing inbound data datagrams back to each member by source. Attach with
/// `handle.supervise(AutoWifi::new())`.
pub struct AutoWifi {
    bitrate_bps: u32,
}

impl AutoWifi {
    #[must_use]
    pub fn new() -> Self {
        Self {
            bitrate_bps: core::WIFI_BITRATE_GUESS_BPS,
        }
    }

    /// Declare a known pipe instead of RNS's 10 Mbps guess; sets the members' announce pacing.
    #[must_use]
    pub fn with_bitrate(bitrate_bps: u32) -> Self {
        Self { bitrate_bps }
    }
}

impl Default for AutoWifi {
    fn default() -> Self {
        Self::new()
    }
}

impl InterfaceSupervisor for AutoWifi {
    async fn run(self, fleet: Fleet) {
        let Some(nic) = link_local_nic() else { return };
        let Ok(Sockets {
            discovery,
            unicast_discovery,
            data,
        }) = open_sockets(nic.index)
        else {
            return;
        };

        let mut sup = Supervisor {
            brain: core::AutoInterfaceProtocol::from_link_local(nic.link_local),
            members: HashMap::new(),
            fleet,
            data: data.clone(),
            index: nic.index,
            bitrate_bps: self.bitrate_bps,
        };
        let token = *sup.brain.our_peering_token().as_bytes();

        let started = tokio::time::Instant::now();
        let mut beacon = tokio::time::interval(BEACON_INTERVAL);
        let mut beacon_cycle: u32 = 0;
        let mut discovery_buf = [0u8; 64];
        let mut unicast_buf = [0u8; 64];
        let mut data_buf = [0u8; core::HARDWARE_MTU];

        loop {
            tokio::select! {
                received = discovery.recv_from(&mut discovery_buf) => {
                    if let Ok((len, src)) = received {
                        let now_ms = started.elapsed().as_millis() as u64;
                        sup.ingest_beacon(src, &discovery_buf[..len], now_ms);
                    }
                }
                received = unicast_discovery.recv_from(&mut unicast_buf) => {
                    if let Ok((len, src)) = received {
                        let now_ms = started.elapsed().as_millis() as u64;
                        sup.ingest_beacon(src, &unicast_buf[..len], now_ms);
                    }
                }
                received = data.recv_from(&mut data_buf) => {
                    if let Ok((len, src)) = received {
                        sup.route_inbound(src, &data_buf[..len]);
                    }
                }
                _ = beacon.tick() => {
                    beacon_cycle = beacon_cycle.wrapping_add(1);
                    let _ = discovery
                        .send_to(&token, scoped(core::DISCOVERY_GROUP, core::DEFAULT_DISCOVERY_PORT, nic.index))
                        .await;
                    if beacon_cycle.is_multiple_of(UNICAST_REPEER_EVERY) {
                        for addr in sup.peer_addresses() {
                            let _ = unicast_discovery
                                .send_to(&token, scoped(addr, core::UNICAST_DISCOVERY_PORT, nic.index))
                                .await;
                        }
                    }
                    let now_ms = started.elapsed().as_millis() as u64;
                    sup.retire_stale(now_ms);
                }
            }
        }
    }
}

struct PeerMember {
    attached: AttachedInterface,
    inbound: UnboundedSender<std::vec::Vec<u8>>,
}

struct Supervisor {
    brain: core::AutoInterfaceProtocol<MAX_PEERS>,
    members: HashMap<Ipv6Addr, PeerMember>,
    fleet: Fleet,
    data: Arc<UdpSocket>,
    index: u32,
    bitrate_bps: u32,
}

impl Supervisor {
    fn ingest_beacon(&mut self, src: SocketAddr, bytes: &[u8], now_ms: u64) {
        let SocketAddr::V6(v6) = src else { return };
        if let core::BeaconVerdict::Peer(addr) =
            self.brain
                .ingest_discovery_datagram(*v6.ip(), bytes, now_ms)
        {
            if !self.members.contains_key(&addr) {
                self.spawn_member(addr);
            }
        }
    }

    fn spawn_member(&mut self, addr: Ipv6Addr) {
        let id = InterfaceId::mint();
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let peer = SocketAddrV6::new(addr, core::DEFAULT_DATA_PORT, 0, self.index);
        let member = AutoWifiPeer::new(id, self.data.clone(), peer, inbound_rx, self.bitrate_bps);
        let attached = self.fleet.add(member);
        self.members.insert(
            addr,
            PeerMember {
                attached,
                inbound: inbound_tx,
            },
        );
    }

    fn route_inbound(&self, src: SocketAddr, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let SocketAddr::V6(v6) = src else { return };
        if let Some(member) = self.members.get(v6.ip()) {
            let _ = member.inbound.send(bytes.to_vec());
        }
    }

    fn peer_addresses(&self) -> std::vec::Vec<Ipv6Addr> {
        self.brain.known_peer_addresses().collect()
    }

    fn retire_stale(&mut self, now_ms: u64) {
        if self.brain.prune_stale_peers(now_ms) == 0 {
            return;
        }
        let live: HashSet<Ipv6Addr> = self.brain.known_peer_addresses().collect();
        let gone: std::vec::Vec<Ipv6Addr> = self
            .members
            .keys()
            .filter(|addr| !live.contains(*addr))
            .copied()
            .collect();
        for addr in gone {
            if let Some(member) = self.members.remove(&addr) {
                member.attached.teardown();
            }
        }
    }
}

struct Nic {
    link_local: Ipv6Addr,
    index: u32,
}

struct Sockets {
    discovery: UdpSocket,
    unicast_discovery: UdpSocket,
    data: Arc<UdpSocket>,
}

fn link_local_nic() -> Option<Nic> {
    for iface in if_addrs::get_if_addrs().ok()? {
        if iface.is_loopback() || !matches!(iface.addr, if_addrs::IfAddr::V6(_)) {
            continue;
        }
        let Some(index) = iface.index else { continue };
        if let Some(link_local) = link_local_for_scope(index) {
            return Some(Nic { link_local, index });
        }
    }
    None
}

fn link_local_for_scope(index: u32) -> Option<Ipv6Addr> {
    let probe =
        std::net::UdpSocket::bind(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0)).ok()?;
    let target = SocketAddrV6::new(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1), 9999, 0, index);
    probe.connect(SocketAddr::V6(target)).ok()?;
    let SocketAddr::V6(local) = probe.local_addr().ok()? else {
        return None;
    };
    let link_local = *local.ip();
    ((link_local.segments()[0] & 0xffc0) == 0xfe80).then_some(link_local)
}

fn open_sockets(index: u32) -> io::Result<Sockets> {
    Ok(Sockets {
        discovery: discovery_socket(index)?,
        unicast_discovery: bound_v6(core::UNICAST_DISCOVERY_PORT)?,
        data: Arc::new(bound_v6(core::DEFAULT_DATA_PORT)?),
    })
}

fn discovery_socket(index: u32) -> io::Result<UdpSocket> {
    let socket = reusable_socket2(core::DEFAULT_DISCOVERY_PORT)?;
    socket.set_multicast_if_v6(index)?;
    socket.join_multicast_v6(&core::DISCOVERY_GROUP, index)?;
    into_tokio(socket)
}

fn bound_v6(port: u16) -> io::Result<UdpSocket> {
    into_tokio(reusable_socket2(port)?)
}

fn reusable_socket2(port: u16) -> io::Result<Socket> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_only_v6(true)?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SockAddr::from(SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::UNSPECIFIED,
        port,
        0,
        0,
    ))))?;
    Ok(socket)
}

fn into_tokio(socket: Socket) -> io::Result<UdpSocket> {
    let socket: std::net::UdpSocket = socket.into();
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket)
}

fn scoped(addr: Ipv6Addr, port: u16, index: u32) -> SocketAddr {
    SocketAddr::V6(SocketAddrV6::new(addr, port, 0, index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor::grant::{GrantConsumer, GrantProducer};
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};

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
