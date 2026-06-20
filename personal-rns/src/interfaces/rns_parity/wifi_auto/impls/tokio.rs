use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::engine::InstantMillis;
use crate::interfaces::rns_parity::tcp::client::tokio::TcpClientInterface;
use crate::interfaces::rns_parity::tcp::server::tokio::TcpServerConnection;
use crate::interfaces::rns_parity::tcp::tokio_socket::tune;
use crate::interfaces::rns_parity::wifi_auto::core;
use crate::interfaces::{
    ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind, InterfaceStatus, TransferRates,
};
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;
use crate::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

const MAX_PEERS: usize = 8;
const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
const UNICAST_REPEER_EVERY: u32 = 3;
/// Consecutive failed discovery beacons before the supervisor reports its egress as down. Two
/// intervals of [`BEACON_INTERVAL`] so a single transient send error does not flap the card.
const EGRESS_DOWN_AFTER: u32 = 2;
/// How long a gateway rendezvous link waits before redialing after a failed or dropped connection.
/// Generous, because on an ordinary network the gateway is no Prns host and this dial never succeeds
/// — it must not busy-loop — while an isolating hotspot's host comes up well within one interval.
const GATEWAY_REDIAL: Duration = Duration::from_secs(10);
/// Beacon cycles between attempts to reclaim the rendezvous port when another local node holds it.
/// Three cycles of [`BEACON_INTERVAL`] (~5s) — brisk enough to take over promptly once the holder
/// exits and frees the port, without calling bind() every beacon.
const REBIND_BEACON_CYCLES: u32 = 3;

pub struct AutoWifiPeer {
    id: InterfaceId,
    socket: Arc<UdpSocket>,
    peer: SocketAddrV6,
    inbound: UnboundedReceiver<std::vec::Vec<u8>>,
    bitrate_bps: u32,
    channel_tag: [u8; 16],
    status: TokioInterfaceStatus,
}

impl AutoWifiPeer {
    /// The member's id is derived from its peer's link-local address — EUI-64-stable to the peer's
    /// MAC, so the same device reconnecting (even on a fresh socket) rebinds the routes it owned.
    pub fn new(
        socket: Arc<UdpSocket>,
        peer: SocketAddrV6,
        inbound: UnboundedReceiver<std::vec::Vec<u8>>,
        bitrate_bps: u32,
    ) -> Self {
        let channel_tag = peer.ip().octets();
        let id = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, &channel_tag);
        Self {
            id,
            socket,
            peer,
            inbound,
            bitrate_bps,
            channel_tag,
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
    const KIND: InterfaceKind = InterfaceKind::WifiPeer;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, self.bitrate_bps)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
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
                    self.status.set_transfer_rates(throughput.rates());
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
                    self.status.set_transfer_rates(throughput.rates());
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
    status: AutoWifiStatus,
}

impl AutoWifi {
    #[must_use]
    pub fn new() -> Self {
        Self::with_bitrate(core::WIFI_LAN_BITRATE_BPS)
    }

    /// Declare a known pipe instead of the default wifi LAN bitrate; sets the members' announce pacing.
    #[must_use]
    pub fn with_bitrate(bitrate_bps: u32) -> Self {
        Self {
            bitrate_bps,
            status: AutoWifiStatus::new(InterfaceId::from_channel_tag(
                InterfaceKind::AutoWifi,
                core::GROUP_ID,
            )),
        }
    }

    /// A clone of this supervisor's aggregate live-status handle: connection (Offline with no NIC,
    /// Dormant when up with no peers, Live with peers), summed traffic, and peer count, plus a
    /// snapshot of each member's own status through [`members`](AutoWifiStatus::members). Call before
    /// [`supervise`](crate::runtime::TokioPrnsHandle::supervise) consumes the supervisor.
    #[must_use]
    pub fn status(&self) -> AutoWifiStatus {
        self.status.clone()
    }
}

impl Default for AutoWifi {
    fn default() -> Self {
        Self::new()
    }
}

/// The WiFi/LAN auto-interface's aggregate live status, an [`InterfaceStatus`] over the whole
/// supervisor: the app renders it as one "WiFi" card whose [`connection`](InterfaceStatus::connection)
/// is Offline (no NIC) / Dormant (up, no peers) / Live (peers), and whose bytes and
/// [`transfer_rates`](InterfaceStatus::transfer_rates) are the sum across the fleet. Its link and
/// destination counts are engine figures, summed over the members by the face's per-interface count
/// query rather than carried here. Each member also keeps its own [`TokioInterfaceStatus`], exposed
/// through [`members`](Self::members) so a face can render the peers as ordinary interface cards
/// beside the aggregate.
#[derive(Clone)]
pub struct AutoWifiStatus {
    shared: Arc<AutoWifiShared>,
}

