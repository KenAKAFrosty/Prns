use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc::{self, Receiver, Sender, UnboundedReceiver};

use crate::tcp::client::TcpClientInterface;
use crate::tcp::server::TcpServerConnection;
use crate::tcp::tokio_socket::tune;
use prns_core::engine::InstantMillis;
use prns_core::interfaces::wifi_auto::core;
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
    InterfaceStatus, TransferRates,
};
use prns_runtime::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use prns_runtime::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::reactor::throughput::ThroughputLedger;
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
const UNICAST_REPEER_EVERY: u32 = 3;
/// How long a local rendezvous link waits before redialing after a failed or dropped connection.
/// This is intentionally shorter than the reference TCP client's general reconnect wait: gateway,
/// loopback, and mDNS dials are the WiFi-auto lifeline on SoftAP and phone networks where multicast
/// is absent, so a dropped peer should not leave the aggregate card Dormant for a long retry window.
const RENDEZVOUS_REDIAL: Duration = Duration::from_secs(3);
/// Beacon cycles between attempts to reclaim the rendezvous port when another local node holds it.
/// Three cycles of [`BEACON_INTERVAL`] (~5s) — brisk enough to take over promptly once the holder
/// exits and frees the port, without calling bind() every beacon.
const REBIND_BEACON_CYCLES: u32 = 3;
/// Per-peer UDP demux backlog. Full means the peer's interface task is behind, so new datagrams are
/// dropped rather than letting one noisy peer grow process memory.
const PEER_INBOUND_DEPTH: usize = 32;

pub struct AutoWifiPeer {
    id: InterfaceId,
    socket: Arc<UdpSocket>,
    peer: SocketAddrV6,
    inbound: Receiver<std::vec::Vec<u8>>,
    policy: EffectiveInterfacePolicy,
    channel_tag: [u8; 16],
    status: TokioInterfaceStatus,
}

impl AutoWifiPeer {
    /// The member's id is derived from its peer's link-local address — EUI-64-stable to the peer's
    /// MAC, so the same device reconnecting (even on a fresh socket) rebinds the routes it owned.
    pub fn new(
        socket: Arc<UdpSocket>,
        peer: SocketAddrV6,
        inbound: Receiver<std::vec::Vec<u8>>,
        bitrate: BitrateBps,
    ) -> Self {
        Self::with_policy(socket, peer, inbound, core::policy_for_bitrate(bitrate))
    }

    pub fn with_policy(
        socket: Arc<UdpSocket>,
        peer: SocketAddrV6,
        inbound: Receiver<std::vec::Vec<u8>>,
        policy: EffectiveInterfacePolicy,
    ) -> Self {
        let channel_tag = peer.ip().octets();
        let id = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, &channel_tag);
        Self {
            id,
            socket,
            peer,
            inbound,
            policy,
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

    fn descriptor(&self) -> InterfaceDescriptor {
        core::descriptor(self.id, self.policy)
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
                    let frame_airtime = frame_airtime_us(sent, self.policy.bitrate);
                    self.status.set_airtime(airtime.record_tx(now, frame_airtime));
                }
            }
        }
    }
}

/// The RNS WiFi/LAN `AutoInterface` as an [`InterfaceSupervisor`]: it owns the multicast
/// discovery sockets and the single shared data socket, beacons its peering token, validates
/// inbound beacons, and stands up an [`AutoWifiPeer`] member per confirmed peer, demuxing
/// inbound datagrams back to each member by source. Attach with `handle.supervise(AutoWifi::new())`.
pub struct AutoWifi {
    policy: EffectiveInterfacePolicy,
    status: AutoWifiStatus,
    mdns: Option<UnboundedReceiver<SocketAddr>>,
}

impl AutoWifi {
    #[must_use]
    pub fn new() -> Self {
        Self::with_policy(core::configured_policy(Default::default()))
    }

    /// Declare a known pipe instead of the default wifi LAN bitrate; sets the members' announce pacing.
    #[must_use]
    pub fn with_bitrate(bitrate: BitrateBps) -> Self {
        Self::with_policy(core::policy_for_bitrate(bitrate))
    }

    #[must_use]
    pub fn with_policy(policy: EffectiveInterfacePolicy) -> Self {
        Self {
            policy,
            status: AutoWifiStatus::new(InterfaceId::from_channel_tag(
                InterfaceKind::AutoWifi,
                core::GROUP_ID,
            )),
            mdns: None,
        }
    }

