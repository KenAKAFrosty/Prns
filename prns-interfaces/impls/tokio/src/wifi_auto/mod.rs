mod mdns;

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc::{self, Receiver, Sender, UnboundedReceiver};
use tokio::sync::watch;

pub use crate::network_device::AutoWifiDevicePolicy;
use crate::reconnect::ReconnectPolicy;
use crate::tcp::{tune, TcpClientInterface, TcpServerConnection};
use prns_core::engine::InstantMillis;
use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
    InterfaceStatus, TransferRates,
};
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
const UNICAST_REPEER_EVERY: u32 = 3;
const RENDEZVOUS_RECONNECT: ReconnectPolicy = ReconnectPolicy::STANDARD;
/// Reclaim the rendezvous port every three beacon cycles without attempting a bind on every beacon.
const REBIND_BEACON_CYCLES: u32 = 3;
/// A full peer lane drops new datagrams rather than allowing one peer to grow process memory.
const PEER_INBOUND_DEPTH: usize = 32;

pub struct AutoWifiPeer {
    id: InterfaceId,
    socket: Arc<UdpSocket>,
    peer: SocketAddrV6,
    inbound: Receiver<std::vec::Vec<u8>>,
    policy: EffectiveInterfacePolicy,
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl AutoWifiPeer {
    /// EUI-64 keeps the id stable across a peer reconnect so it rebinds its routes.
    pub fn new(
        socket: Arc<UdpSocket>,
        peer: SocketAddrV6,
        inbound: Receiver<std::vec::Vec<u8>>,
        bitrate: BitrateBps,
    ) -> Self {
        Self::with_policy(socket, peer, inbound, contract::policy_for_bitrate(bitrate))
    }

    pub fn with_policy(
        socket: Arc<UdpSocket>,
        peer: SocketAddrV6,
        inbound: Receiver<std::vec::Vec<u8>>,
        policy: EffectiveInterfacePolicy,
    ) -> Self {
        Self::with_policy_and_instance_tag(socket, peer, inbound, policy, &[])
    }

    fn with_policy_and_instance_tag(
        socket: Arc<UdpSocket>,
        peer: SocketAddrV6,
        inbound: Receiver<std::vec::Vec<u8>>,
        policy: EffectiveInterfacePolicy,
        instance_tag: &[u8],
    ) -> Self {
        let mut channel_tag = if instance_tag.is_empty() {
            std::vec::Vec::new()
        } else {
            let mut tag = (instance_tag.len() as u64).to_be_bytes().to_vec();
            tag.extend_from_slice(instance_tag);
            tag
        };
        channel_tag.extend_from_slice(&peer.ip().octets());
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
    const HW_MTU: usize = contract::WIFI_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::WifiPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        contract::descriptor(self.id, self.policy)
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
                    if outbound.is_empty() || outbound.len() > contract::HARDWARE_MTU {
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

pub struct AutoWifi {
    policy: EffectiveInterfacePolicy,
    settings: AutoWifiSettings,
    status: AutoWifiStatus,
    mdns: Option<UnboundedReceiver<SocketAddr>>,
    rendezvous_listener: Option<TcpListener>,
    host_network_discovery: HostNetworkDiscovery,
}

enum HostNetworkDiscovery {
    Enumerated,
    PlatformRendezvous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoWifiSettingsError {
    InvalidDiscoveryPort,
    InvalidDataPort,
}

impl std::fmt::Display for AutoWifiSettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDiscoveryPort => {
                formatter.write_str("AutoInterface discovery port must be between 1 and 65534")
            }
            Self::InvalidDataPort => {
                formatter.write_str("AutoInterface data port must be between 1 and 65535")
            }
        }
    }
}

impl std::error::Error for AutoWifiSettingsError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoWifiSettings {
    instance_tag: std::vec::Vec<u8>,
    group_id: std::vec::Vec<u8>,
    discovery_scope: contract::DiscoveryScope,
    multicast_address_type: contract::MulticastAddressType,
    discovery_port: u16,
    data_port: u16,
    devices: AutoWifiDevicePolicy,
    mdns_find: bool,
}

impl AutoWifiSettings {
    pub fn new(
        group_id: impl Into<std::vec::Vec<u8>>,
        discovery_scope: contract::DiscoveryScope,
        multicast_address_type: contract::MulticastAddressType,
        discovery_port: u16,
        data_port: u16,
        devices: AutoWifiDevicePolicy,
    ) -> Result<Self, AutoWifiSettingsError> {
        if discovery_port == 0 || discovery_port == u16::MAX {
            return Err(AutoWifiSettingsError::InvalidDiscoveryPort);
        }
        if data_port == 0 {
            return Err(AutoWifiSettingsError::InvalidDataPort);
        }
        let group_id = group_id.into();
        Ok(Self {
            instance_tag: group_id.clone(),
            group_id,
            discovery_scope,
            multicast_address_type,
            discovery_port,
            data_port,
            devices,
            mdns_find: false,
        })
    }

    pub fn group_id(&self) -> &[u8] {
        &self.group_id
    }

    pub fn with_instance_tag(mut self, instance_tag: impl Into<std::vec::Vec<u8>>) -> Self {
        self.instance_tag = instance_tag.into();
        self
    }

    pub const fn discovery_scope(&self) -> contract::DiscoveryScope {
        self.discovery_scope
    }

    pub const fn multicast_address_type(&self) -> contract::MulticastAddressType {
        self.multicast_address_type
    }

    pub const fn discovery_port(&self) -> u16 {
        self.discovery_port
    }

    pub const fn data_port(&self) -> u16 {
        self.data_port
    }

    pub const fn devices(&self) -> &AutoWifiDevicePolicy {
        &self.devices
    }

    #[must_use]
    pub fn with_mdns_find(mut self) -> Self {
        self.mdns_find = true;
        self
    }

    #[must_use]
    pub const fn mdns_find(&self) -> bool {
        self.mdns_find
    }

    fn discovery_group(&self) -> Ipv6Addr {
        contract::discovery_group(
            &self.group_id,
            self.discovery_scope,
            self.multicast_address_type,
        )
    }

    const fn reverse_discovery_port(&self) -> u16 {
        self.discovery_port + 1
    }

    fn prns_rendezvous_enabled(&self) -> bool {
        self.group_id == contract::GROUP_ID
            && self.discovery_scope == contract::DiscoveryScope::Link
            && self.multicast_address_type == contract::MulticastAddressType::Temporary
            && self.discovery_port == contract::DEFAULT_DISCOVERY_PORT
            && self.data_port == contract::DEFAULT_DATA_PORT
    }
}

impl Default for AutoWifiSettings {
    fn default() -> Self {
        Self {
            instance_tag: contract::GROUP_ID.to_vec(),
            group_id: contract::GROUP_ID.to_vec(),
            discovery_scope: contract::DiscoveryScope::Link,
            multicast_address_type: contract::MulticastAddressType::Temporary,
            discovery_port: contract::DEFAULT_DISCOVERY_PORT,
            data_port: contract::DEFAULT_DATA_PORT,
            devices: AutoWifiDevicePolicy::default(),
            mdns_find: false,
        }
    }
}

impl AutoWifi {
    #[must_use]
    pub fn new() -> Self {
        Self::with_policy(contract::configured_policy(Default::default()))
    }

    #[must_use]
    pub fn with_bitrate(bitrate: BitrateBps) -> Self {
        Self::with_policy(contract::policy_for_bitrate(bitrate))
    }

    #[must_use]
    pub fn with_policy(policy: EffectiveInterfacePolicy) -> Self {
        Self::with_policy_and_settings(policy, AutoWifiSettings::default())
    }

    #[must_use]
    pub fn with_policy_and_settings(
        policy: EffectiveInterfacePolicy,
        settings: AutoWifiSettings,
    ) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, &settings.instance_tag);
        let mdns_find = settings.mdns_find();
        Self {
            policy,
            settings,
            status: AutoWifiStatus::new(id, mdns_find),
            mdns: None,
            rendezvous_listener: None,
            host_network_discovery: HostNetworkDiscovery::Enumerated,
        }
    }