struct AutoWifiShared {
    id: InterfaceId,
    up: AtomicBool,
    peers: AtomicU32,
    rx: AtomicU64,
    tx: AtomicU64,
    tx_failures: AtomicU64,
    egress_down: AtomicBool,
    members: Mutex<std::vec::Vec<TokioInterfaceStatus>>,
}

impl AutoWifiStatus {
    fn new(id: InterfaceId) -> Self {
        Self {
            shared: Arc::new(AutoWifiShared {
                id,
                up: AtomicBool::new(false),
                peers: AtomicU32::new(0),
                rx: AtomicU64::new(0),
                tx: AtomicU64::new(0),
                tx_failures: AtomicU64::new(0),
                egress_down: AtomicBool::new(false),
                members: Mutex::new(std::vec::Vec::new()),
            }),
        }
    }

    fn mark_up(&self) {
        self.shared.up.store(true, Ordering::Relaxed);
    }

    fn bump_tx_failures(&self) {
        self.shared.tx_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn set_egress_down(&self, down: bool) {
        self.shared.egress_down.store(down, Ordering::Relaxed);
    }

    fn publish(&self, peers: u32, rx: u64, tx: u64, members: std::vec::Vec<TokioInterfaceStatus>) {
        self.shared.peers.store(peers, Ordering::Relaxed);
        self.shared.rx.store(rx, Ordering::Relaxed);
        self.shared.tx.store(tx, Ordering::Relaxed);
        if let Ok(mut slot) = self.shared.members.lock() {
            *slot = members;
        }
    }

    /// A snapshot of each confirmed peer's own live status, for rendering the members as ordinary
    /// interface cards. Cheap: each handle is an `Arc` clone.
    #[must_use]
    pub fn members(&self) -> std::vec::Vec<TokioInterfaceStatus> {
        match self.shared.members.lock() {
            Ok(members) => members.clone(),
            Err(_) => std::vec::Vec::new(),
        }
    }

    /// Cumulative count of discovery beacons that never left the host, e.g. the OS rejecting
    /// multicast egress on the chosen NIC with `EHOSTUNREACH`. A climbing value alongside an Offline
    /// card is the "cannot transmit on this interface" signature, distinct from a NIC that never
    /// came up at all.
    #[must_use]
    pub fn tx_failures(&self) -> u64 {
        self.shared.tx_failures.load(Ordering::Relaxed)
    }

    /// True once recent discovery beacons could not be sent at all: the supervisor is up (NIC found,
    /// sockets bound) but the host is refusing multicast egress, so no peer can ever hear us. On
    /// macOS this is almost always the Local Network privacy gate, an ungranted app gets
    /// `EHOSTUNREACH` on every multicast send until the user approves it; elsewhere it usually means
    /// the link or AP is dropping multicast. The "~ just works ~" canary, an auto-interface that
    /// cannot beacon says so on its card (Offline) instead of sitting indistinguishable from a
    /// quiet-but-healthy Dormant.
    #[must_use]
    pub fn egress_down(&self) -> bool {
        self.shared.up.load(Ordering::Relaxed) && self.shared.egress_down.load(Ordering::Relaxed)
    }
}

impl InterfaceStatus for AutoWifiStatus {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.shared.up.load(Ordering::Relaxed)
            || self.shared.egress_down.load(Ordering::Relaxed)
        {
            ConnectionState::Failed
        } else if self.shared.peers.load(Ordering::Relaxed) > 0 {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    fn rx_bytes(&self) -> u64 {
        self.shared.rx.load(Ordering::Relaxed)
    }

    fn tx_bytes(&self) -> u64 {
        self.shared.tx.load(Ordering::Relaxed)
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        let members = self.shared.members.lock().ok()?;
        members
            .iter()
            .filter_map(InterfaceStatus::transfer_rates)
            .reduce(|acc, rates| TransferRates {
                rx_bps: acc.rx_bps.saturating_add(rates.rx_bps),
                tx_bps: acc.tx_bps.saturating_add(rates.tx_bps),
            })
    }
}

impl InterfaceSupervisor for AutoWifi {
    const KIND: InterfaceKind = InterfaceKind::AutoWifi;