    /// Add Bonjour/mDNS as a second, concurrent discovery channel: each resolved rendezvous
    /// endpoint arriving on `sightings` is dialed as a [`TcpClientInterface`] to the same
    /// [`core::TCP_RENDEZVOUS_PORT`] the multicast star and gateway fold use, deduped by
    /// target. This is what lets a peer that cannot run raw multicast (iOS, exempt from the
    /// entitlement only for standard Bonjour) still be reached; the platform backend feeds the channel from outside, keeping this supervisor `forbid-unsafe`.
    #[must_use]
    pub fn with_mdns(mut self, sightings: UnboundedReceiver<SocketAddr>) -> Self {
        self.mdns = Some(sightings);
        self
    }

    /// A clone of this supervisor's aggregate live-status handle: connection (Offline with no NIC,
    /// Dormant when up with no peers, Live with peers), summed traffic, and peer count, plus a
    /// snapshot of each member's own status through [`members`](AutoWifiStatus::members). Call before
    /// [`supervise`](prns_runtime::runtime::TokioPrnsHandle::supervise) consumes the supervisor.
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
/// supervisor: one "WiFi" card whose connection is Offline (no NIC) / Dormant (up, no peers) /
/// Live (peers), and whose bytes and rates are the sum across the fleet. Link and destination
/// counts are engine figures summed by the face, not carried here. Each member keeps its own
/// [`TokioInterfaceStatus`], exposed through [`members`](Self::members) for per-peer cards.
#[derive(Clone)]
pub struct AutoWifiStatus {
    shared: Arc<AutoWifiShared>,
}

struct AutoWifiShared {
    id: InterfaceId,
    enabled: AtomicBool,
    peers: AtomicU32,
    rx: AtomicU64,
    tx: AtomicU64,
    members: Mutex<std::vec::Vec<TokioInterfaceStatus>>,
}

impl AutoWifiStatus {
    fn new(id: InterfaceId) -> Self {
        Self {
            shared: Arc::new(AutoWifiShared {
                id,
                enabled: AtomicBool::new(true),
                peers: AtomicU32::new(0),
                rx: AtomicU64::new(0),
                tx: AtomicU64::new(0),
                members: Mutex::new(std::vec::Vec::new()),
            }),
        }
    }

    /// Turn WiFi-auto discovery and its current members off or back on from the application.
    pub fn set_enabled(&self, enabled: bool) {
        self.shared.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether WiFi-auto should be discovering and carrying peers.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
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
}

impl InterfaceStatus for AutoWifiStatus {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            ConnectionState::Disabled
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
        // Auto-wifi runs on *every* non-virtual link-local interface present (RNS's
        // AutoInterface is inherently multi-interface): one socket set joins the group on every
        // interface, each interface gets its own brain (its own link-local, so its own peering
        // token), and inbound datagrams demux by source scope id. The multicast plane is
        // best-effort: a platform that cannot stand it up (iOS) still runs the rendezvous, gateway, and mDNS planes.
        let mut nics = link_local_nics();
        let sockets = if nics.is_empty() {
            None
        } else {
            open_sockets(&nics).ok()
        };
        let mut sup = Supervisor {
            brains: if sockets.is_some() {
                nics.iter()
                    .map(|nic| {
                        (
                            nic.index,
                            core::HeapAutoInterfaceProtocol::from_link_local(nic.link_local),
                        )
                    })
                    .collect()
            } else {
                HashMap::new()
            },
            members: HashMap::new(),
            gateways: HashMap::new(),
            mdns_dials: HashMap::new(),
            accepted: std::vec::Vec::new(),
            prefixes: local_prefixes(),
            fleet,
            data: sockets.as_ref().map(|s| s.data.clone()),
            policy: self.policy,
            status: self.status,
        };
        sup.dial_initial_gateways(&nics);
        sup.publish_status();

        let mut mdns = self.mdns;
        let started = tokio::time::Instant::now();
        let mut beacon = tokio::time::interval(BEACON_INTERVAL);
        let mut beacon_cycle: u32 = 0;
        let mut discovery_buf = [0u8; 64];
        let mut unicast_buf = [0u8; 64];
        let mut data_buf = [0u8; core::HARDWARE_MTU];
        let mut rendezvous = TcpListener::bind(("0.0.0.0", core::TCP_RENDEZVOUS_PORT))
            .await
            .ok();
        let mut loopback = bounce_to_local_core(&rendezvous, &sup.fleet, sup.policy);