    #[must_use]
    pub fn with_mdns_find(mut self) -> Self {
        self.settings.mdns_find = true;
        self.status.set_mdns_find(true);
        self
    }

    /// Bonjour / mDNS sightings feed UDP unicast probes on the reverse discovery port.
    #[must_use]
    pub fn with_mdns_sightings(mut self, sightings: UnboundedReceiver<SocketAddr>) -> Self {
        self.settings.mdns_find = true;
        self.status.set_mdns_find(true);
        self.mdns = Some(sightings);
        self
    }

    /// Compatibility alias for [`Self::with_mdns_sightings`].
    #[must_use]
    pub fn with_mdns(self, sightings: UnboundedReceiver<SocketAddr>) -> Self {
        self.with_mdns_sightings(sightings)
    }

    #[must_use]
    pub fn without_host_network_discovery(mut self) -> Self {
        self.host_network_discovery = HostNetworkDiscovery::PlatformRendezvous;
        self
    }

    /// Supplies a rendezvous listener that was bound during an outer lifecycle's startup
    /// transaction. This lets applications prove the local port is owned before publishing a
    /// running state, while retaining AutoWifi's normal accept and rebind behavior afterwards.
    #[must_use]
    pub fn with_rendezvous_listener(mut self, listener: TcpListener) -> Self {
        self.rendezvous_listener = Some(listener);
        self
    }

    #[must_use]
    pub fn with_platform_rendezvous(mut self, sightings: UnboundedReceiver<SocketAddr>) -> Self {
        self.settings.mdns_find = true;
        self.status.set_mdns_find(true);
        self.mdns = Some(sightings);
        self.host_network_discovery = HostNetworkDiscovery::PlatformRendezvous;
        self
    }

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

#[derive(Clone)]
pub struct AutoWifiStatus {
    shared: Arc<AutoWifiShared>,
}

struct AutoWifiShared {
    id: InterfaceId,
    enabled: watch::Sender<bool>,
    mdns_find: AtomicBool,
    accounting: Mutex<AutoWifiAccounting>,
}

#[derive(Default)]
struct CompletedTraffic {
    rx: u64,
    tx: u64,
}

struct AutoWifiAccounting {
    completed: CompletedTraffic,
    members: std::vec::Vec<TokioInterfaceStatus>,
}

impl CompletedTraffic {
    fn retain(&mut self, member: &TokioInterfaceStatus) {
        self.rx = self.rx.saturating_add(member.rx_bytes());
        self.tx = self.tx.saturating_add(member.tx_bytes());
    }
}

impl AutoWifiStatus {
    fn new(id: InterfaceId, mdns_find: bool) -> Self {
        let (enabled, _) = watch::channel(true);
        Self {
            shared: Arc::new(AutoWifiShared {
                id,
                enabled,
                mdns_find: AtomicBool::new(mdns_find),
                accounting: Mutex::new(AutoWifiAccounting {
                    completed: CompletedTraffic::default(),
                    members: std::vec::Vec::new(),
                }),
            }),
        }
    }