    fn channel_tag(&self) -> &[u8] {
        core::GROUP_ID
    }

    async fn run(self, fleet: Fleet) {
        // Auto-wifi runs on *every* non-virtual link-local interface present — the WiFi client link,
        // a hosted AP, wired LAN — not just the default-route one (RNS's AutoInterface is inherently
        // multi-interface). One socket set joins the discovery group on every interface; each
        // interface gets its own brain (its own link-local, so its own peering token), and inbound
        // datagrams demux to the right brain by their source scope id.
        let nics = link_local_nics();
        if nics.is_empty() {
            return;
        }
        let Ok(Sockets {
            discovery,
            unicast_discovery,
            data,
        }) = open_sockets(&nics)
        else {
            return;
        };
        self.status.mark_up();

        dial_gateways(&fleet, &nics, self.bitrate_bps);

        let mut sup = Supervisor {
            brains: nics
                .iter()
                .map(|nic| {
                    (
                        nic.index,
                        core::AutoInterfaceProtocol::from_link_local(nic.link_local),
                    )
                })
                .collect(),
            members: HashMap::new(),
            fleet,
            data: data.clone(),
            bitrate_bps: self.bitrate_bps,
            status: self.status,
            consecutive_tx_failures: 0,
        };
        sup.publish_status();

        let started = tokio::time::Instant::now();
        let mut beacon = tokio::time::interval(BEACON_INTERVAL);
        let mut beacon_cycle: u32 = 0;
        let mut discovery_buf = [0u8; 64];
        let mut unicast_buf = [0u8; 64];
        let mut data_buf = [0u8; core::HARDWARE_MTU];
        let mut rendezvous = TcpListener::bind(("0.0.0.0", core::TCP_RENDEZVOUS_PORT))
            .await
            .ok();
        let mut loopback = bounce_to_local_core(&rendezvous, &sup.fleet, sup.bitrate_bps);

        loop {
            let mut reclaim_port = false;
            tokio::select! {
                accepted = accept_maybe(&rendezvous) => {
                    if let Ok((stream, peer)) = accepted {
                        tune(&stream);
                        let _ = sup.fleet.add(TcpServerConnection::new(
                            peer.to_string().into_bytes(),
                            stream,
                            sup.bitrate_bps,
                        ));
                    }
                }
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
                    // Beacon every interface with that interface's own token, and periodically
                    // unicast-repeer each interface's known peers (scoped to that interface).
                    let repeer = beacon_cycle.is_multiple_of(UNICAST_REPEER_EVERY);
                    let mut any_sent = false;
                    for (&index, brain) in &sup.brains {
                        let token = *brain.our_peering_token().as_bytes();
                        if discovery
                            .send_to(
                                &token,
                                scoped(core::DISCOVERY_GROUP, core::DEFAULT_DISCOVERY_PORT, index),
                            )
                            .await
                            .is_ok()
                        {
                            any_sent = true;
                        }
                        if repeer {
                            for addr in brain.known_peer_addresses() {
                                let _ = unicast_discovery
                                    .send_to(
                                        &token,
                                        scoped(addr, core::UNICAST_DISCOVERY_PORT, index),
                                    )
                                    .await;
                            }
                        }
                    }
                    sup.note_beacon(any_sent);
                    let now_ms = started.elapsed().as_millis() as u64;
                    sup.retire_stale(now_ms);
                    sup.publish_status();
                    reclaim_port =
                        rendezvous.is_none() && beacon_cycle.is_multiple_of(REBIND_BEACON_CYCLES);
                }
            }
            if reclaim_port {
                if let Ok(listener) =
                    TcpListener::bind(("0.0.0.0", core::TCP_RENDEZVOUS_PORT)).await
                {
                    rendezvous = Some(listener);
                    if let Some(loopback) = loopback.take() {
                        loopback.teardown();
                    }
                }
            }
        }
    }
}

struct PeerMember {
    attached: AttachedInterface,
    inbound: UnboundedSender<std::vec::Vec<u8>>,
    status: TokioInterfaceStatus,
}

struct Supervisor {
    /// One protocol brain per interface, keyed by its NIC index — each holds that interface's own
    /// link-local (so its own peering token) and peer table. Inbound datagrams demux here by the
    /// source address's scope id (the interface they arrived on).
    brains: HashMap<u32, core::AutoInterfaceProtocol<MAX_PEERS>>,
    members: HashMap<Ipv6Addr, PeerMember>,
    fleet: Fleet,
    data: Arc<UdpSocket>,
    bitrate_bps: u32,
    status: AutoWifiStatus,
    consecutive_tx_failures: u32,
}

impl Supervisor {
    fn ingest_beacon(&mut self, src: SocketAddr, bytes: &[u8], now_ms: u64) {
        let SocketAddr::V6(v6) = src else { return };
        let scope = v6.scope_id();
        let Some(brain) = self.brains.get_mut(&scope) else {
            return;
        };
        if let core::BeaconVerdict::Peer(addr) =
            brain.ingest_discovery_datagram(*v6.ip(), bytes, now_ms)
        {
            if !self.members.contains_key(&addr) {
                self.spawn_member(addr, scope);
            }
        }
    }