        loop {
            while !sup.status.is_enabled() {
                sup.disable_members();
                tokio::time::sleep(BEACON_INTERVAL).await;
            }
            let mut reclaim_port = false;
            tokio::select! {
                accepted = accept_maybe(&rendezvous) => {
                    if let Ok((stream, peer)) = accepted {
                        if is_local_peer(peer.ip(), &sup.prefixes) {
                            tune(&stream);
                            let connection = TcpServerConnection::with_policy(
                                peer.to_string().into_bytes(),
                                stream,
                                sup.policy,
                            );
                            let status = connection.status();
                            let attached = sup.fleet.add(connection);
                            sup.accepted.push(AttachedStatus { attached, status });
                            sup.publish_status();
                        }
                    }
                }
                received = recv_maybe(sockets.as_ref().map(|s| &s.discovery), &mut discovery_buf) => {
                    if let Some((len, src)) = received {
                        let now_ms = started.elapsed().as_millis() as u64;
                        sup.ingest_beacon(src, &discovery_buf[..len], now_ms);
                    }
                }
                received = recv_maybe(sockets.as_ref().map(|s| &s.unicast_discovery), &mut unicast_buf) => {
                    if let Some((len, src)) = received {
                        let now_ms = started.elapsed().as_millis() as u64;
                        sup.ingest_beacon(src, &unicast_buf[..len], now_ms);
                    }
                }
                received = recv_maybe(sockets.as_ref().map(|s| s.data.as_ref()), &mut data_buf) => {
                    if let Some((len, src)) = received {
                        sup.route_inbound(src, &data_buf[..len]);
                    }
                }
                sighting = next_mdns(mdns.as_mut()) => {
                    match sighting {
                        Some(target) => sup.dial_mdns_sighting(target),
                        None => mdns = None,
                    }
                }
                _ = beacon.tick() => {
                    beacon_cycle = beacon_cycle.wrapping_add(1);
                    let repeer = beacon_cycle.is_multiple_of(UNICAST_REPEER_EVERY);
                    if let Some(sockets) = sockets.as_ref() {
                        for (&index, brain) in &sup.brains {
                            let token = *brain.our_peering_token().as_bytes();
                            let _ = sockets
                                .discovery
                                .send_to(
                                    &token,
                                    scoped(core::DISCOVERY_GROUP, core::DEFAULT_DISCOVERY_PORT, index),
                                )
                                .await;
                            if repeer {
                                for addr in brain.known_peer_addresses() {
                                    let _ = sockets
                                        .unicast_discovery
                                        .send_to(
                                            &token,
                                            scoped(addr, core::UNICAST_DISCOVERY_PORT, index),
                                        )
                                        .await;
                                }
                            }
                        }
                    }
                    let now_ms = started.elapsed().as_millis() as u64;
                    if beacon_cycle.is_multiple_of(REBIND_BEACON_CYCLES) {
                        sup.reconcile_nics(sockets.as_ref().map(|s| &s.discovery), &mut nics);
                    }
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
    inbound: Sender<std::vec::Vec<u8>>,
    status: TokioInterfaceStatus,
}

struct AttachedStatus {
    attached: AttachedInterface,
    status: TokioInterfaceStatus,
}

struct Supervisor {
    /// One protocol brain per interface, keyed by its NIC index — each holds that interface's own
    /// link-local (so its own peering token) and peer table. Inbound datagrams demux here by the
    /// source address's scope id (the interface they arrived on). Empty when the multicast plane did
    /// not come up (iOS), so the supervisor is rendezvous- and mDNS-only.
    brains: HashMap<u32, core::HeapAutoInterfaceProtocol>,
    members: HashMap<Ipv6Addr, PeerMember>,
    gateways: HashMap<u32, GatewayDial>,
    /// One TCP dial per mDNS-discovered peer rendezvous, keyed by target so a repeated sighting never
    /// stacks a second dial. Holds the dial's live status for the aggregate; the dial itself is a
    /// fleet member, so disabling WiFi-auto can tear it down with the rest of the umbrella.
    mdns_dials: HashMap<SocketAddr, AttachedStatus>,
    accepted: std::vec::Vec<AttachedStatus>,
    prefixes: std::vec::Vec<LocalPrefix>,
    fleet: Fleet,
    /// The shared UDP data socket each multicast-discovered member transmits on; `None` when the
    /// multicast plane is absent, which is exactly when no member is ever spawned.
    data: Option<Arc<UdpSocket>>,
    policy: EffectiveInterfacePolicy,
    status: AutoWifiStatus,
}

struct GatewayDial {
    gateway: IpAddr,
    attached: AttachedInterface,
    status: TokioInterfaceStatus,
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
        let Some(data) = self.data.clone() else {
            return;
        };
        let (inbound_tx, inbound_rx) = mpsc::channel(PEER_INBOUND_DEPTH);
        let peer = SocketAddrV6::new(addr, core::DEFAULT_DATA_PORT, 0, scope);
        let member = AutoWifiPeer::with_policy(data, peer, inbound_rx, self.policy);
        let status = member.status();
        let attached = self.fleet.add(member);
        crate::diagnostic_log::debug!("wifi-auto: peer {addr}%{scope} discovered over multicast");
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

    fn publish_status(&mut self) {
        let mut accepted = std::vec::Vec::new();
        for member in self.accepted.drain(..) {
            if matches!(member.status.connection(), ConnectionState::Disconnected) {
                member.attached.teardown();
            } else {
                accepted.push(member);
            }
        }
        self.accepted = accepted;
        let mut statuses: std::vec::Vec<TokioInterfaceStatus> = self
            .members
            .values()
            .map(|member| member.status.clone())
            .collect();
        statuses.extend(self.gateways.values().map(|dial| dial.status.clone()));
        statuses.extend(self.accepted.iter().map(|member| member.status.clone()));
        statuses.extend(self.mdns_dials.values().map(|dial| dial.status.clone()));
        let rx = statuses.iter().map(InterfaceStatus::rx_bytes).sum();
        let tx = statuses.iter().map(InterfaceStatus::tx_bytes).sum();
        let peers = statuses.len();
        self.status.publish(peers as u32, rx, tx, statuses);
    }

    fn disable_members(&mut self) {
        for (_, member) in self.members.drain() {
            member.attached.teardown();
        }
        for (_, dial) in self.gateways.drain() {
            dial.attached.teardown();
        }
        for (_, dial) in self.mdns_dials.drain() {
            dial.attached.teardown();
        }
        for member in self.accepted.drain(..) {
            member.attached.teardown();
        }
        self.status.publish(0, 0, 0, std::vec::Vec::new());
    }

    /// Dial a peer's rendezvous endpoint surfaced by Bonjour/mDNS, reusing the gateway fold's
    /// [`TcpClientInterface`] so an mDNS-found peer becomes an ordinary fleet member. Deduped by
    /// target so a repeated sighting never stacks a second dial, and self-skipping so we never dial
    /// our own advertised rendezvous even if the backend surfaces it.
    fn dial_mdns_sighting(&mut self, target: SocketAddr) {
        if is_own_address(target.ip(), &self.prefixes) || self.mdns_dials.contains_key(&target) {
            return;
        }
        crate::diagnostic_log::debug!("wifi-auto: dialing mDNS rendezvous {target}");
        let client =
            TcpClientInterface::with_policy(target.to_string(), self.policy, RENDEZVOUS_REDIAL);
        let status = client.status();
        let attached = self.fleet.add(client);
        self.mdns_dials
            .insert(target, AttachedStatus { attached, status });
        self.publish_status();
    }

    fn route_inbound(&self, src: SocketAddr, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let SocketAddr::V6(v6) = src else { return };
        if let Some(member) = self.members.get(v6.ip()) {
            let _ = member.inbound.try_send(bytes.to_vec());
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
        self.reap_orphaned_members();
    }

    fn reap_orphaned_members(&mut self) {
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
        if gone.is_empty() {
            return;
        }
        for addr in gone {
            if let Some(member) = self.members.remove(&addr) {
                crate::diagnostic_log::debug!(
                    "wifi-auto: peer {addr} retired after missed beacons"
                );
                member.attached.teardown();
            }
        }
        self.publish_status();
    }

    fn dial_initial_gateways(&mut self, nics: &[Nic]) {
        let ifaces = netdev::get_interfaces();
        for nic in nics {
            self.refresh_gateway(nic.index, gateway_for(&ifaces, nic.index));
        }
    }

    fn refresh_gateway(&mut self, index: u32, gateway: Option<IpAddr>) {
        if self.gateways.get(&index).map(|dial| dial.gateway) == gateway {
            return;
        }
        if let Some(old) = self.gateways.remove(&index) {
            crate::diagnostic_log::debug!(
                "wifi-auto: gateway rendezvous removed on ifindex {index} ({})",
                old.gateway
            );
            old.attached.teardown();
        }
        if let Some(gateway) = gateway {
            let target = SocketAddr::new(gateway, core::TCP_RENDEZVOUS_PORT).to_string();
            crate::diagnostic_log::debug!(
                "wifi-auto: dialing gateway rendezvous {target} on ifindex {index}"
            );
            let client = TcpClientInterface::with_policy(target, self.policy, RENDEZVOUS_REDIAL);
            let status = client.status();
            let attached = self.fleet.add(client);
            self.gateways.insert(
                index,
                GatewayDial {
                    gateway,
                    attached,
                    status,
                },
            );
        }
    }

    fn reconcile_nics(&mut self, discovery: Option<&UdpSocket>, nics: &mut std::vec::Vec<Nic>) {
        let Some(discovery) = discovery else {
            return;
        };
        let fresh = link_local_nics();
        let ifaces = netdev::get_interfaces();
        self.prefixes = local_prefixes();
        self.apply_reconcile(discovery, nics, fresh, |index| gateway_for(&ifaces, index));
    }

    fn apply_reconcile(
        &mut self,
        discovery: &UdpSocket,
        nics: &mut std::vec::Vec<Nic>,
        fresh: std::vec::Vec<Nic>,
        gateway_of: impl Fn(u32) -> Option<IpAddr>,
    ) {
        let plan = plan_reconcile(nics, &fresh);
        for index in plan.removed {
            let _ = discovery.leave_multicast_v6(&core::DISCOVERY_GROUP, index);
            self.brains.remove(&index);
            if let Some(dial) = self.gateways.remove(&index) {
                dial.attached.teardown();
            }
        }
        for nic in plan.added {
            let _ = discovery.join_multicast_v6(&core::DISCOVERY_GROUP, nic.index);
            self.brains.insert(
                nic.index,
                core::HeapAutoInterfaceProtocol::from_link_local(nic.link_local),
            );
        }
        for nic in plan.rebound {
            self.brains.insert(
                nic.index,
                core::HeapAutoInterfaceProtocol::from_link_local(nic.link_local),
            );
        }
        for nic in &fresh {
            self.refresh_gateway(nic.index, gateway_of(nic.index));
        }
        self.reap_orphaned_members();
        *nics = fresh;
    }
}

#[derive(Default, PartialEq, Debug)]
struct ReconcilePlan {
    removed: std::vec::Vec<u32>,
    added: std::vec::Vec<Nic>,
    rebound: std::vec::Vec<Nic>,
}

fn plan_reconcile(current: &[Nic], fresh: &[Nic]) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();
    for old in current {
        if !fresh.iter().any(|nic| nic.index == old.index) {
            plan.removed.push(old.index);
        }
    }
    for nic in fresh {
        match current.iter().find(|old| old.index == nic.index) {
            None => plan.added.push(*nic),
            Some(old) if old.link_local != nic.link_local => plan.rebound.push(*nic),
            Some(_) => {}
        }
    }
    plan
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Nic {
    link_local: Ipv6Addr,
    index: u32,
}

struct Sockets {
    discovery: UdpSocket,
    unicast_discovery: UdpSocket,
    data: Arc<UdpSocket>,
}

/// Every non-virtual link-local interface the node should run auto-wifi on, deduplicated by
/// index; loopback and known virtual/tunnel interfaces (utun, awdl, bridge, docker) are
/// excluded. The per-scope probe gives the kernel's own send-source link-local for that
/// interface, so the token we beacon matches what a peer recomputes from the datagram source.
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

#[derive(Clone, Copy, PartialEq, Debug)]
struct LocalPrefix {
    addr: IpAddr,
    netmask: IpAddr,
}

fn local_prefixes() -> std::vec::Vec<LocalPrefix> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return std::vec::Vec::new();
    };
    ifaces
        .iter()
        .filter(|iface| !iface.is_loopback())
        .map(|iface| match &iface.addr {
            if_addrs::IfAddr::V4(v4) => LocalPrefix {
                addr: IpAddr::V4(v4.ip),
                netmask: IpAddr::V4(v4.netmask),
            },
            if_addrs::IfAddr::V6(v6) => LocalPrefix {
                addr: IpAddr::V6(v6.ip),
                netmask: IpAddr::V6(v6.netmask),
            },
        })
        .collect()
}

fn is_local_peer(peer: IpAddr, prefixes: &[LocalPrefix]) -> bool {
    peer.is_loopback()
        || prefixes
            .iter()
            .any(|prefix| same_subnet(peer, prefix.addr, prefix.netmask))
}

/// Whether `ip` is one of this host's own interface addresses (or loopback) — the self-dial guard for
/// mDNS sightings, since Bonjour browses surface our own advertised service back to us and dialing it
/// would loop the rendezvous onto itself.
fn is_own_address(ip: IpAddr, prefixes: &[LocalPrefix]) -> bool {
    ip.is_loopback() || prefixes.iter().any(|prefix| prefix.addr == ip)
}

fn same_subnet(peer: IpAddr, addr: IpAddr, netmask: IpAddr) -> bool {
    match (peer, addr, netmask) {
        (IpAddr::V4(peer), IpAddr::V4(addr), IpAddr::V4(mask)) => {
            let mask = u32::from(mask);
            (u32::from(peer) & mask) == (u32::from(addr) & mask)
        }
        (IpAddr::V6(peer), IpAddr::V6(addr), IpAddr::V6(mask)) => {
            let mask = u128::from(mask);
            (u128::from(peer) & mask) == (u128::from(addr) & mask)
        }
        _ => false,
    }
}

fn gateway_for(ifaces: &[netdev::Interface], index: u32) -> Option<IpAddr> {
    ifaces
        .iter()
        .find(|iface| iface.index == index)
        .and_then(gateway_addr)
}

/// Dial the loopback rendezvous when another node on this host already holds the port, so its star
/// carries our traffic too — we bridge through the local core rather than going dark. `None` when we
/// hold the port ourselves; we are the core then, not a client of it.
fn bounce_to_local_core(
    rendezvous: &Option<TcpListener>,
    fleet: &Fleet,
    policy: EffectiveInterfacePolicy,
) -> Option<AttachedInterface> {
    if rendezvous.is_some() {
        return None;
    }
    let target = std::format!("127.0.0.1:{}", core::TCP_RENDEZVOUS_PORT);
    Some(fleet.add(TcpClientInterface::with_policy(
        target,
        policy,
        RENDEZVOUS_REDIAL,
    )))
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

/// Receive on a multicast socket when the multicast plane is up, otherwise stay pending forever — so
/// the discovery/unicast/data arms are present whether or not multicast could be stood up, mirroring
/// [`accept_maybe`]. A receive error yields `None`, which the arm ignores.
async fn recv_maybe(socket: Option<&UdpSocket>, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
    match socket {
        Some(socket) => socket.recv_from(buf).await.ok(),
        None => std::future::pending().await,
    }
}

/// Take the next Bonjour/mDNS sighting when the channel is present, otherwise stay pending forever.
/// `None` once the backend's sender is dropped, which the caller uses to retire the arm.
async fn next_mdns(sightings: Option<&mut UnboundedReceiver<SocketAddr>>) -> Option<SocketAddr> {
    match sightings {
        Some(sightings) => sightings.recv().await,
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
    const VIRTUAL_PREFIXES: [&str; 14] = [
        "utun", "tun", "tap", "ppp", "ipsec", "awdl", "llw", "gif", "stf", "bridge", "vmnet",
        "vnic", "docker", "p2p",
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

impl prns_core::interfaces::ReportsStatus for AutoWifi {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl prns_core::interfaces::ReportsStatus for AutoWifiPeer {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_runtime::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};

    const TEST_FRAME_CAP: usize = 2_048;

    fn nic(index: u32, link_local_tail: u16) -> Nic {
        Nic {
            index,
            link_local: Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, link_local_tail),
        }
    }

    #[test]
    fn wifi_direct_group_netdevs_read_as_virtual() {
        assert!(is_virtual("p2p-wlan0-0"));
        assert!(is_virtual("p2p-dev-wlan0"));
        assert!(!is_virtual("wlan0"));
        assert!(!is_virtual("wlp0s20f3"));
    }

    #[test]
    fn reconcile_plan_is_empty_when_the_nic_set_is_unchanged() {
        let current = [nic(1, 0x10), nic(2, 0x20)];
        let plan = plan_reconcile(&current, &current);
        assert_eq!(plan, ReconcilePlan::default());
    }

    #[test]
    fn reconcile_plan_adds_a_brand_new_nic() {
        let plan = plan_reconcile(&[nic(1, 0x10)], &[nic(1, 0x10), nic(2, 0x20)]);
        assert_eq!(plan.added, std::vec![nic(2, 0x20)]);
        assert!(plan.removed.is_empty());
        assert!(plan.rebound.is_empty());
    }

    #[test]
    fn reconcile_plan_removes_a_vanished_nic_by_index() {
        let plan = plan_reconcile(&[nic(1, 0x10), nic(2, 0x20)], &[nic(1, 0x10)]);
        assert_eq!(plan.removed, std::vec![2]);
        assert!(plan.added.is_empty());
        assert!(plan.rebound.is_empty());
    }

    #[test]
    fn reconcile_plan_rebinds_a_nic_whose_link_local_changed() {
        let plan = plan_reconcile(&[nic(1, 0x10)], &[nic(1, 0x99)]);
        assert_eq!(plan.rebound, std::vec![nic(1, 0x99)]);
        assert!(plan.added.is_empty());
        assert!(plan.removed.is_empty());
    }

    #[test]
    fn reconcile_plan_handles_add_remove_and_rebind_at_once() {
        let current = [nic(1, 0x10), nic(2, 0x20)];
        let fresh = [nic(2, 0x99), nic(3, 0x30)];
        let plan = plan_reconcile(&current, &fresh);
        assert_eq!(plan.removed, std::vec![1]);
        assert_eq!(plan.added, std::vec![nic(3, 0x30)]);
        assert_eq!(plan.rebound, std::vec![nic(2, 0x99)]);
    }

    #[test]
    fn reconcile_plan_drops_every_nic_when_the_link_goes_away() {
        let plan = plan_reconcile(&[nic(1, 0x10), nic(2, 0x20)], &[]);
        assert_eq!(plan.removed, std::vec![1, 2]);
        assert!(plan.added.is_empty());
        assert!(plan.rebound.is_empty());
    }

    fn prefix(addr: [u8; 4], mask: [u8; 4]) -> LocalPrefix {
        LocalPrefix {
            addr: IpAddr::V4(std::net::Ipv4Addr::new(addr[0], addr[1], addr[2], addr[3])),
            netmask: IpAddr::V4(std::net::Ipv4Addr::new(mask[0], mask[1], mask[2], mask[3])),
        }
    }

    fn v4(addr: [u8; 4]) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::new(addr[0], addr[1], addr[2], addr[3]))
    }

    #[test]
    fn a_hotspot_client_on_our_subnet_is_a_local_peer() {
        let prefixes = [prefix([192, 168, 137, 1], [255, 255, 255, 0])];
        assert!(is_local_peer(v4([192, 168, 137, 128]), &prefixes));
    }

    #[test]
    fn a_routed_internet_peer_is_not_local() {
        let prefixes = [prefix([192, 168, 137, 1], [255, 255, 255, 0])];
        assert!(!is_local_peer(v4([8, 8, 8, 8]), &prefixes));
    }

    #[test]
    fn a_peer_on_a_different_private_subnet_is_not_local() {
        let prefixes = [prefix([192, 168, 137, 1], [255, 255, 255, 0])];
        assert!(!is_local_peer(v4([192, 168, 1, 50]), &prefixes));
    }

    #[test]
    fn loopback_is_always_local_even_with_no_prefixes() {
        assert!(is_local_peer(v4([127, 0, 0, 1]), &[]));
        assert!(is_local_peer(IpAddr::V6(Ipv6Addr::LOCALHOST), &[]));
    }

    #[test]
    fn a_peer_matches_any_one_of_several_attached_prefixes() {
        let prefixes = [
            prefix([192, 168, 137, 1], [255, 255, 255, 0]),
            prefix([10, 0, 0, 2], [255, 0, 0, 0]),
        ];
        assert!(is_local_peer(v4([10, 55, 4, 9]), &prefixes));
        assert!(!is_local_peer(v4([172, 16, 0, 1]), &prefixes));
    }

    fn test_supervisor() -> (Supervisor, prns_runtime::runtime::DetachedFleet) {
        let id = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, core::GROUP_ID);
        let (fleet, guard) = Fleet::detached(id);
        let data = std::net::UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).expect("bind data socket");
        data.set_nonblocking(true).expect("nonblocking");
        let sup = Supervisor {
            brains: HashMap::new(),
            members: HashMap::new(),
            gateways: HashMap::new(),
            mdns_dials: HashMap::new(),
            accepted: std::vec::Vec::new(),
            prefixes: std::vec::Vec::new(),
            fleet,
            data: Some(Arc::new(UdpSocket::from_std(data).expect("into tokio"))),
            policy: core::configured_policy(Default::default()),
            status: AutoWifiStatus::new(id),
        };
        (sup, guard)
    }

    #[tokio::test]
    async fn a_peer_inherits_the_auto_wifi_effective_policy() {
        let socket = Arc::new(UdpSocket::bind("[::1]:0").await.unwrap());
        let (_inbound_tx, inbound_rx) = mpsc::channel(1);
        let policy = core::configured_policy(prns_core::interfaces::ConfiguredInterfacePolicy {
            mode: Some(prns_core::interfaces::InterfaceMode::Gateway),
            bitrate: Some(BitrateBps::guess(900_000_000)),
            ..prns_core::interfaces::ConfiguredInterfacePolicy::default()
        });
        let peer = AutoWifiPeer::with_policy(
            socket,
            SocketAddrV6::new(Ipv6Addr::LOCALHOST, 1, 0, 0),
            inbound_rx,
            policy,
        );

        assert_eq!(peer.descriptor(), policy.descriptor(peer.id()));
    }

    #[tokio::test]
    async fn aggregate_stays_live_while_a_multicast_peer_is_registered() {
        let (mut sup, _guard) = test_supervisor();
        let peer = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x867c);

        sup.spawn_member(peer, 0);
        assert_eq!(sup.status.connection(), ConnectionState::Connected);

        let member = sup.members.get(&peer).expect("peer was registered");
        member.status.set_connection(ConnectionState::Disconnected);
        sup.publish_status();

        assert_eq!(
            sup.status.connection(),
            ConnectionState::Connected,
            "the aggregate follows the validated peer table, not a transient child-status blip",
        );
    }