    fn set_mdns_find(&self, mdns_find: bool) {
        self.shared.mdns_find.store(mdns_find, Ordering::Relaxed);
    }

    pub fn enable(&self) {
        self.update_enabled(true);
    }

    pub fn disable(&self) {
        self.update_enabled(false);
    }

    pub fn toggle_enabled(&self) {
        self.shared.enabled.send_if_modified(|current| {
            *current = !*current;
            true
        });
    }

    fn update_enabled(&self, enabled: bool) {
        self.shared.enabled.send_if_modified(|current| {
            let changed = *current != enabled;
            *current = enabled;
            changed
        });
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        *self.shared.enabled.borrow()
    }

    async fn wait_until_enabled(&self) {
        self.wait_for_enabled_state(true).await;
    }

    async fn wait_until_disabled(&self) {
        self.wait_for_enabled_state(false).await;
    }

    async fn wait_for_enabled_state(&self, enabled: bool) {
        let mut changed = self.shared.enabled.subscribe();
        let _ = changed.wait_for(|current| *current == enabled).await;
    }

    fn publish(&self, completed: &CompletedTraffic, members: std::vec::Vec<TokioInterfaceStatus>) {
        if let Ok(mut accounting) = self.shared.accounting.lock() {
            accounting.completed.rx = completed.rx;
            accounting.completed.tx = completed.tx;
            accounting.members = members;
        }
    }

    #[must_use]
    pub fn members(&self) -> std::vec::Vec<TokioInterfaceStatus> {
        match self.shared.accounting.lock() {
            Ok(accounting) => accounting.members.clone(),
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
            return ConnectionState::Disabled;
        }
        self.shared
            .accounting
            .lock()
            .ok()
            .and_then(|accounting| {
                accounting
                    .members
                    .iter()
                    .map(InterfaceStatus::connection)
                    .min_by_key(|state| auto_wifi_connection_rank(*state))
            })
            .unwrap_or(ConnectionState::Disconnected)
    }

    fn rx_bytes(&self) -> u64 {
        self.shared
            .accounting
            .lock()
            .map(|accounting| {
                accounting
                    .members
                    .iter()
                    .fold(accounting.completed.rx, |total, member| {
                        total.saturating_add(member.rx_bytes())
                    })
            })
            .unwrap_or(0)
    }

    fn tx_bytes(&self) -> u64 {
        self.shared
            .accounting
            .lock()
            .map(|accounting| {
                accounting
                    .members
                    .iter()
                    .fold(accounting.completed.tx, |total, member| {
                        total.saturating_add(member.tx_bytes())
                    })
            })
            .unwrap_or(0)
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        let accounting = self.shared.accounting.lock().ok()?;
        accounting
            .members
            .iter()
            .filter_map(InterfaceStatus::transfer_rates)
            .reduce(|acc, rates| TransferRates {
                rx_bps: acc.rx_bps.saturating_add(rates.rx_bps),
                tx_bps: acc.tx_bps.saturating_add(rates.tx_bps),
            })
    }

    fn mdns_find(&self) -> Option<bool> {
        Some(self.shared.mdns_find.load(Ordering::Relaxed))
    }
}

fn auto_wifi_connection_rank(state: ConnectionState) -> u8 {
    match state {
        ConnectionState::Connected => 0,
        ConnectionState::Degraded => 1,
        ConnectionState::Initializing => 2,
        ConnectionState::Reconnecting => 3,
        ConnectionState::Failed => 4,
        ConnectionState::Disconnected => 5,
        ConnectionState::Disabled => 6,
        ConnectionState::Unknown => 7,
    }
}

impl InterfaceSupervisor for AutoWifi {
    const KIND: InterfaceKind = InterfaceKind::AutoWifi;