    fn spawn_member(&mut self, addr: Ipv6Addr, scope: u32) {
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let peer = SocketAddrV6::new(addr, core::DEFAULT_DATA_PORT, 0, scope);
        let member = AutoWifiPeer::new(self.data.clone(), peer, inbound_rx, self.bitrate_bps);
        let status = member.status();
        let attached = self.fleet.add(member);
        self.members.insert(
            addr,
            PeerMember {
                attached,
                inbound: inbound_tx,
                status,
            },
        );
        self.publish_status();
    }

    fn publish_status(&self) {
        let members: std::vec::Vec<TokioInterfaceStatus> = self
            .members
            .values()
            .map(|member| member.status.clone())
            .collect();
        let rx = members.iter().map(InterfaceStatus::rx_bytes).sum();
        let tx = members.iter().map(InterfaceStatus::tx_bytes).sum();
        self.status.publish(members.len() as u32, rx, tx, members);
    }

    fn note_beacon(&mut self, sent: bool) {
        if sent {
            self.consecutive_tx_failures = 0;
            self.status.set_egress_down(false);
        } else {
            self.status.bump_tx_failures();
            self.consecutive_tx_failures = self.consecutive_tx_failures.saturating_add(1);
            if self.consecutive_tx_failures >= EGRESS_DOWN_AFTER {
                self.status.set_egress_down(true);
            }
        }
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

    fn retire_stale(&mut self, now_ms: u64) {
        let pruned: usize = self
            .brains
            .values_mut()
            .map(|brain| brain.prune_stale_peers(now_ms))
            .sum();
        if pruned == 0 {
            return;
        }
        let live: HashSet<Ipv6Addr> = self
            .brains
            .values()
            .flat_map(|brain| brain.known_peer_addresses())
            .collect();
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
        self.publish_status();
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

/// Every non-virtual link-local interface the node should run auto-wifi on — its WiFi client link, a
/// hosted AP, wired LAN, all at once (RNS's AutoInterface is inherently multi-interface). Loopback
/// and known virtual/tunnel interfaces (utun, awdl, bridge, docker, …) are excluded, and the result
/// is deduplicated by interface index. The per-scope probe gives the kernel's own send-source
/// link-local for that interface, so the token we beacon matches what a peer recomputes from the
/// datagram source.
fn link_local_nics() -> std::vec::Vec<Nic> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return std::vec::Vec::new();
    };
    let mut nics = std::vec::Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for iface in ifaces {
        // Don't require an if_addrs *V6 entry*: some platforms only surface a NIC's global addresses
        // there, never its link-local — so an AP-only interface (just a link-local + a private v4,
        // e.g. `ap0`) would never qualify. The per-scope probe below is the real test of link-local
        // capability, and dedup-by-index means each interface is considered once regardless of how
        // many addresses it carries.
        if iface.is_loopback() || is_virtual(&iface.name) {
            continue;
        }
        let Some(index) = iface.index else { continue };
        if !seen.insert(index) {
            continue;
        }
        if let Some(link_local) = link_local_for_scope(index) {
            nics.push(Nic { link_local, index });
        }
    }
    nics
}

/// Beyond parity: dial each NIC's default gateway over TCP ([`TCP_RENDEZVOUS_PORT`]), standing up a
/// reconnecting [`TcpClientInterface`] member per gateway. On an ordinary network the gateway is no
/// Prns host and the member just retries in the background; on an isolating hotspot it is the one
/// reachable rendezvous, and the engine relays peer-to-peer through the link it forms. A hosted-AP
/// NIC has no gateway of its own, so a host never dials itself.
fn dial_gateways(fleet: &Fleet, nics: &[Nic], bitrate_bps: u32) {
    let interfaces = netdev::get_interfaces();
    for nic in nics {
        let Some(gateway) = interfaces
            .iter()
            .find(|iface| iface.index == nic.index)
            .and_then(gateway_addr)
        else {
            continue;
        };
        let target = SocketAddr::new(gateway, core::TCP_RENDEZVOUS_PORT).to_string();
        let _ = fleet.add(TcpClientInterface::new(target, bitrate_bps, GATEWAY_REDIAL));
    }
}

/// Dial the loopback rendezvous when another node on this host already holds the port, so its star
/// carries our traffic too — we bridge through the local core rather than going dark. `None` when we
/// hold the port ourselves; we are the core then, not a client of it.
fn bounce_to_local_core(
    rendezvous: &Option<TcpListener>,
    fleet: &Fleet,
    bitrate_bps: u32,
) -> Option<AttachedInterface> {
    if rendezvous.is_some() {
        return None;
    }
    let target = std::format!("127.0.0.1:{}", core::TCP_RENDEZVOUS_PORT);
    Some(fleet.add(TcpClientInterface::new(target, bitrate_bps, GATEWAY_REDIAL)))
}

/// Accept on the rendezvous listener when there is one, otherwise stay pending forever — so the
/// supervisor's `select!` carries a TCP-accept arm whether or not the port could be bound (a second
/// node on the host, a platform that refused it), without a separate code path.
async fn accept_maybe(listener: &Option<TcpListener>) -> io::Result<(TcpStream, SocketAddr)> {
    match listener {
        Some(listener) => listener.accept().await,
        None => std::future::pending().await,
    }
}

/// The default-gateway address an interface routes through — IPv4 preferred (a hotspot hands out a v4
/// gateway), else IPv6. `None` when the NIC has no default route (a hosted AP, a link-local-only
/// segment), so a host's own AP interface is skipped.
fn gateway_addr(iface: &netdev::Interface) -> Option<IpAddr> {
    let gateway = iface.gateway.as_ref()?;
    gateway
        .ipv4
        .first()
        .copied()
        .map(IpAddr::V4)
        .or_else(|| gateway.ipv6.first().copied().map(IpAddr::V6))
}

fn is_virtual(name: &str) -> bool {
    const VIRTUAL_PREFIXES: [&str; 13] = [
        "utun", "tun", "tap", "ppp", "ipsec", "awdl", "llw", "gif", "stf", "bridge", "vmnet",
        "vnic", "docker",
    ];
    VIRTUAL_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
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

fn open_sockets(nics: &[Nic]) -> io::Result<Sockets> {
    Ok(Sockets {
        discovery: discovery_socket(nics)?,
        unicast_discovery: bound_v6(core::UNICAST_DISCOVERY_PORT)?,
        data: Arc::new(bound_v6(core::DEFAULT_DATA_PORT)?),
    })
}

/// One discovery socket that joins the multicast group on *every* interface, so it both hears the
/// group on all of them and (via scoped sends) beacons across all of them. Sends always carry an
/// explicit scope, so no default multicast interface is set.
fn discovery_socket(nics: &[Nic]) -> io::Result<UdpSocket> {
    let socket = reusable_socket2(core::DEFAULT_DISCOVERY_PORT)?;
    // Best-effort per interface: a join that fails (an interface that can't do multicast) is skipped,
    // never fatal — one bad interface must not take auto-wifi down on every other one.
    let joined = nics
        .iter()
        .filter(|nic| {
            socket
                .join_multicast_v6(&core::DISCOVERY_GROUP, nic.index)
                .is_ok()
        })
        .count();
    if joined == 0 {
        return Err(io::Error::other("no interface joined the discovery group"));
    }
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

impl crate::interfaces::ReportsStatus for AutoWifi {
    fn status_view(&self) -> Option<crate::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![crate::interfaces::InterfaceSnapshot::of(&status)]
        }))
    }
}

impl crate::interfaces::ReportsStatus for AutoWifiPeer {
    fn status_view(&self) -> Option<crate::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![crate::interfaces::InterfaceSnapshot::of(&status)]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};

    const TEST_FRAME_CAP: usize = 2_048;

    /// A hand-driven seam, as in the UDP tests: captures every `next_inbound`, supplies
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
            shared.clone(),
            peer_addr,
            demux_rx,
            core::WIFI_BITRATE_GUESS_BPS,
        );

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(TEST_FRAME_CAP, 2);
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