    #[tokio::test]
    async fn aggregate_stays_live_while_a_rendezvous_peer_is_registered() {
        use std::net::Ipv4Addr;
        let (mut sup, _guard) = test_supervisor();
        let peer = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 77)),
            core::TCP_RENDEZVOUS_PORT,
        );

        sup.dial_mdns_sighting(peer);
        let status = &sup.mdns_dials.get(&peer).expect("peer was dialed").status;
        status.set_connection(ConnectionState::Disconnected);
        sup.publish_status();

        assert_eq!(
            sup.status.connection(),
            ConnectionState::Connected,
            "the aggregate follows the rendezvous peer table, not a transient child-status blip",
        );
    }

    #[tokio::test]
    async fn reconcile_applies_nic_churn_and_repoints_gateways_against_a_real_fleet() {
        let (mut sup, _guard) = test_supervisor();
        let discovery = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0))
            .await
            .expect("bind discovery socket");
        let mut nics = std::vec::Vec::new();

        let gw_a = |index: u32| match index {
            1 => Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1))),
            _ => None,
        };
        sup.apply_reconcile(
            &discovery,
            &mut nics,
            std::vec![nic(1, 0x10), nic(2, 0x20)],
            gw_a,
        );
        assert_eq!(sup.brains.len(), 2, "both NICs got a brain");
        assert!(sup.brains.contains_key(&1) && sup.brains.contains_key(&2));
        assert_eq!(
            sup.gateways.get(&1).map(|dial| dial.gateway),
            Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1))),
            "the NIC with a gateway got a dial",
        );
        assert_eq!(sup.gateways.len(), 1, "the gateway-less NIC dialed nothing");

        let gw_b = |index: u32| match index {
            1 => Some(IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1))),
            3 => Some(IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 3))),
            _ => None,
        };
        sup.apply_reconcile(
            &discovery,
            &mut nics,
            std::vec![nic(1, 0x10), nic(3, 0x30)],
            gw_b,
        );
        assert!(
            !sup.brains.contains_key(&2),
            "the vanished NIC's brain was dropped",
        );
        assert!(sup.brains.contains_key(&3), "the new NIC got a brain");
        assert_eq!(
            sup.gateways.get(&1).map(|dial| dial.gateway),
            Some(IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1))),
            "the surviving NIC's gateway was re-pointed after the roam",
        );
        assert!(
            sup.gateways.contains_key(&3),
            "the new NIC's gateway was dialed",
        );
        assert!(
            !sup.gateways.contains_key(&2),
            "the vanished NIC's gateway dial was torn down",
        );

        sup.apply_reconcile(&discovery, &mut nics, std::vec![], |_| None);
        assert!(
            sup.brains.is_empty(),
            "the brains drained when the link left"
        );
        assert!(sup.gateways.is_empty(), "every gateway dial was torn down");
        assert!(nics.is_empty());
    }

    #[tokio::test]
    async fn mdns_sightings_dial_each_peer_once_and_never_dial_ourselves() {
        use std::net::Ipv4Addr;
        let (mut sup, _guard) = test_supervisor();
        sup.prefixes = std::vec![prefix([192, 168, 1, 50], [255, 255, 255, 0])];

        let peer = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 77)),
            core::TCP_RENDEZVOUS_PORT,
        );
        sup.dial_mdns_sighting(peer);
        assert_eq!(sup.mdns_dials.len(), 1, "a fresh sighting becomes a dial");

        sup.dial_mdns_sighting(peer);
        assert_eq!(
            sup.mdns_dials.len(),
            1,
            "a repeat sighting does not stack a second dial",
        );

        let ours = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
            core::TCP_RENDEZVOUS_PORT,
        );
        sup.dial_mdns_sighting(ours);
        assert_eq!(
            sup.mdns_dials.len(),
            1,
            "we never dial our own advertised rendezvous",
        );
    }

    #[test]
    fn the_card_is_dormant_or_live_but_never_failed() {
        let id = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, core::GROUP_ID);
        let status = AutoWifiStatus::new(id);

        assert_eq!(
            status.connection(),
            ConnectionState::Disconnected,
            "a fresh supervisor with no peers reads Dormant, not Failed",
        );

        status.publish(2, 0, 0, std::vec::Vec::new());
        assert_eq!(
            status.connection(),
            ConnectionState::Connected,
            "a peer touched makes the card Live",
        );

        status.publish(0, 0, 0, std::vec::Vec::new());
        assert_eq!(
            status.connection(),
            ConnectionState::Disconnected,
            "losing the last peer returns to Dormant, never Failed",
        );
    }

    /// A hand-driven seam, as in the UDP tests: captures every `next_inbound`, supplies
    /// `next_outbound` from a grant lane the test fills.
    struct MockSeam {
        inbound: mpsc::UnboundedSender<std::vec::Vec<u8>>,
        sink: std::vec::Vec<u8>,
        outbound: TokioGrantConsumer,
    }

    use prns_core::interfaces::FrameSink;

    impl InterfaceSeam for MockSeam {
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

        let (demux_tx, demux_rx) = mpsc::channel::<std::vec::Vec<u8>>(PEER_INBOUND_DEPTH);
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
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };
        tokio::spawn(member.run(seam));

        // Inbound: the supervisor would demux a datagram from this peer to the member's channel; the
        // test plays the supervisor and pushes one directly. The member hands it up the seam whole.
        let payload = [0x7Eu8, 0x01, 0x7D, 0x02, 0x7E];
        demux_tx
            .try_send(payload.to_vec())
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