    fn channel_tag(&self) -> &[u8] {
        &self.settings.instance_tag
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        // RNS AutoInterface is multi-interface: each eligible link-local NIC owns its token and peer table, and inbound datagrams demultiplex by source scope. Multicast is best-effort so rendezvous, gateway, and mDNS still run when it is unavailable.
        let AutoWifi {
            policy,
            settings,
            status,
            mdns,
            rendezvous_listener,
            host_network_discovery,
        } = self;
        let enumerate_host_network =
            matches!(host_network_discovery, HostNetworkDiscovery::Enumerated);
        let mut nics = if enumerate_host_network {
            link_local_nics(&settings.devices)
        } else {
            std::vec::Vec::new()
        };
        let sockets = if nics.is_empty() {
            None
        } else {
            open_sockets(&nics, &settings).ok()
        };
        let prefixes = if enumerate_host_network {
            local_prefixes(&settings.devices)
        } else {
            std::vec::Vec::new()
        };
        let mut sup = Supervisor {
            brains: if sockets.is_some() {
                nics.iter()
                    .map(|nic| {
                        (
                            nic.index,
                            contract::HeapAutoInterfaceProtocol::from_link_local_with_group(
                                nic.link_local,
                                &settings.group_id,
                            ),
                        )
                    })
                    .collect()
            } else {
                HashMap::new()
            },
            members: HashMap::new(),
            gateways: HashMap::new(),
            accepted: std::vec::Vec::new(),
            prefixes,
            fleet,
            data: sockets.as_ref().map(|s| s.data.clone()),
            policy,
            settings: settings.clone(),
            status,
            completed: CompletedTraffic::default(),
        };
        if enumerate_host_network {
            sup.dial_initial_gateways(&nics);
        }
        sup.publish_status();

        let mut mdns = mdns.or_else(|| {
            settings
                .mdns_find
                .then(|| mdns::start(&settings.instance_tag, settings.devices.clone()))
        });
        // Bonjour often emits each peer once; remember sightings and keep probing until
        // multicast/reverse peering installs them as known members.
        let mut mdns_targets: HashMap<Ipv6Addr, u32> = HashMap::new();
        let started = tokio::time::Instant::now();
        let mut beacon = tokio::time::interval(BEACON_INTERVAL);
        let mut beacon_cycle: u32 = 0;
        let mut discovery_buf = [0u8; 64];
        let mut unicast_buf = [0u8; 64];
        let mut data_buf = [0u8; contract::HARDWARE_MTU];
        let mut rendezvous = if settings.prns_rendezvous_enabled() {
            match rendezvous_listener {
                Some(listener) => Some(listener),
                None => TcpListener::bind(("0.0.0.0", contract::TCP_RENDEZVOUS_PORT))
                    .await
                    .ok(),
            }
        } else {
            None
        };
        let mut loopback = bounce_to_local_core(&settings, &rendezvous, &sup.fleet, sup.policy);

        loop {
            if !sup.status.is_enabled() {
                sup.disable_members();
                sup.status.wait_until_enabled().await;
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
                        Some(target) => {
                            if let SocketAddr::V6(v6) = target {
                                let ip = *v6.ip();
                                if ip.is_unicast_link_local()
                                    && !is_own_address(IpAddr::V6(ip), &sup.prefixes)
                                    && !sup
                                        .brains
                                        .values()
                                        .any(|brain| brain.our_link_local() == ip)
                                {
                                    let scope = if v6.scope_id() != 0 {
                                        v6.scope_id()
                                    } else {
                                        0
                                    };
                                    mdns_targets.insert(ip, scope);
                                    if let Some(sockets) = sockets.as_ref() {
                                        probe_mdns_peer(
                                            sockets,
                                            &sup.brains,
                                            settings.reverse_discovery_port(),
                                            target,
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                        None => mdns = None,
                    }
                }
                () = sup.status.wait_until_disabled() => {}
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
                                    scoped(
                                        settings.discovery_group(),
                                        settings.discovery_port,
                                        index,
                                    ),
                                )
                                .await;
                            if repeer {
                                for addr in brain.known_peer_addresses() {
                                    let _ = sockets
                                        .unicast_discovery
                                        .send_to(
                                            &token,
                                            scoped(
                                                addr,
                                                settings.reverse_discovery_port(),
                                                index,
                                            ),
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
                    // Bonjour often emits once; keep probing remembered finds whenever they
                    // are not currently members so a later retire can re-peer without a fresh
                    // mDNS sighting.
                    if repeer {
                        if let Some(sockets) = sockets.as_ref() {
                            for (&ip, &scope) in &mdns_targets {
                                if sup.members.contains_key(&ip) {
                                    continue;
                                }
                                let target = SocketAddr::V6(SocketAddrV6::new(ip, 0, 0, scope));
                                probe_mdns_peer(
                                    sockets,
                                    &sup.brains,
                                    settings.reverse_discovery_port(),
                                    target,
                                )
                                .await;
                            }
                        }
                    }
                    sup.publish_status();
                    reclaim_port =
                        settings.prns_rendezvous_enabled()
                            && rendezvous.is_none()
                            && beacon_cycle.is_multiple_of(REBIND_BEACON_CYCLES);
                }
            }
            if reclaim_port {
                if let Ok(listener) =
                    TcpListener::bind(("0.0.0.0", contract::TCP_RENDEZVOUS_PORT)).await
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
    brains: HashMap<u32, contract::HeapAutoInterfaceProtocol>,
    members: HashMap<Ipv6Addr, PeerMember>,
    gateways: HashMap<u32, GatewayDial>,
    accepted: std::vec::Vec<AttachedStatus>,
    prefixes: std::vec::Vec<LocalPrefix>,
    fleet: Fleet,
    data: Option<Arc<UdpSocket>>,
    policy: EffectiveInterfacePolicy,
    settings: AutoWifiSettings,
    status: AutoWifiStatus,
    completed: CompletedTraffic,
}

struct GatewayDial {
    gateway: IpAddr,
    attached: AttachedInterface,
    status: TokioInterfaceStatus,
}

struct GatewayInventoryUnavailable;

type GatewayInventory = Result<HashMap<u32, IpAddr>, GatewayInventoryUnavailable>;

impl Supervisor {
    fn ingest_beacon(&mut self, src: SocketAddr, bytes: &[u8], now_ms: u64) {
        let SocketAddr::V6(v6) = src else { return };
        let scope = v6.scope_id();
        let Some(brain) = self.brains.get_mut(&scope) else {
            return;
        };
        if let contract::BeaconVerdict::Peer(addr) =
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
        let peer = SocketAddrV6::new(addr, self.settings.data_port, 0, scope);
        let member = AutoWifiPeer::with_policy_and_instance_tag(
            data,
            peer,
            inbound_rx,
            self.policy,
            &self.settings.instance_tag,
        );
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
                self.completed.retain(&member.status);
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
        self.status.publish(&self.completed, statuses);
    }

    fn disable_members(&mut self) {
        for (_, member) in self.members.drain() {
            self.completed.retain(&member.status);
            member.attached.teardown();
        }
        for (_, dial) in self.gateways.drain() {
            self.completed.retain(&dial.status);
            dial.attached.teardown();
        }
        for member in self.accepted.drain(..) {
            self.completed.retain(&member.status);
            member.attached.teardown();
        }
        self.status.publish(&self.completed, std::vec::Vec::new());
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
                self.completed.retain(&member.status);
                member.attached.teardown();
            }
        }
        self.publish_status();
    }

    fn dial_initial_gateways(&mut self, nics: &[Nic]) {
        let Ok(routes) = platform_gateway_inventory() else {
            return;
        };
        for nic in nics {
            self.refresh_gateway(nic.index, gateway_for(&routes, nic.index));
        }
    }

    fn refresh_gateway(&mut self, index: u32, gateway: Option<IpAddr>) {
        if !self.settings.prns_rendezvous_enabled() {
            if let Some(old) = self.gateways.remove(&index) {
                self.completed.retain(&old.status);
                old.attached.teardown();
            }
            return;
        }
        if self.gateways.get(&index).map(|dial| dial.gateway) == gateway {
            return;
        }
        if let Some(old) = self.gateways.remove(&index) {
            crate::diagnostic_log::debug!(
                "wifi-auto: gateway rendezvous removed on ifindex {index} ({})",
                old.gateway
            );
            self.completed.retain(&old.status);
            old.attached.teardown();
        }
        if let Some(gateway) = gateway {
            let target = SocketAddr::new(gateway, contract::TCP_RENDEZVOUS_PORT).to_string();
            crate::diagnostic_log::debug!(
                "wifi-auto: dialing gateway rendezvous {target} on ifindex {index}"
            );
            let client = TcpClientInterface::with_policy(target, self.policy, RENDEZVOUS_RECONNECT);
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
        let fresh = link_local_nics(&self.settings.devices);
        let gateways = platform_gateway_inventory();
        self.prefixes = local_prefixes(&self.settings.devices);
        self.apply_reconcile(discovery, nics, fresh, gateways);
    }

    fn apply_reconcile(
        &mut self,
        discovery: &UdpSocket,
        nics: &mut std::vec::Vec<Nic>,
        fresh: std::vec::Vec<Nic>,
        gateways: GatewayInventory,
    ) {
        let plan = plan_reconcile(nics, &fresh);
        let discovery_group = self.settings.discovery_group();
        for index in plan.removed {
            let _ = discovery.leave_multicast_v6(&discovery_group, index);
            self.brains.remove(&index);
            if let Some(dial) = self.gateways.remove(&index) {
                self.completed.retain(&dial.status);
                dial.attached.teardown();
            }
        }
        for nic in plan.added {
            let _ = discovery.join_multicast_v6(&discovery_group, nic.index);
            self.brains.insert(
                nic.index,
                contract::HeapAutoInterfaceProtocol::from_link_local_with_group(
                    nic.link_local,
                    &self.settings.group_id,
                ),
            );
        }
        for nic in plan.rebound {
            self.brains.insert(
                nic.index,
                contract::HeapAutoInterfaceProtocol::from_link_local_with_group(
                    nic.link_local,
                    &self.settings.group_id,
                ),
            );
        }
        if let Ok(routes) = gateways {
            for nic in &fresh {
                self.refresh_gateway(nic.index, gateway_for(&routes, nic.index));
            }
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

/// The per-scope probe obtains the kernel's send-source link-local address so the beacon token matches what peers recompute from the datagram source.
fn link_local_nics(devices: &AutoWifiDevicePolicy) -> std::vec::Vec<Nic> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return std::vec::Vec::new();
    };
    let mut nics = std::vec::Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for iface in ifaces {
        // Some platforms omit a NIC's link-local address from if_addrs, so the per-scope probe is authoritative and each interface is considered once by index.
        if !devices.allows(&iface.name, iface.is_loopback()) {
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

fn local_prefixes(devices: &AutoWifiDevicePolicy) -> std::vec::Vec<LocalPrefix> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return std::vec::Vec::new();
    };
    ifaces
        .iter()
        .filter(|iface| devices.allows(&iface.name, iface.is_loopback()))
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

fn gateway_for(routes: &HashMap<u32, IpAddr>, index: u32) -> Option<IpAddr> {
    routes.get(&index).copied()
}

#[cfg(not(target_os = "ios"))]
fn platform_gateway_inventory() -> GatewayInventory {
    Ok(netdev::get_interfaces()
        .iter()
        .filter_map(|interface| gateway_addr(interface).map(|address| (interface.index, address)))
        .collect())
}

#[cfg(target_os = "ios")]
fn platform_gateway_inventory() -> GatewayInventory {
    Err(GatewayInventoryUnavailable)
}

/// A node that cannot claim the local rendezvous port joins its current holder over loopback.
fn bounce_to_local_core(
    settings: &AutoWifiSettings,
    rendezvous: &Option<TcpListener>,
    fleet: &Fleet,
    policy: EffectiveInterfacePolicy,
) -> Option<AttachedInterface> {
    if !settings.prns_rendezvous_enabled() || rendezvous.is_some() {
        return None;
    }
    let target = std::format!("127.0.0.1:{}", contract::TCP_RENDEZVOUS_PORT);
    Some(fleet.add(TcpClientInterface::with_policy(
        target,
        policy,
        RENDEZVOUS_RECONNECT,
    )))
}

async fn accept_maybe(listener: &Option<TcpListener>) -> io::Result<(TcpStream, SocketAddr)> {
    match listener {
        Some(listener) => listener.accept().await,
        None => std::future::pending().await,
    }
}

async fn recv_maybe(socket: Option<&UdpSocket>, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
    match socket {
        Some(socket) => socket.recv_from(buf).await.ok(),
        None => std::future::pending().await,
    }
}

async fn next_mdns(sightings: Option<&mut UnboundedReceiver<SocketAddr>>) -> Option<SocketAddr> {
    match sightings {
        Some(sightings) => sightings.recv().await,
        None => std::future::pending().await,
    }
}

async fn probe_mdns_peer(
    sockets: &Sockets,
    brains: &HashMap<u32, contract::HeapAutoInterfaceProtocol>,
    reverse_port: u16,
    target: SocketAddr,
) {
    let SocketAddr::V6(v6) = target else {
        return;
    };
    let ip = *v6.ip();
    if !ip.is_unicast_link_local() {
        return;
    }
    let scopes: std::vec::Vec<u32> = if v6.scope_id() != 0 {
        vec![v6.scope_id()]
    } else {
        brains.keys().copied().collect()
    };
    for scope in scopes {
        let Some(brain) = brains.get(&scope) else {
            continue;
        };
        let token = *brain.our_peering_token().as_bytes();
        crate::diagnostic_log::debug!("wifi-auto: mDNS found {ip}%{scope}, probing UDP unicast");
        let _ = sockets
            .unicast_discovery
            .send_to(&token, scoped(ip, reverse_port, scope))
            .await;
    }
}

/// Prefer an IPv4 default gateway, then IPv6; a hosted AP with no default route is skipped.
#[cfg(not(target_os = "ios"))]
fn gateway_addr(iface: &netdev::Interface) -> Option<IpAddr> {
    let gateway = iface.gateway.as_ref()?;
    gateway
        .ipv4
        .first()
        .copied()
        .map(IpAddr::V4)
        .or_else(|| gateway.ipv6.first().copied().map(IpAddr::V6))
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

fn open_sockets(nics: &[Nic], settings: &AutoWifiSettings) -> io::Result<Sockets> {
    Ok(Sockets {
        discovery: discovery_socket(nics, settings)?,
        unicast_discovery: bound_v6(settings.reverse_discovery_port())?,
        data: Arc::new(bound_v6(settings.data_port)?),
    })
}

/// Joins every eligible interface and sends with an explicit scope rather than a default multicast interface.
fn discovery_socket(nics: &[Nic], settings: &AutoWifiSettings) -> io::Result<UdpSocket> {
    let socket = reusable_socket2(settings.discovery_port)?;
    let discovery_group = settings.discovery_group();
    // A failed multicast join is skipped so one interface cannot take AutoWifi down on every other interface.
    let joined = nics
        .iter()
        .filter(|nic| {
            socket
                .join_multicast_v6(&discovery_group, nic.index)
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
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};

    const TEST_FRAME_CAP: usize = 2_048;

    fn nic(index: u32, link_local_tail: u16) -> Nic {
        Nic {
            index,
            link_local: Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, link_local_tail),
        }
    }

    #[test]
    fn configured_device_allow_and_ignore_lists_have_stock_precedence() {
        let default = AutoWifiDevicePolicy::default();
        assert!(!default.allows("awdl0", false));
        assert!(default.allows("en0", false));

        let configured = AutoWifiDevicePolicy::new(
            std::vec![String::from("awdl0"), String::from("en0")],
            std::vec![String::from("en0")],
        );
        assert!(configured.allows("awdl0", false));
        assert!(!configured.allows("en0", false));
        assert!(!configured.allows("wlan0", false));
        assert!(!configured.allows("awdl0", true));
    }

    #[test]
    fn platform_rendezvous_replaces_host_network_discovery() {
        let (_, sightings) = tokio::sync::mpsc::unbounded_channel();
        let wifi = AutoWifi::new().with_platform_rendezvous(sightings);

        assert!(wifi.mdns.is_some());
        assert!(matches!(
            wifi.host_network_discovery,
            HostNetworkDiscovery::PlatformRendezvous
        ));
    }

    #[test]
    fn configured_auto_interface_settings_drive_the_rns_discovery_socket() {
        let settings = AutoWifiSettings::new(
            b"field-mesh".to_vec(),
            contract::DiscoveryScope::Site,
            contract::MulticastAddressType::Permanent,
            30_000,
            40_000,
            AutoWifiDevicePolicy::default(),
        )
        .expect("ports are valid");

        assert_eq!(
            settings.discovery_group(),
            contract::discovery_group(
                b"field-mesh",
                contract::DiscoveryScope::Site,
                contract::MulticastAddressType::Permanent,
            )
        );
        assert_eq!(settings.discovery_port(), 30_000);
        assert_eq!(settings.reverse_discovery_port(), 30_001);
        assert_eq!(settings.data_port(), 40_000);
        assert!(!settings.prns_rendezvous_enabled());

        let custom_port = AutoWifiSettings::new(
            contract::GROUP_ID.to_vec(),
            contract::DiscoveryScope::Link,
            contract::MulticastAddressType::Temporary,
            30_000,
            contract::DEFAULT_DATA_PORT,
            AutoWifiDevicePolicy::default(),
        )
        .expect("ports are valid");
        assert!(AutoWifiSettings::default().prns_rendezvous_enabled());
        assert!(!custom_port.prns_rendezvous_enabled());
    }

    #[tokio::test]
    async fn configured_auto_interface_instances_cannot_collapse_into_one_runtime_identity() {
        let policy = contract::configured_policy(Default::default());
        let first_settings = AutoWifiSettings::default().with_instance_tag(b"LAN one".to_vec());
        let second_settings = AutoWifiSettings::default().with_instance_tag(b"LAN two".to_vec());
        let first = AutoWifi::with_policy_and_settings(policy, first_settings.clone());
        let second = AutoWifi::with_policy_and_settings(policy, second_settings.clone());
        assert_ne!(first.status().id(), second.status().id());

        let socket = Arc::new(UdpSocket::bind("[::1]:0").await.expect("bind peer socket"));
        let peer = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 42_671, 0, 0);
        let (_, first_inbound) = mpsc::channel(1);
        let (_, second_inbound) = mpsc::channel(1);
        let first_peer = AutoWifiPeer::with_policy_and_instance_tag(
            Arc::clone(&socket),
            peer,
            first_inbound,
            policy,
            &first_settings.instance_tag,
        );
        let second_peer = AutoWifiPeer::with_policy_and_instance_tag(
            socket,
            peer,
            second_inbound,
            policy,
            &second_settings.instance_tag,
        );
        assert_ne!(first_peer.id(), second_peer.id());
    }

    #[test]
    fn auto_interface_settings_reject_ports_the_runtime_cannot_represent() {
        let settings = |discovery_port, data_port| {
            AutoWifiSettings::new(
                contract::GROUP_ID.to_vec(),
                contract::DiscoveryScope::Link,
                contract::MulticastAddressType::Temporary,
                discovery_port,
                data_port,
                AutoWifiDevicePolicy::default(),
            )
        };

        assert_eq!(
            settings(0, contract::DEFAULT_DATA_PORT),
            Err(AutoWifiSettingsError::InvalidDiscoveryPort)
        );
        assert_eq!(
            settings(u16::MAX, contract::DEFAULT_DATA_PORT),
            Err(AutoWifiSettingsError::InvalidDiscoveryPort)
        );
        assert_eq!(
            settings(contract::DEFAULT_DISCOVERY_PORT, 0),
            Err(AutoWifiSettingsError::InvalidDataPort)
        );
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
        let id = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, contract::GROUP_ID);
        let settings = AutoWifiSettings::default();
        let (fleet, guard) = Fleet::detached(id);
        let data = std::net::UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).expect("bind data socket");
        data.set_nonblocking(true).expect("nonblocking");
        let sup = Supervisor {
            brains: HashMap::new(),
            members: HashMap::new(),
            gateways: HashMap::new(),
            accepted: std::vec::Vec::new(),
            prefixes: std::vec::Vec::new(),
            fleet,
            data: Some(Arc::new(UdpSocket::from_std(data).expect("into tokio"))),
            policy: contract::configured_policy(Default::default()),
            settings,
            status: AutoWifiStatus::new(id, false),
            completed: CompletedTraffic::default(),
        };
        (sup, guard)
    }

    fn gateway_inventory<const N: usize>(routes: [(u32, IpAddr); N]) -> GatewayInventory {
        Ok(routes.into_iter().collect())
    }

    #[tokio::test]
    async fn a_peer_inherits_the_auto_wifi_effective_policy() {
        let socket = Arc::new(UdpSocket::bind("[::1]:0").await.unwrap());
        let (_inbound_tx, inbound_rx) = mpsc::channel(1);
        let policy =
            contract::configured_policy(prns_core::interfaces::ConfiguredInterfacePolicy {
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
    async fn a_disconnected_multicast_member_does_not_make_the_aggregate_live() {
        let (mut sup, _guard) = test_supervisor();
        let peer = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x867c);

        sup.spawn_member(peer, 0);
        assert_eq!(sup.status.connection(), ConnectionState::Connected);

        let member = sup.members.get(&peer).expect("peer was registered");
        member.status.set_connection(ConnectionState::Disconnected);
        sup.publish_status();

        assert_eq!(
            sup.status.connection(),
            ConnectionState::Disconnected,
            "the aggregate follows the live child state",
        );
    }

    #[tokio::test]
    async fn a_disconnected_gateway_dial_does_not_make_the_aggregate_live() {
        use std::net::Ipv4Addr;
        let (mut sup, _guard) = test_supervisor();
        let gateway = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let client = TcpClientInterface::with_policy(
            SocketAddr::new(gateway, contract::TCP_RENDEZVOUS_PORT).to_string(),
            sup.policy,
            RENDEZVOUS_RECONNECT,
        );
        let status = client.status();
        let attached = sup.fleet.add(client);
        status.set_connection(ConnectionState::Disconnected);
        sup.gateways.insert(
            1,
            GatewayDial {
                gateway,
                attached,
                status,
            },
        );
        sup.publish_status();

        assert_eq!(
            sup.status.connection(),
            ConnectionState::Disconnected,
            "an unsuccessful gateway dial remains dormant",
        );
    }

    #[tokio::test]
    async fn reconcile_applies_nic_churn_and_repoints_gateways_against_a_real_fleet() {
        let (mut sup, _guard) = test_supervisor();
        let discovery = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0))
            .await
            .expect("bind discovery socket");
        let mut nics = std::vec::Vec::new();

        sup.apply_reconcile(
            &discovery,
            &mut nics,
            std::vec![nic(1, 0x10), nic(2, 0x20)],
            gateway_inventory([(1, IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1)))]),
        );
        assert_eq!(sup.brains.len(), 2, "both NICs got a brain");
        assert!(sup.brains.contains_key(&1) && sup.brains.contains_key(&2));
        assert_eq!(
            sup.gateways.get(&1).map(|dial| dial.gateway),
            Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1))),
            "the NIC with a gateway got a dial",
        );
        assert_eq!(sup.gateways.len(), 1, "the gateway-less NIC dialed nothing");

        sup.apply_reconcile(
            &discovery,
            &mut nics,
            std::vec![nic(1, 0x10), nic(3, 0x30)],
            gateway_inventory([
                (1, IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1))),
                (3, IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 3))),
            ]),
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

        sup.apply_reconcile(
            &discovery,
            &mut nics,
            std::vec![],
            Err(GatewayInventoryUnavailable),
        );
        assert!(
            sup.brains.is_empty(),
            "the brains drained when the link left"
        );
        assert!(sup.gateways.is_empty(), "every gateway dial was torn down");
        assert!(nics.is_empty());
    }

    #[test]
    fn mdns_find_only_accepts_foreign_link_local_peers() {
        let prefixes = [LocalPrefix {
            addr: IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x50)),
            netmask: IpAddr::V6(Ipv6Addr::from([
                0xff, 0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ])),
        }];
        let ours = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x50);
        let peer = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x77);
        let routed = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);

        assert!(
            peer.is_unicast_link_local() && !is_own_address(IpAddr::V6(peer), &prefixes),
            "a foreign link-local sighting is probeable",
        );
        assert!(
            is_own_address(IpAddr::V6(ours), &prefixes),
            "we never probe our own advertised address",
        );
        assert!(
            !routed.is_unicast_link_local(),
            "routable IPv6 sightings stay on the multicast path",
        );
    }

    #[tokio::test]
    async fn custom_group_nodes_never_dial_the_stock_loopback_rendezvous() {
        let (sup, _guard) = test_supervisor();
        let custom = AutoWifiSettings::new(
            b"field-mesh".to_vec(),
            contract::DiscoveryScope::Site,
            contract::MulticastAddressType::Permanent,
            30_000,
            40_000,
            AutoWifiDevicePolicy::default(),
        )
        .expect("ports are valid");
        assert!(
            bounce_to_local_core(&custom, &None, &sup.fleet, sup.policy).is_none(),
            "a node that opted out of the stock rendezvous has no holder to join",
        );

        let stock =
            bounce_to_local_core(&AutoWifiSettings::default(), &None, &sup.fleet, sup.policy)
                .expect("a stock node that lost the port joins its holder over loopback");
        stock.teardown();
    }

    #[test]
    fn the_parent_connection_follows_the_best_live_child_state() {
        let id = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, contract::GROUP_ID);
        let status = AutoWifiStatus::new(id, false);
        let first = TokioInterfaceStatus::new(
            InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"first"),
            ConnectionState::Initializing,
        );
        let second = TokioInterfaceStatus::new(
            InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"second"),
            ConnectionState::Disconnected,
        );
        let completed = CompletedTraffic::default();

        assert_eq!(status.connection(), ConnectionState::Disconnected);
        status.publish(&completed, vec![first.clone(), second.clone()]);
        assert_eq!(status.connection(), ConnectionState::Initializing);
        first.set_connection(ConnectionState::Connected);
        assert_eq!(status.connection(), ConnectionState::Connected);
        first.set_connection(ConnectionState::Disconnected);
        second.set_connection(ConnectionState::Reconnecting);
        assert_eq!(status.connection(), ConnectionState::Reconnecting);
        second.set_connection(ConnectionState::Disconnected);
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        status.disable();
        assert_eq!(status.connection(), ConnectionState::Disabled);
    }

    #[test]
    fn completed_and_reconnected_member_traffic_is_monotonic() {
        let id = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"accounting");
        let status = AutoWifiStatus::new(id, false);
        let first = TokioInterfaceStatus::new(
            InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"peer"),
            ConnectionState::Connected,
        );
        first.add_rx(90);
        first.add_tx(45);
        let mut completed = CompletedTraffic::default();

        status.publish(&completed, vec![first.clone()]);
        assert_eq!((status.rx_bytes(), status.tx_bytes()), (90, 45));
        completed.retain(&first);
        status.publish(&completed, vec![]);
        assert_eq!((status.rx_bytes(), status.tx_bytes()), (90, 45));

        let reconnected = TokioInterfaceStatus::new(
            InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"peer"),
            ConnectionState::Connected,
        );
        reconnected.add_rx(30);
        reconnected.add_tx(15);
        status.publish(&completed, vec![reconnected.clone()]);
        assert_eq!((status.rx_bytes(), status.tx_bytes()), (120, 60));
        completed.retain(&reconnected);
        status.publish(&completed, vec![]);
        assert_eq!((status.rx_bytes(), status.tx_bytes()), (120, 60));
    }

    struct MockSeam {
        inbound: mpsc::UnboundedSender<std::vec::Vec<u8>>,
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

    fn v6(addr: SocketAddr) -> SocketAddrV6 {
        match addr {
            SocketAddr::V6(a) => a,
            SocketAddr::V4(_) => unreachable!("bound an IPv6 socket"),
        }
    }

    #[tokio::test]
    async fn frames_ride_the_shared_socket_out_and_the_demux_channel_in() {
        let loopback = SocketAddr::from((Ipv6Addr::LOCALHOST, 0));

        let peer = UdpSocket::bind(loopback)
            .await
            .expect("binds the test peer");
        let peer_addr = v6(peer.local_addr().expect("the peer address is known"));

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
            contract::WIFI_BITRATE_GUESS_BPS,
        );

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };
        tokio::spawn(member.run(seam));

        let payload = [0x7Eu8, 0x01, 0x7D, 0x02, 0x7E];
        demux_tx
            .try_send(payload.to_vec())
            .expect("the member is receiving");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the member hands the frame up within the window")
            .expect("the member task is alive");
        assert_eq!(received, payload, "raw bytes, exactly as demuxed");

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
