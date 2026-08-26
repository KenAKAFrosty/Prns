use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::num::NonZeroU8;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::watch;

pub use crate::network_device::AutoWifiDevicePolicy;
use crate::reconnect::ReconnectPolicy;
use crate::tcp::{tune, TcpClientInterface, TcpServerConnection};
use prns_core::engine::InstantMillis;
use prns_core::interfaces::local_network::is_same_subnet;
use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::wifi_auto::{
    DiscoveryEndpoint, DiscoveryServiceName, DiscoverySnapshot, DiscoveryTransport,
};
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

#[cfg(all(
    feature = "wifi-auto-apple",
    any(target_os = "macos", target_os = "ios")
))]
mod apple;
mod discovery;
#[cfg(feature = "wifi-auto-mdns")]
mod mdns;
#[cfg(feature = "wifi-auto-mdns")]
mod publication_absence;

#[cfg(all(
    feature = "wifi-auto-apple",
    any(target_os = "macos", target_os = "ios")
))]
pub use apple::apple_service_discovery;
pub use discovery::{
    DiscoveryLifecycleError, DiscoveryParticipation, ServiceDiscovery, ServiceDiscoveryPublisher,
    SnapshotPublication,
};
#[cfg(feature = "wifi-auto-mdns")]
pub use mdns::native_service_discovery;

const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
const UNICAST_REPEER_EVERY: u32 = 3;
const RENDEZVOUS_RECONNECT: ReconnectPolicy = ReconnectPolicy::STANDARD;
const REBIND_BEACON_CYCLES: u32 = 3;
/// A full peer lane drops new datagrams rather than allowing one peer to grow process memory unbounded.
const PEER_INBOUND_DEPTH: usize = 32;
const TCP_RENDEZVOUS_ACCEPTED_CAPACITY: u8 = u8::MAX;
const UDP_PEER_CAPACITY: NonZeroU8 = contract::DEFAULT_DISCOVERY_SERVICE_CAPACITY;
type AutoWifiBrain = contract::FixedAutoInterfaceProtocol<{ UDP_PEER_CAPACITY.get() as usize }>;

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
        channel_tag.extend_from_slice(&peer.scope_id().to_be_bytes());
        let id = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, &channel_tag);
        Self {
            id,
            socket,
            peer,
            inbound,
            policy,
            channel_tag,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected),
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
    service_discovery: Option<ServiceDiscovery>,
    rendezvous_listener: Option<TcpListener>,
    network_discovery_owner: NetworkDiscoveryOwner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkDiscoveryOwner {
    Host,
    Platform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoInterfaceNetworkParticipation {
    Active,
    Dormant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RendezvousClaimSchedule {
    Due,
    Waiting,
}

impl RendezvousClaimSchedule {
    fn when_unbound(
        settings: &AutoWifiSettings,
        rendezvous_listener: &Option<TcpListener>,
    ) -> Self {
        match (
            settings.stock_service_discovery_enabled(),
            rendezvous_listener,
        ) {
            (true, None) => Self::Due,
            (false, _) | (true, Some(_)) => Self::Waiting,
        }
    }

    fn after_beacon(
        settings: &AutoWifiSettings,
        rendezvous_listener: &Option<TcpListener>,
        rebind_schedule: RebindSchedule,
    ) -> Self {
        match (
            Self::when_unbound(settings, rendezvous_listener),
            rebind_schedule,
        ) {
            (Self::Due, RebindSchedule::Due) => Self::Due,
            (Self::Due | Self::Waiting, RebindSchedule::Waiting)
            | (Self::Waiting, RebindSchedule::Due) => Self::Waiting,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebindSchedule {
    Due,
    Waiting,
}

impl RebindSchedule {
    fn for_beacon(beacon_cycle: u32) -> Self {
        if beacon_cycle.is_multiple_of(REBIND_BEACON_CYCLES) {
            Self::Due
        } else {
            Self::Waiting
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeeringRefresh {
    Due,
    Waiting,
}

impl PeeringRefresh {
    fn for_beacon(beacon_cycle: u32) -> Self {
        if beacon_cycle.is_multiple_of(UNICAST_REPEER_EVERY) {
            Self::Due
        } else {
            Self::Waiting
        }
    }
}

fn auto_interface_network_participation(
    settings: &AutoWifiSettings,
    discovery_participation: DiscoveryParticipation,
) -> AutoInterfaceNetworkParticipation {
    match (
        settings.stock_service_discovery_enabled(),
        discovery_participation,
    ) {
        (false, _) | (true, DiscoveryParticipation::Central) => {
            AutoInterfaceNetworkParticipation::Active
        }
        (true, DiscoveryParticipation::Inactive | DiscoveryParticipation::Satellite) => {
            AutoInterfaceNetworkParticipation::Dormant
        }
    }
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

    pub(crate) fn stock_service_discovery_enabled(&self) -> bool {
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
        Self {
            policy,
            settings,
            status: AutoWifiStatus::new(id),
            service_discovery: None,
            rendezvous_listener: None,
            network_discovery_owner: NetworkDiscoveryOwner::Host,
        }
    }

    #[must_use]
    pub fn with_host_discovery(mut self, service_discovery: ServiceDiscovery) -> Self {
        self.service_discovery = Some(service_discovery);
        self
    }

    /// Supplies a rendezvous listener that was bound during an outer lifecycle's startup transaction.
    ///
    /// This lets applications prove the local port is owned before publishing a running state,
    /// while retaining AutoWifi's normal accept and rebind behavior afterwards.
    #[must_use]
    pub fn with_rendezvous_listener(mut self, rendezvous_listener: TcpListener) -> Self {
        self.rendezvous_listener = Some(rendezvous_listener);
        self
    }

    #[must_use]
    pub fn with_platform_discovery(mut self, service_discovery: ServiceDiscovery) -> Self {
        self.service_discovery = Some(service_discovery);
        self.network_discovery_owner = NetworkDiscoveryOwner::Platform;
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
    member_updates: watch::Sender<std::vec::Vec<TokioInterfaceStatus>>,
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
    fn new(id: InterfaceId) -> Self {
        let (enabled, _) = watch::channel(true);
        let (member_updates, _) = watch::channel(std::vec::Vec::new());
        Self {
            shared: Arc::new(AutoWifiShared {
                id,
                enabled,
                member_updates,
                accounting: Mutex::new(AutoWifiAccounting {
                    completed: CompletedTraffic::default(),
                    members: std::vec::Vec::new(),
                }),
            }),
        }
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
        let members_changed = {
            let current = self.shared.member_updates.borrow();
            current.len() != members.len()
                || current
                    .iter()
                    .zip(&members)
                    .any(|(current, updated)| current.id() != updated.id())
        };
        if let Ok(mut accounting) = self.shared.accounting.lock() {
            accounting.completed.rx = completed.rx;
            accounting.completed.tx = completed.tx;
            accounting.members = members;
            if members_changed {
                self.shared
                    .member_updates
                    .send_replace(accounting.members.clone());
            }
        }
    }

    /// Subscribes to complete, deterministically ordered member-set replacements.
    #[must_use]
    pub fn subscribe_members(&self) -> watch::Receiver<std::vec::Vec<TokioInterfaceStatus>> {
        self.shared.member_updates.subscribe()
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
            mut service_discovery,
            rendezvous_listener: supplied_rendezvous_listener,
            network_discovery_owner,
        } = self;
        let mut nics = std::vec::Vec::new();
        let mut sockets = None;
        let prefixes = local_prefixes(&settings.devices);
        let mut supervisor = Supervisor {
            brains: HashMap::new(),
            members: HashMap::new(),
            gateways: HashMap::new(),
            discovered_services: DiscoveredServices::new(),
            loopback: None,
            accepted: std::vec::Vec::new(),
            prefixes,
            fleet,
            data: None,
            policy,
            settings: settings.clone(),
            status,
            completed: CompletedTraffic::default(),
        };
        supervisor.publish_status();

        let runtime_started_at = tokio::time::Instant::now();
        let mut beacon_interval = tokio::time::interval(BEACON_INTERVAL);
        let mut beacon_cycle: u32 = 0;
        let mut discovery_buffer = [0u8; 64];
        let mut unicast_discovery_buffer = [0u8; 64];
        let mut data_buffer = [0u8; contract::HARDWARE_MTU];
        let mut rendezvous_listener = settings
            .stock_service_discovery_enabled()
            .then_some(supplied_rendezvous_listener)
            .flatten();
        let mut discovery_participation = if rendezvous_listener.is_some() {
            DiscoveryParticipation::Central
        } else {
            DiscoveryParticipation::Inactive
        };
        let mut rendezvous_claim_schedule =
            RendezvousClaimSchedule::when_unbound(&settings, &rendezvous_listener);
        if let Some(service_discovery) = service_discovery.as_ref() {
            service_discovery.set_participation(discovery_participation);
        }
        if auto_interface_network_participation(&settings, discovery_participation)
            == AutoInterfaceNetworkParticipation::Active
        {
            match supervisor.ensure_network_active(&mut nics, &mut sockets, network_discovery_owner)
            {
                NetworkActivation::SocketUnavailable(error_kind) => {
                    crate::diagnostic_log::debug!(
                        "wifi-auto: UDP sockets unavailable ({error_kind:?}); retrying"
                    );
                }
                NetworkActivation::AlreadyActive
                | NetworkActivation::Activated
                | NetworkActivation::NoEligibleInterfaces => {}
            }
        }

        loop {
            if !supervisor.status.is_enabled() {
                discovery_participation = DiscoveryParticipation::Inactive;
                supervisor
                    .suspend_until_enabled(
                        &mut rendezvous_listener,
                        service_discovery.as_ref(),
                        &mut nics,
                        &mut sockets,
                        network_discovery_owner,
                    )
                    .await;
                rendezvous_claim_schedule =
                    RendezvousClaimSchedule::when_unbound(&settings, &rendezvous_listener);
                continue;
            }
            if rendezvous_claim_schedule == RendezvousClaimSchedule::Due {
                discovery_participation = supervisor.apply_rendezvous_claim(
                    claim_local_rendezvous().await,
                    &mut rendezvous_listener,
                    &mut nics,
                    &mut sockets,
                    network_discovery_owner,
                );
                if let Some(service_discovery) = service_discovery.as_ref() {
                    service_discovery.set_participation(discovery_participation);
                }
                rendezvous_claim_schedule = RendezvousClaimSchedule::Waiting;
            }
            tokio::select! {
                accepted_connection = accept_maybe(&rendezvous_listener) => {
                    supervisor.accept_rendezvous_connection(accepted_connection);
                }
                received_discovery_datagram = recv_maybe(
                    sockets
                        .as_ref()
                        .and_then(|sockets| sockets.discovery.as_ref()),
                    &mut discovery_buffer,
                ) => {
                    if let Some((received_bytes, source_address)) = received_discovery_datagram {
                        let elapsed_millis = runtime_started_at.elapsed().as_millis() as u64;
                        supervisor.ingest_beacon(
                            source_address,
                            &discovery_buffer[..received_bytes],
                            elapsed_millis,
                            BeaconChannel::Multicast,
                        );
                    }
                }
                received_unicast_datagram = recv_maybe(
                    sockets.as_ref().map(|sockets| &sockets.unicast_discovery),
                    &mut unicast_discovery_buffer,
                ) => {
                    if let Some((received_bytes, source_address)) = received_unicast_datagram {
                        let elapsed_millis = runtime_started_at.elapsed().as_millis() as u64;
                        let peering_token_reply = supervisor.ingest_beacon(
                            source_address,
                            &unicast_discovery_buffer[..received_bytes],
                            elapsed_millis,
                            BeaconChannel::Unicast,
                        );
                        send_peering_token_reply(
                            sockets.as_ref().map(|sockets| &sockets.unicast_discovery),
                            peering_token_reply,
                        ).await;
                    }
                }
                received_data_datagram = recv_maybe(
                    sockets.as_ref().map(|sockets| sockets.data.as_ref()),
                    &mut data_buffer,
                ) => {
                    if let Some((received_bytes, source_address)) = received_data_datagram {
                        supervisor.route_inbound(
                            source_address,
                            &data_buffer[..received_bytes],
                        );
                    }
                }
                discovery_snapshot = next_discovery_snapshot(
                    service_discovery.as_mut(),
                    discovery_participation,
                ) => {
                    supervisor.apply_discovery_snapshot(
                        discovery_snapshot,
                        &mut service_discovery,
                        sockets.as_ref().map(|sockets| &sockets.unicast_discovery),
                        network_discovery_owner,
                    ).await;
                }
                () = supervisor.status.wait_until_disabled() => continue,
                _ = beacon_interval.tick() => {
                    beacon_cycle = beacon_cycle.wrapping_add(1);
                    supervisor.run_beacon_cycle(
                        beacon_cycle,
                        &runtime_started_at,
                        discovery_participation,
                        &mut nics,
                        &mut sockets,
                        network_discovery_owner,
                    ).await;
                    let rebind_schedule = RebindSchedule::for_beacon(beacon_cycle);
                    rendezvous_claim_schedule = RendezvousClaimSchedule::after_beacon(
                        &settings,
                        &rendezvous_listener,
                        rebind_schedule,
                    );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ScopedPeer {
    ip_address: Ipv6Addr,
    scope_id: u32,
}

impl ScopedPeer {
    const fn from_socket_address(socket_address: SocketAddrV6) -> Self {
        Self {
            ip_address: *socket_address.ip(),
            scope_id: socket_address.scope_id(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerMemberAdmission {
    Existing,
    Available,
    AtCapacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerMembership {
    Existing,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeaconChannel {
    Multicast,
    Unicast,
}

fn peer_member_admission(
    peer_membership: PeerMembership,
    current_member_count: usize,
) -> PeerMemberAdmission {
    match peer_membership {
        PeerMembership::Existing => PeerMemberAdmission::Existing,
        PeerMembership::New if current_member_count < usize::from(UDP_PEER_CAPACITY.get()) => {
            PeerMemberAdmission::Available
        }
        PeerMembership::New => PeerMemberAdmission::AtCapacity,
    }
}

struct AttachedStatus {
    attached: AttachedInterface,
    status: TokioInterfaceStatus,
}

struct DiscoveredServices {
    tcp_dials: DiscoveredTcpDials,
    udp_targets: DiscoveredUdpTargets,
}

impl DiscoveredServices {
    const fn new() -> Self {
        Self {
            tcp_dials: DiscoveredTcpDials::new(),
            udp_targets: DiscoveredUdpTargets::new(),
        }
    }

    fn reconcile(
        &mut self,
        discovery_snapshot: &DiscoverySnapshot,
        network_discovery_owner: NetworkDiscoveryOwner,
        local_prefixes: &[LocalPrefix],
        fleet: &Fleet,
        policy: EffectiveInterfacePolicy,
        completed_traffic: &mut CompletedTraffic,
    ) -> NewlyActiveUdpTargets {
        let selected_tcp_endpoints = selected_discovery_endpoints(
            discovery_snapshot,
            DiscoveryTransport::Tcp,
            network_discovery_owner,
            local_prefixes,
        );
        self.tcp_dials.reconcile(
            selected_tcp_endpoints.values().copied().collect(),
            fleet,
            policy,
            completed_traffic,
        );

        let selected_udp_endpoints = selected_discovery_endpoints(
            discovery_snapshot,
            DiscoveryTransport::Udp,
            network_discovery_owner,
            local_prefixes,
        );
        self.udp_targets.reconcile(selected_udp_endpoints)
    }

    fn clear(&mut self, completed_traffic: &mut CompletedTraffic) {
        self.tcp_dials.clear(completed_traffic);
        self.udp_targets.clear();
    }
}

struct DiscoveredTcpDials {
    members: BTreeMap<DiscoveryEndpoint, AttachedStatus>,
}

impl DiscoveredTcpDials {
    const fn new() -> Self {
        Self {
            members: BTreeMap::new(),
        }
    }

    fn reconcile(
        &mut self,
        desired_endpoints: BTreeSet<DiscoveryEndpoint>,
        fleet: &Fleet,
        policy: EffectiveInterfacePolicy,
        completed_traffic: &mut CompletedTraffic,
    ) {
        let removed_endpoints: std::vec::Vec<DiscoveryEndpoint> = self
            .members
            .keys()
            .filter(|endpoint| !desired_endpoints.contains(endpoint))
            .copied()
            .collect();
        for removed_endpoint in removed_endpoints {
            if let Some(removed_dial) = self.members.remove(&removed_endpoint) {
                completed_traffic.retain(&removed_dial.status);
                removed_dial.attached.teardown();
            }
        }
        for desired_endpoint in desired_endpoints {
            if self.members.contains_key(&desired_endpoint) {
                continue;
            }
            crate::diagnostic_log::debug!(
                "wifi-auto: dialing discovered service {desired_endpoint}"
            );
            let tcp_client = TcpClientInterface::with_policy(
                desired_endpoint.to_string(),
                policy,
                RENDEZVOUS_RECONNECT,
            );
            let client_status = tcp_client.status();
            let attached_client =
                fleet.add_named(tcp_client, desired_endpoint.to_string(), None);
            self.members.insert(
                desired_endpoint,
                AttachedStatus {
                    attached: attached_client,
                    status: client_status,
                },
            );
        }
    }

    fn clear(&mut self, completed_traffic: &mut CompletedTraffic) {
        for (_discovery_endpoint, removed_dial) in std::mem::take(&mut self.members) {
            completed_traffic.retain(&removed_dial.status);
            removed_dial.attached.teardown();
        }
    }
}

struct DiscoveredUdpTargets {
    by_service: BTreeMap<DiscoveryServiceName, DiscoveryEndpoint>,
}

impl DiscoveredUdpTargets {
    const fn new() -> Self {
        Self {
            by_service: BTreeMap::new(),
        }
    }

    fn reconcile(
        &mut self,
        selected_endpoints: BTreeMap<DiscoveryServiceName, DiscoveryEndpoint>,
    ) -> NewlyActiveUdpTargets {
        let previously_active = self.active_endpoints();
        self.by_service = selected_endpoints;
        let currently_active = self.active_endpoints();
        NewlyActiveUdpTargets(
            currently_active
                .difference(&previously_active)
                .copied()
                .collect(),
        )
    }

    fn active_endpoints(&self) -> BTreeSet<DiscoveryEndpoint> {
        self.by_service.values().copied().collect()
    }

    fn clear(&mut self) {
        self.by_service.clear();
    }
}

struct NewlyActiveUdpTargets(BTreeSet<DiscoveryEndpoint>);

impl NewlyActiveUdpTargets {
    fn iter(&self) -> impl Iterator<Item = DiscoveryEndpoint> + '_ {
        self.0.iter().copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UdpPeeringProbe {
    target: SocketAddrV6,
    peering_token: [u8; contract::PEERING_TOKEN_BYTES],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeeringTokenReply {
    NotRequired,
    Send(UdpPeeringProbe),
}

enum UdpPeeringProbePreparation {
    Ready(UdpPeeringProbe),
    LocalInterfaceUnavailable,
    PeerAlreadyKnown,
    EndpointContractMismatch,
}

fn selected_discovery_endpoints(
    discovery_snapshot: &DiscoverySnapshot,
    transport: DiscoveryTransport,
    network_discovery_owner: NetworkDiscoveryOwner,
    local_prefixes: &[LocalPrefix],
) -> BTreeMap<DiscoveryServiceName, DiscoveryEndpoint> {
    discovery_snapshot
        .iter()
        .filter(|service_advertisement| service_advertisement.service().transport() == transport)
        .filter_map(|service_advertisement| {
            let selected_endpoint =
                service_advertisement
                    .endpoints()
                    .iter()
                    .copied()
                    .find(|discovery_endpoint| {
                        endpoint_is_eligible(
                            *discovery_endpoint,
                            network_discovery_owner,
                            local_prefixes,
                        )
                    })?;
            Some((service_advertisement.service().clone(), selected_endpoint))
        })
        .collect()
}

struct Supervisor {
    brains: HashMap<u32, AutoWifiBrain>,
    members: HashMap<ScopedPeer, PeerMember>,
    gateways: HashMap<u32, GatewayDial>,
    discovered_services: DiscoveredServices,
    loopback: Option<AttachedStatus>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkActivation {
    AlreadyActive,
    Activated,
    NoEligibleInterfaces,
    SocketUnavailable(io::ErrorKind),
}

impl Supervisor {
    fn ensure_network_active(
        &mut self,
        nics: &mut std::vec::Vec<Nic>,
        sockets: &mut Option<Sockets>,
        network_discovery_owner: NetworkDiscoveryOwner,
    ) -> NetworkActivation {
        if let Some(_active_sockets) = sockets {
            return NetworkActivation::AlreadyActive;
        }
        let fresh_nics = link_local_nics(&self.settings.devices);
        if fresh_nics.is_empty() {
            nics.clear();
            return NetworkActivation::NoEligibleInterfaces;
        }
        let opened_sockets =
            match open_sockets(&fresh_nics, &self.settings, network_discovery_owner) {
                Ok(opened_sockets) => opened_sockets,
                Err(socket_error) => {
                    *nics = fresh_nics;
                    return NetworkActivation::SocketUnavailable(socket_error.kind());
                }
            };
        *nics = fresh_nics;
        *sockets = Some(opened_sockets);
        self.activate_network(nics, sockets.as_ref());
        if network_discovery_owner == NetworkDiscoveryOwner::Host {
            self.dial_initial_gateways(nics);
        }
        NetworkActivation::Activated
    }

    async fn suspend_until_enabled(
        &mut self,
        rendezvous_listener: &mut Option<TcpListener>,
        service_discovery: Option<&ServiceDiscovery>,
        nics: &mut std::vec::Vec<Nic>,
        sockets: &mut Option<Sockets>,
        network_discovery_owner: NetworkDiscoveryOwner,
    ) {
        *rendezvous_listener = None;
        if let Some(service_discovery) = service_discovery {
            service_discovery.set_participation(DiscoveryParticipation::Inactive);
        }
        self.disable_members();
        drop(sockets.take());
        nics.clear();
        self.status.wait_until_enabled().await;
        if auto_interface_network_participation(&self.settings, DiscoveryParticipation::Inactive)
            == AutoInterfaceNetworkParticipation::Active
        {
            let _network_activation =
                self.ensure_network_active(nics, sockets, network_discovery_owner);
        }
    }

    fn apply_rendezvous_claim(
        &mut self,
        rendezvous_claim: RendezvousClaim,
        rendezvous_listener: &mut Option<TcpListener>,
        nics: &mut std::vec::Vec<Nic>,
        sockets: &mut Option<Sockets>,
        network_discovery_owner: NetworkDiscoveryOwner,
    ) -> DiscoveryParticipation {
        match rendezvous_claim {
            RendezvousClaim::Central(claimed_rendezvous_listener) => {
                *rendezvous_listener = Some(claimed_rendezvous_listener);
                self.remove_loopback();
                if let NetworkActivation::SocketUnavailable(error_kind) =
                    self.ensure_network_active(nics, sockets, network_discovery_owner)
                {
                    crate::diagnostic_log::debug!(
                        "wifi-auto: UDP sockets unavailable ({error_kind:?}); retrying"
                    );
                }
                DiscoveryParticipation::Central
            }
            RendezvousClaim::Satellite => {
                *rendezvous_listener = None;
                self.teardown_auto_interface_network();
                drop(sockets.take());
                nics.clear();
                self.ensure_loopback();
                self.clear_discovered_services();
                DiscoveryParticipation::Satellite
            }
            RendezvousClaim::Unavailable(error_kind) => {
                *rendezvous_listener = None;
                self.teardown_auto_interface_network();
                drop(sockets.take());
                nics.clear();
                self.remove_loopback();
                self.clear_discovered_services();
                crate::diagnostic_log::debug!(
                    "wifi-auto: rendezvous listener unavailable ({error_kind:?}); retrying"
                );
                DiscoveryParticipation::Inactive
            }
        }
    }

    fn accept_rendezvous_connection(
        &mut self,
        accepted_connection: io::Result<(TcpStream, SocketAddr)>,
    ) {
        match accepted_connection {
            Ok((tcp_stream, peer_address)) if is_local_peer(peer_address.ip(), &self.prefixes) => {
                self.reap_accepted_rendezvous();
                if self.accepted.len() >= usize::from(TCP_RENDEZVOUS_ACCEPTED_CAPACITY) {
                    crate::diagnostic_log::debug!(
                        "wifi-auto: rejecting rendezvous connection from {peer_address}; capacity reached"
                    );
                } else {
                    tune(&tcp_stream);
                    let tcp_connection = TcpServerConnection::with_policy(
                        peer_address.to_string().into_bytes(),
                        tcp_stream,
                        self.policy,
                    );
                    let connection_status = tcp_connection.status();
                    let attached_connection =
                        self.fleet.add_named(tcp_connection, peer_address.to_string(), None);
                    self.accepted.push(AttachedStatus {
                        attached: attached_connection,
                        status: connection_status,
                    });
                    self.publish_status();
                }
            }
            Ok(_) | Err(_) => {}
        }
    }

    async fn apply_discovery_snapshot(
        &mut self,
        discovery_snapshot: Result<DiscoverySnapshot, discovery::DiscoverySnapshotError>,
        service_discovery: &mut Option<ServiceDiscovery>,
        unicast_discovery_socket: Option<&UdpSocket>,
        network_discovery_owner: NetworkDiscoveryOwner,
    ) {
        match discovery_snapshot {
            Ok(discovery_snapshot) => {
                let newly_active_udp_targets =
                    self.reconcile_discovery(&discovery_snapshot, network_discovery_owner);
                let probes = self.prepare_udp_peering_probes(newly_active_udp_targets.iter());
                send_udp_peering_probes(unicast_discovery_socket, probes).await;
            }
            Err(discovery::DiscoverySnapshotError::PublisherClosed) => {
                *service_discovery = None;
                self.clear_discovered_services();
            }
        }
    }

    async fn run_beacon_cycle(
        &mut self,
        beacon_cycle: u32,
        runtime_started_at: &tokio::time::Instant,
        discovery_participation: DiscoveryParticipation,
        nics: &mut std::vec::Vec<Nic>,
        sockets: &mut Option<Sockets>,
        network_discovery_owner: NetworkDiscoveryOwner,
    ) {
        let rebind_schedule = RebindSchedule::for_beacon(beacon_cycle);
        self.recover_network_for_beacon(
            rebind_schedule,
            discovery_participation,
            nics,
            sockets,
            network_discovery_owner,
        )
        .await;
        self.send_periodic_peering(sockets.as_ref(), PeeringRefresh::for_beacon(beacon_cycle))
            .await;
        let elapsed_millis = runtime_started_at.elapsed().as_millis() as u64;
        self.reconcile_network_for_beacon(rebind_schedule, nics, sockets, network_discovery_owner);
        self.retire_stale(elapsed_millis);
        self.publish_status();
    }

    async fn recover_network_for_beacon(
        &mut self,
        rebind_schedule: RebindSchedule,
        discovery_participation: DiscoveryParticipation,
        nics: &mut std::vec::Vec<Nic>,
        sockets: &mut Option<Sockets>,
        network_discovery_owner: NetworkDiscoveryOwner,
    ) {
        if let (None, AutoInterfaceNetworkParticipation::Active, RebindSchedule::Due) = (
            sockets.as_ref(),
            auto_interface_network_participation(&self.settings, discovery_participation),
            rebind_schedule,
        ) {
            match self.ensure_network_active(nics, sockets, network_discovery_owner) {
                NetworkActivation::Activated => {
                    let active_udp_targets =
                        self.discovered_services.udp_targets.active_endpoints();
                    let probes = self.prepare_udp_peering_probes(active_udp_targets);
                    send_udp_peering_probes(
                        sockets.as_ref().map(|sockets| &sockets.unicast_discovery),
                        probes,
                    )
                    .await;
                }
                NetworkActivation::SocketUnavailable(error_kind) => {
                    crate::diagnostic_log::debug!(
                        "wifi-auto: UDP sockets unavailable ({error_kind:?}); retrying"
                    );
                }
                NetworkActivation::AlreadyActive | NetworkActivation::NoEligibleInterfaces => {}
            }
        }
    }

    async fn send_periodic_peering(
        &self,
        sockets: Option<&Sockets>,
        peering_refresh: PeeringRefresh,
    ) {
        if let Some(sockets) = sockets {
            for (&interface_index, auto_interface_protocol) in &self.brains {
                let peering_token = *auto_interface_protocol.our_peering_token().as_bytes();
                if let Some(multicast_discovery_socket) = sockets.discovery.as_ref() {
                    let _ = multicast_discovery_socket
                        .send_to(
                            &peering_token,
                            scoped(
                                self.settings.discovery_group(),
                                self.settings.discovery_port,
                                interface_index,
                            ),
                        )
                        .await;
                }
                if peering_refresh == PeeringRefresh::Due {
                    for peer_address in auto_interface_protocol.known_peer_addresses() {
                        let _ = sockets
                            .unicast_discovery
                            .send_to(
                                &peering_token,
                                scoped(
                                    peer_address,
                                    self.settings.reverse_discovery_port(),
                                    interface_index,
                                ),
                            )
                            .await;
                    }
                }
            }
            if peering_refresh == PeeringRefresh::Due {
                let active_udp_targets = self.discovered_services.udp_targets.active_endpoints();
                let probes = self.prepare_udp_peering_probes(active_udp_targets);
                send_udp_peering_probes(Some(&sockets.unicast_discovery), probes).await;
            }
        }
    }

    fn reconcile_network_for_beacon(
        &mut self,
        rebind_schedule: RebindSchedule,
        nics: &mut std::vec::Vec<Nic>,
        sockets: &mut Option<Sockets>,
        network_discovery_owner: NetworkDiscoveryOwner,
    ) {
        if let (Some(active_sockets), RebindSchedule::Due) = (sockets.as_ref(), rebind_schedule) {
            self.reconcile_nics(
                active_sockets.discovery.as_ref(),
                nics,
                network_discovery_owner,
            );
            if nics.is_empty() {
                self.deactivate_network();
                *sockets = None;
            }
        }
    }

    fn activate_network(&mut self, nics: &[Nic], sockets: Option<&Sockets>) {
        self.brains = if sockets.is_some() {
            nics.iter()
                .map(|nic| {
                    (
                        nic.index,
                        AutoWifiBrain::from_link_local_with_group(
                            nic.link_local,
                            &self.settings.group_id,
                        ),
                    )
                })
                .collect()
        } else {
            HashMap::new()
        };
        self.data = sockets.map(|sockets| sockets.data.clone());
        self.prefixes = local_prefixes(&self.settings.devices);
    }

    fn deactivate_network(&mut self) {
        self.brains.clear();
        self.data = None;
        self.prefixes.clear();
    }

    fn teardown_auto_interface_network(&mut self) {
        for (_peer_address, member) in self.members.drain() {
            self.completed.retain(&member.status);
            member.attached.teardown();
        }
        for (_interface_index, gateway) in self.gateways.drain() {
            self.completed.retain(&gateway.status);
            gateway.attached.teardown();
        }
        self.deactivate_network();
    }

    fn ingest_beacon(
        &mut self,
        src: SocketAddr,
        bytes: &[u8],
        now_ms: u64,
        beacon_channel: BeaconChannel,
    ) -> PeeringTokenReply {
        let SocketAddr::V6(v6) = src else {
            return PeeringTokenReply::NotRequired;
        };
        let scope = v6.scope_id();
        let peer = ScopedPeer::from_socket_address(v6);
        let member_admission = self.peer_member_admission(peer);
        if member_admission == PeerMemberAdmission::AtCapacity {
            return PeeringTokenReply::NotRequired;
        }
        let Some(brain) = self.brains.get_mut(&scope) else {
            return PeeringTokenReply::NotRequired;
        };
        let peering_token = *brain.our_peering_token().as_bytes();
        let contract::BeaconObservation::AuthenticatedPeer {
            address,
            peer_observation,
        } = brain.observe_discovery_datagram(*v6.ip(), bytes, now_ms)
        else {
            return PeeringTokenReply::NotRequired;
        };
        if peer_observation == contract::PeerObservation::TableFull {
            return PeeringTokenReply::NotRequired;
        }
        if member_admission == PeerMemberAdmission::Available {
            self.spawn_member(ScopedPeer {
                ip_address: address,
                scope_id: scope,
            });
        }
        match (beacon_channel, peer_observation) {
            (BeaconChannel::Unicast, contract::PeerObservation::NewlyDiscovered) => {
                PeeringTokenReply::Send(UdpPeeringProbe {
                    target: SocketAddrV6::new(
                        address,
                        self.settings.reverse_discovery_port(),
                        0,
                        scope,
                    ),
                    peering_token,
                })
            }
            (
                BeaconChannel::Multicast | BeaconChannel::Unicast,
                contract::PeerObservation::Refreshed | contract::PeerObservation::TableFull,
            )
            | (BeaconChannel::Multicast, contract::PeerObservation::NewlyDiscovered) => {
                PeeringTokenReply::NotRequired
            }
        }
    }

    fn peer_member_admission(&self, peer: ScopedPeer) -> PeerMemberAdmission {
        let peer_membership = match self.members.get(&peer) {
            Some(_existing_member) => PeerMembership::Existing,
            None => PeerMembership::New,
        };
        peer_member_admission(peer_membership, self.members.len())
    }

    fn spawn_member(&mut self, peer_address: ScopedPeer) {
        let Some(data) = self.data.clone() else {
            return;
        };
        let (inbound_tx, inbound_rx) = mpsc::channel(PEER_INBOUND_DEPTH);
        let peer = SocketAddrV6::new(
            peer_address.ip_address,
            self.settings.data_port,
            0,
            peer_address.scope_id,
        );
        let member = AutoWifiPeer::with_policy_and_instance_tag(
            data,
            peer,
            inbound_rx,
            self.policy,
            &self.settings.instance_tag,
        );
        let status = member.status();
        let peer_name = format!("{}%{}", peer_address.ip_address, peer_address.scope_id);
        let attached = self.fleet.add_named(member, peer_name, None);
        crate::diagnostic_log::debug!(
            "wifi-auto: peer {}%{} discovered",
            peer_address.ip_address,
            peer_address.scope_id
        );
        self.members.insert(
            peer_address,
            PeerMember {
                attached,
                inbound: inbound_tx,
                status,
            },
        );
        self.publish_status();
    }

    fn publish_status(&mut self) {
        self.reap_accepted_rendezvous();
        let mut statuses: std::vec::Vec<TokioInterfaceStatus> = self
            .members
            .values()
            .map(|member| member.status.clone())
            .collect();
        statuses.extend(self.gateways.values().map(|dial| dial.status.clone()));
        statuses.extend(self.accepted.iter().map(|member| member.status.clone()));
        statuses.extend(
            self.discovered_services
                .tcp_dials
                .members
                .values()
                .map(|dial| dial.status.clone()),
        );
        statuses.extend(self.loopback.iter().map(|dial| dial.status.clone()));
        statuses.sort_by_key(InterfaceStatus::id);
        self.status.publish(&self.completed, statuses);
    }

    fn disable_members(&mut self) {
        self.teardown_auto_interface_network();
        self.discovered_services.clear(&mut self.completed);
        self.remove_loopback();
        for member in self.accepted.drain(..) {
            self.completed.retain(&member.status);
            member.attached.teardown();
        }
        self.status.publish(&self.completed, std::vec::Vec::new());
    }

    fn reconcile_discovery(
        &mut self,
        discovery_snapshot: &DiscoverySnapshot,
        network_discovery_owner: NetworkDiscoveryOwner,
    ) -> NewlyActiveUdpTargets {
        let newly_active_udp_targets = self.discovered_services.reconcile(
            discovery_snapshot,
            network_discovery_owner,
            &self.prefixes,
            &self.fleet,
            self.policy,
            &mut self.completed,
        );
        self.publish_status();
        newly_active_udp_targets
    }

    fn prepare_udp_peering_probes(
        &self,
        discovery_endpoints: impl IntoIterator<Item = DiscoveryEndpoint>,
    ) -> std::vec::Vec<UdpPeeringProbe> {
        discovery_endpoints
            .into_iter()
            .filter_map(|discovery_endpoint| {
                match self.prepare_udp_peering_probe(discovery_endpoint) {
                    UdpPeeringProbePreparation::Ready(probe) => Some(probe),
                    UdpPeeringProbePreparation::LocalInterfaceUnavailable
                    | UdpPeeringProbePreparation::PeerAlreadyKnown
                    | UdpPeeringProbePreparation::EndpointContractMismatch => None,
                }
            })
            .collect()
    }

    fn prepare_udp_peering_probe(
        &self,
        discovery_endpoint: DiscoveryEndpoint,
    ) -> UdpPeeringProbePreparation {
        if discovery_endpoint.transport() != DiscoveryTransport::Udp {
            return UdpPeeringProbePreparation::EndpointContractMismatch;
        }
        let SocketAddr::V6(target) = discovery_endpoint.socket_addr() else {
            return UdpPeeringProbePreparation::EndpointContractMismatch;
        };
        let Some(auto_interface_protocol) = self.brains.get(&target.scope_id()) else {
            return UdpPeeringProbePreparation::LocalInterfaceUnavailable;
        };
        if auto_interface_protocol
            .known_peer_addresses()
            .any(|peer_address| peer_address == *target.ip())
        {
            return UdpPeeringProbePreparation::PeerAlreadyKnown;
        }
        UdpPeeringProbePreparation::Ready(UdpPeeringProbe {
            target,
            peering_token: *auto_interface_protocol.our_peering_token().as_bytes(),
        })
    }

    fn clear_discovered_services(&mut self) {
        self.discovered_services.clear(&mut self.completed);
        self.publish_status();
    }

    fn reap_accepted_rendezvous(&mut self) {
        let mut retained = std::vec::Vec::with_capacity(self.accepted.len());
        for accepted in std::mem::take(&mut self.accepted) {
            if matches!(
                accepted.status.connection(),
                ConnectionState::Disconnected | ConnectionState::Failed | ConnectionState::Disabled
            ) {
                self.completed.retain(&accepted.status);
                accepted.attached.teardown();
            } else {
                retained.push(accepted);
            }
        }
        self.accepted = retained;
    }

    fn ensure_loopback(&mut self) {
        if self.loopback.is_some() || !self.settings.stock_service_discovery_enabled() {
            return;
        }
        let target = std::format!("127.0.0.1:{}", contract::TCP_RENDEZVOUS_PORT);
        let client = TcpClientInterface::with_policy(target.clone(), self.policy, RENDEZVOUS_RECONNECT);
        let status = client.status();
        let attached = self.fleet.add_named(client, target, None);
        self.loopback = Some(AttachedStatus { attached, status });
        self.publish_status();
    }

    fn remove_loopback(&mut self) {
        let Some(loopback) = self.loopback.take() else {
            return;
        };
        self.completed.retain(&loopback.status);
        loopback.attached.teardown();
        self.publish_status();
    }

    fn route_inbound(&self, src: SocketAddr, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let SocketAddr::V6(v6) = src else { return };
        let peer = ScopedPeer::from_socket_address(v6);
        if let Some(member) = self.members.get(&peer) {
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
        let live: HashSet<ScopedPeer> = self
            .brains
            .iter()
            .flat_map(|(&scope_id, brain)| {
                brain
                    .known_peer_addresses()
                    .map(move |ip_address| ScopedPeer {
                        ip_address,
                        scope_id,
                    })
            })
            .collect();
        let gone: std::vec::Vec<ScopedPeer> = self
            .members
            .keys()
            .filter(|addr| !live.contains(*addr))
            .copied()
            .collect();
        if gone.is_empty() {
            return;
        }
        for peer_address in gone {
            if let Some(member) = self.members.remove(&peer_address) {
                crate::diagnostic_log::debug!(
                    "wifi-auto: peer {}%{} retired after missed beacons",
                    peer_address.ip_address,
                    peer_address.scope_id
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
        if !self.settings.stock_service_discovery_enabled() {
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
            let client =
                TcpClientInterface::with_policy(target.clone(), self.policy, RENDEZVOUS_RECONNECT);
            let status = client.status();
            let attached = self.fleet.add_named(client, target, None);
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

    fn reconcile_nics(
        &mut self,
        discovery: Option<&UdpSocket>,
        nics: &mut std::vec::Vec<Nic>,
        network_discovery_owner: NetworkDiscoveryOwner,
    ) {
        let gateways = match network_discovery_owner {
            NetworkDiscoveryOwner::Host => platform_gateway_inventory(),
            NetworkDiscoveryOwner::Platform => Err(GatewayInventoryUnavailable),
        };
        let fresh = link_local_nics(&self.settings.devices);
        self.prefixes = local_prefixes(&self.settings.devices);
        self.apply_reconcile(discovery, nics, fresh, gateways);
    }

    fn apply_reconcile(
        &mut self,
        discovery: Option<&UdpSocket>,
        nics: &mut std::vec::Vec<Nic>,
        fresh: std::vec::Vec<Nic>,
        gateways: GatewayInventory,
    ) {
        let plan = plan_reconcile(nics, &fresh);
        let discovery_group = self.settings.discovery_group();
        for index in plan.removed {
            if let Some(discovery) = discovery {
                let _ = discovery.leave_multicast_v6(&discovery_group, index);
            }
            self.brains.remove(&index);
            if let Some(dial) = self.gateways.remove(&index) {
                self.completed.retain(&dial.status);
                dial.attached.teardown();
            }
        }
        for nic in plan.added {
            if let Some(discovery) = discovery {
                let _ = discovery.join_multicast_v6(&discovery_group, nic.index);
            }
            self.brains.insert(
                nic.index,
                AutoWifiBrain::from_link_local_with_group(nic.link_local, &self.settings.group_id),
            );
        }
        for nic in plan.rebound {
            self.brains.insert(
                nic.index,
                AutoWifiBrain::from_link_local_with_group(nic.link_local, &self.settings.group_id),
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
    discovery: Option<UdpSocket>,
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
    index: u32,
}

fn local_prefixes(auto_wifi_device_policy: &AutoWifiDevicePolicy) -> std::vec::Vec<LocalPrefix> {
    let Ok(network_interfaces) = if_addrs::get_if_addrs() else {
        return std::vec::Vec::new();
    };
    network_interfaces
        .iter()
        .filter(|network_interface| {
            auto_wifi_device_policy.allows(&network_interface.name, network_interface.is_loopback())
        })
        .map(|network_interface| match &network_interface.addr {
            if_addrs::IfAddr::V4(ipv4_address) => LocalPrefix {
                addr: IpAddr::V4(ipv4_address.ip),
                netmask: IpAddr::V4(ipv4_address.netmask),
                index: network_interface.index.unwrap_or(0),
            },
            if_addrs::IfAddr::V6(ipv6_address) => LocalPrefix {
                addr: IpAddr::V6(ipv6_address.ip),
                netmask: IpAddr::V6(ipv6_address.netmask),
                index: network_interface.index.unwrap_or(0),
            },
        })
        .collect()
}

fn is_local_peer(peer_address: IpAddr, local_prefixes: &[LocalPrefix]) -> bool {
    peer_address.is_loopback()
        || local_prefixes.iter().any(|local_prefix| {
            is_same_subnet(local_prefix.addr, local_prefix.netmask, peer_address)
        })
}

fn is_own_address(ip_address: IpAddr, local_prefixes: &[LocalPrefix]) -> bool {
    ip_address.is_loopback()
        || local_prefixes
            .iter()
            .any(|local_prefix| local_prefix.addr == ip_address)
}

fn endpoint_is_eligible(
    discovery_endpoint: DiscoveryEndpoint,
    network_discovery_owner: NetworkDiscoveryOwner,
    local_prefixes: &[LocalPrefix],
) -> bool {
    let socket_address = discovery_endpoint.socket_addr();
    if is_own_address(socket_address.ip(), local_prefixes) {
        return false;
    }
    if network_discovery_owner == NetworkDiscoveryOwner::Platform {
        return true;
    }
    local_prefixes.iter().any(|local_prefix| {
        if let SocketAddr::V6(ipv6_socket_address) = socket_address {
            if ipv6_socket_address.ip().is_unicast_link_local()
                && ipv6_socket_address.scope_id() != local_prefix.index
            {
                return false;
            }
        }
        is_same_subnet(local_prefix.addr, local_prefix.netmask, socket_address.ip())
    })
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

enum RendezvousClaim {
    Central(TcpListener),
    Satellite,
    Unavailable(io::ErrorKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RendezvousBindFailure {
    Satellite,
    Unavailable(io::ErrorKind),
}

fn classify_rendezvous_bind_error(bind_error: &io::Error) -> RendezvousBindFailure {
    if bind_error.kind() == io::ErrorKind::AddrInUse {
        RendezvousBindFailure::Satellite
    } else {
        RendezvousBindFailure::Unavailable(bind_error.kind())
    }
}

async fn claim_local_rendezvous() -> RendezvousClaim {
    match TcpListener::bind(("0.0.0.0", contract::TCP_RENDEZVOUS_PORT)).await {
        Ok(rendezvous_listener) => RendezvousClaim::Central(rendezvous_listener),
        Err(bind_error) => match classify_rendezvous_bind_error(&bind_error) {
            RendezvousBindFailure::Satellite => RendezvousClaim::Satellite,
            RendezvousBindFailure::Unavailable(error_kind) => {
                RendezvousClaim::Unavailable(error_kind)
            }
        },
    }
}

async fn accept_maybe(
    rendezvous_listener: &Option<TcpListener>,
) -> io::Result<(TcpStream, SocketAddr)> {
    match rendezvous_listener {
        Some(rendezvous_listener) => rendezvous_listener.accept().await,
        None => std::future::pending().await,
    }
}

async fn recv_maybe(socket: Option<&UdpSocket>, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
    match socket {
        Some(socket) => socket.recv_from(buf).await.ok(),
        None => std::future::pending().await,
    }
}

async fn send_udp_peering_probes(
    unicast_discovery_socket: Option<&UdpSocket>,
    probes: std::vec::Vec<UdpPeeringProbe>,
) {
    let Some(unicast_discovery_socket) = unicast_discovery_socket else {
        return;
    };
    for probe in probes {
        let _ = unicast_discovery_socket
            .send_to(&probe.peering_token, SocketAddr::V6(probe.target))
            .await;
    }
}

async fn send_peering_token_reply(
    unicast_discovery_socket: Option<&UdpSocket>,
    peering_token_reply: PeeringTokenReply,
) {
    let (Some(unicast_discovery_socket), PeeringTokenReply::Send(reply)) =
        (unicast_discovery_socket, peering_token_reply)
    else {
        return;
    };
    let _ = unicast_discovery_socket
        .send_to(&reply.peering_token, SocketAddr::V6(reply.target))
        .await;
}

async fn next_discovery_snapshot(
    service_discovery: Option<&mut ServiceDiscovery>,
    discovery_participation: DiscoveryParticipation,
) -> Result<DiscoverySnapshot, discovery::DiscoverySnapshotError> {
    match discovery_participation {
        DiscoveryParticipation::Inactive | DiscoveryParticipation::Satellite => {
            return std::future::pending().await;
        }
        DiscoveryParticipation::Central => {}
    }
    match service_discovery {
        Some(service_discovery) => service_discovery.next_snapshot().await,
        None => std::future::pending().await,
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

fn open_sockets(
    nics: &[Nic],
    settings: &AutoWifiSettings,
    network_discovery_owner: NetworkDiscoveryOwner,
) -> io::Result<Sockets> {
    Ok(Sockets {
        discovery: match network_discovery_owner {
            NetworkDiscoveryOwner::Host => discovery_socket(nics, settings).ok(),
            NetworkDiscoveryOwner::Platform => None,
        },
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
    use std::num::NonZeroU8;

    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};

    const TEST_DISCOVERY_CAPACITY: NonZeroU8 = NonZeroU8::new(8).unwrap();
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
    fn platform_discovery_selects_platform_network_ownership() {
        let (service_discovery, _service_discovery_publisher) =
            ServiceDiscovery::channel(TEST_DISCOVERY_CAPACITY);
        let auto_wifi = AutoWifi::new().with_platform_discovery(service_discovery);

        auto_wifi
            .service_discovery
            .as_ref()
            .expect("platform discovery is retained");
        assert!(matches!(
            auto_wifi.network_discovery_owner,
            NetworkDiscoveryOwner::Platform
        ));
    }

    #[test]
    fn stock_satellites_are_network_dormant_while_custom_auto_interface_remains_standalone() {
        let stock_settings = AutoWifiSettings::default();
        assert_eq!(
            auto_interface_network_participation(&stock_settings, DiscoveryParticipation::Central),
            AutoInterfaceNetworkParticipation::Active
        );
        assert_eq!(
            auto_interface_network_participation(
                &stock_settings,
                DiscoveryParticipation::Satellite
            ),
            AutoInterfaceNetworkParticipation::Dormant
        );
        assert_eq!(
            auto_interface_network_participation(&stock_settings, DiscoveryParticipation::Inactive),
            AutoInterfaceNetworkParticipation::Dormant
        );

        let custom_settings = AutoWifiSettings::new(
            b"field-mesh".to_vec(),
            contract::DiscoveryScope::Site,
            contract::MulticastAddressType::Permanent,
            30_000,
            40_000,
            AutoWifiDevicePolicy::default(),
        )
        .expect("custom settings are valid");
        assert_eq!(
            auto_interface_network_participation(
                &custom_settings,
                DiscoveryParticipation::Inactive
            ),
            AutoInterfaceNetworkParticipation::Active
        );
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
        assert!(!settings.stock_service_discovery_enabled());

        let custom_port = AutoWifiSettings::new(
            contract::GROUP_ID.to_vec(),
            contract::DiscoveryScope::Link,
            contract::MulticastAddressType::Temporary,
            30_000,
            contract::DEFAULT_DATA_PORT,
            AutoWifiDevicePolicy::default(),
        )
        .expect("ports are valid");
        assert!(AutoWifiSettings::default().stock_service_discovery_enabled());
        assert!(!custom_port.stock_service_discovery_enabled());
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
            index: 1,
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
        let supervisor = Supervisor {
            brains: HashMap::new(),
            members: HashMap::new(),
            gateways: HashMap::new(),
            discovered_services: DiscoveredServices::new(),
            loopback: None,
            accepted: std::vec::Vec::new(),
            prefixes: std::vec::Vec::new(),
            fleet,
            data: Some(Arc::new(UdpSocket::from_std(data).expect("into tokio"))),
            policy: contract::configured_policy(Default::default()),
            settings,
            status: AutoWifiStatus::new(id),
            completed: CompletedTraffic::default(),
        };
        (supervisor, guard)
    }

    fn gateway_inventory<const N: usize>(routes: [(u32, IpAddr); N]) -> GatewayInventory {
        Ok(routes.into_iter().collect())
    }

    fn discovery_snapshot(service_name: &str, socket_address: SocketAddr) -> DiscoverySnapshot {
        let discovery_endpoint =
            DiscoveryEndpoint::tcp(socket_address).expect("test endpoint is valid");
        let mut service_advertisement = contract::ServiceAdvertisement::new(
            contract::DiscoveryServiceName::from_instance(
                service_name,
                contract::DiscoveryTransport::Tcp,
            )
            .expect("test service name is valid"),
        );
        let _ = service_advertisement.insert(discovery_endpoint);
        let mut discovery_snapshot = DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY);
        let _ = discovery_snapshot.insert(service_advertisement);
        discovery_snapshot
    }

    fn udp_discovery_endpoint(address_suffix: u16, scope_id: u32) -> DiscoveryEndpoint {
        DiscoveryEndpoint::udp(SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, address_suffix),
            contract::UNICAST_DISCOVERY_PORT,
            0,
            scope_id,
        )))
        .expect("test UDP endpoint is valid")
    }

    fn service_advertisement(
        service_name: &str,
        discovery_endpoint: DiscoveryEndpoint,
    ) -> contract::ServiceAdvertisement {
        let discovery_service_name = contract::DiscoveryServiceName::from_instance(
            service_name,
            discovery_endpoint.transport(),
        )
        .expect("test service name is valid");
        let mut service_advertisement = contract::ServiceAdvertisement::new(discovery_service_name);
        service_advertisement
            .insert(discovery_endpoint)
            .expect("endpoint transport matches the service");
        service_advertisement
    }

    fn insert_advertisement(
        discovery_snapshot: &mut DiscoverySnapshot,
        service_name: &str,
        discovery_endpoint: DiscoveryEndpoint,
    ) {
        assert_eq!(
            discovery_snapshot.insert(service_advertisement(service_name, discovery_endpoint,)),
            contract::AdvertisementInsertion::Inserted
        );
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
        let (mut supervisor, _guard) = test_supervisor();
        let peer = ScopedPeer {
            ip_address: Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x867c),
            scope_id: 0,
        };

        supervisor.spawn_member(peer);
        assert_eq!(supervisor.status.connection(), ConnectionState::Connected);

        let member = supervisor.members.get(&peer).expect("peer was registered");
        member.status.set_connection(ConnectionState::Disconnected);
        supervisor.publish_status();

        assert_eq!(
            supervisor.status.connection(),
            ConnectionState::Disconnected,
            "the aggregate follows the live child state",
        );
    }

    #[tokio::test]
    async fn a_disconnected_rendezvous_dial_does_not_make_the_aggregate_live() {
        use std::net::Ipv4Addr;
        let (mut supervisor, _guard) = test_supervisor();
        let peer = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 77)),
            contract::TCP_RENDEZVOUS_PORT,
        );
        supervisor.prefixes = std::vec![prefix([192, 168, 1, 50], [255, 255, 255, 0])];

        supervisor.reconcile_discovery(
            &discovery_snapshot("peer", peer),
            NetworkDiscoveryOwner::Host,
        );
        let endpoint = DiscoveryEndpoint::tcp(peer).unwrap();
        let status = &supervisor
            .discovered_services
            .tcp_dials
            .members
            .get(&endpoint)
            .expect("peer was dialed")
            .status;
        status.set_connection(ConnectionState::Disconnected);
        supervisor.publish_status();

        assert_eq!(
            supervisor.status.connection(),
            ConnectionState::Disconnected,
            "an unsuccessful gateway dial remains dormant",
        );
    }

    #[tokio::test]
    async fn reconcile_applies_nic_churn_and_repoints_gateways_against_a_real_fleet() {
        let (mut supervisor, _guard) = test_supervisor();
        let discovery = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0))
            .await
            .expect("bind discovery socket");
        let mut nics = std::vec::Vec::new();

        supervisor.apply_reconcile(
            Some(&discovery),
            &mut nics,
            std::vec![nic(1, 0x10), nic(2, 0x20)],
            gateway_inventory([(1, IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1)))]),
        );
        assert_eq!(supervisor.brains.len(), 2, "both NICs got a brain");
        assert!(supervisor.brains.contains_key(&1) && supervisor.brains.contains_key(&2));
        assert_eq!(
            supervisor.gateways.get(&1).map(|dial| dial.gateway),
            Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1))),
            "the NIC with a gateway got a dial",
        );
        assert_eq!(
            supervisor.gateways.len(),
            1,
            "the gateway-less NIC dialed nothing"
        );

        supervisor.apply_reconcile(
            Some(&discovery),
            &mut nics,
            std::vec![nic(1, 0x10), nic(3, 0x30)],
            gateway_inventory([
                (1, IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1))),
                (3, IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 3))),
            ]),
        );
        assert!(
            !supervisor.brains.contains_key(&2),
            "the vanished NIC's brain was dropped",
        );
        assert!(
            supervisor.brains.contains_key(&3),
            "the new NIC got a brain"
        );
        assert_eq!(
            supervisor.gateways.get(&1).map(|dial| dial.gateway),
            Some(IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1))),
            "the surviving NIC's gateway was re-pointed after the roam",
        );
        assert!(
            supervisor.gateways.contains_key(&3),
            "the new NIC's gateway was dialed",
        );
        assert!(
            !supervisor.gateways.contains_key(&2),
            "the vanished NIC's gateway dial was torn down",
        );

        supervisor.apply_reconcile(
            Some(&discovery),
            &mut nics,
            std::vec![],
            Err(GatewayInventoryUnavailable),
        );
        assert!(
            supervisor.brains.is_empty(),
            "the brains drained when the link left"
        );
        assert!(
            supervisor.gateways.is_empty(),
            "every gateway dial was torn down"
        );
        assert!(nics.is_empty());
    }

    #[tokio::test]
    async fn discovery_snapshots_are_idempotent_remove_stale_peers_and_never_dial_ourselves() {
        use std::net::Ipv4Addr;
        let (mut supervisor, _guard) = test_supervisor();
        supervisor.prefixes = std::vec![prefix([192, 168, 1, 50], [255, 255, 255, 0])];

        let peer = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 77)),
            contract::TCP_RENDEZVOUS_PORT,
        );
        let snapshot = discovery_snapshot("peer", peer);
        supervisor.reconcile_discovery(&snapshot, NetworkDiscoveryOwner::Host);
        assert_eq!(
            supervisor.discovered_services.tcp_dials.members.len(),
            1,
            "a fresh snapshot becomes a dial"
        );

        supervisor.reconcile_discovery(&snapshot, NetworkDiscoveryOwner::Host);
        assert_eq!(
            supervisor.discovered_services.tcp_dials.members.len(),
            1,
            "a repeat snapshot does not stack a second dial",
        );

        let ours = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
            contract::TCP_RENDEZVOUS_PORT,
        );
        supervisor.reconcile_discovery(
            &discovery_snapshot("ours", ours),
            NetworkDiscoveryOwner::Host,
        );
        assert_eq!(
            supervisor.discovered_services.tcp_dials.members.len(),
            0,
            "we never dial our own advertised rendezvous",
        );

        supervisor.reconcile_discovery(
            &DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY),
            NetworkDiscoveryOwner::Host,
        );
        assert!(supervisor.discovered_services.tcp_dials.members.is_empty());
    }

    #[tokio::test]
    async fn one_snapshot_activates_tcp_and_prepares_authenticated_udp_probing() {
        let (mut supervisor, _guard) = test_supervisor();
        let scope_id = 7;
        let our_link_local = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
        supervisor.brains.insert(
            scope_id,
            AutoWifiBrain::from_link_local_with_group(our_link_local, contract::GROUP_ID),
        );
        let tcp_endpoint = DiscoveryEndpoint::tcp("192.168.1.8:42699".parse().unwrap()).unwrap();
        let udp_endpoint = udp_discovery_endpoint(8, scope_id);
        let mut discovery_snapshot = DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY);
        insert_advertisement(&mut discovery_snapshot, "peer-tcp", tcp_endpoint);
        insert_advertisement(&mut discovery_snapshot, "peer-udp", udp_endpoint);

        let newly_active_udp_targets =
            supervisor.reconcile_discovery(&discovery_snapshot, NetworkDiscoveryOwner::Platform);
        assert_eq!(supervisor.discovered_services.tcp_dials.members.len(), 1);
        assert_eq!(
            supervisor.discovered_services.udp_targets.by_service.len(),
            1
        );
        let probes = supervisor.prepare_udp_peering_probes(newly_active_udp_targets.iter());
        assert_eq!(
            probes,
            vec![UdpPeeringProbe {
                target: match udp_endpoint.socket_addr() {
                    SocketAddr::V6(target) => target,
                    SocketAddr::V4(_) => unreachable!("validated UDP endpoints are IPv6"),
                },
                peering_token: *contract::peering_token(&our_link_local).as_bytes(),
            }]
        );
    }

    #[tokio::test]
    async fn udp_records_reconcile_independently_while_duplicate_targets_probe_once() {
        let (mut supervisor, _guard) = test_supervisor();
        let first_endpoint = udp_discovery_endpoint(8, 7);
        let replacement_endpoint = udp_discovery_endpoint(9, 7);
        let mut initial_snapshot = DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY);
        insert_advertisement(&mut initial_snapshot, "first", first_endpoint);
        insert_advertisement(&mut initial_snapshot, "second", first_endpoint);

        let initially_active =
            supervisor.reconcile_discovery(&initial_snapshot, NetworkDiscoveryOwner::Platform);
        assert_eq!(
            initially_active.iter().collect::<Vec<_>>(),
            vec![first_endpoint]
        );
        assert_eq!(
            supervisor.discovered_services.udp_targets.by_service.len(),
            2
        );
        assert_eq!(
            supervisor
                .discovered_services
                .udp_targets
                .active_endpoints(),
            BTreeSet::from([first_endpoint])
        );

        let repeated =
            supervisor.reconcile_discovery(&initial_snapshot, NetworkDiscoveryOwner::Platform);
        assert!(repeated.iter().next().is_none());

        let mut one_record_removed = DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY);
        insert_advertisement(&mut one_record_removed, "second", first_endpoint);
        let still_active =
            supervisor.reconcile_discovery(&one_record_removed, NetworkDiscoveryOwner::Platform);
        assert!(still_active.iter().next().is_none());
        assert_eq!(
            supervisor
                .discovered_services
                .udp_targets
                .active_endpoints(),
            BTreeSet::from([first_endpoint])
        );

        let mut replaced = DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY);
        insert_advertisement(&mut replaced, "second", replacement_endpoint);
        let newly_active =
            supervisor.reconcile_discovery(&replaced, NetworkDiscoveryOwner::Platform);
        assert_eq!(
            newly_active.iter().collect::<Vec<_>>(),
            vec![replacement_endpoint]
        );
        assert_eq!(
            supervisor
                .discovered_services
                .udp_targets
                .active_endpoints(),
            BTreeSet::from([replacement_endpoint])
        );

        supervisor.reconcile_discovery(
            &DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY),
            NetworkDiscoveryOwner::Platform,
        );
        assert!(supervisor
            .discovered_services
            .udp_targets
            .by_service
            .is_empty());
    }

    #[tokio::test]
    async fn an_established_udp_peer_stops_dns_sd_retries_without_owning_target_liveness() {
        let (mut supervisor, _guard) = test_supervisor();
        let scope_id = 7;
        let our_link_local = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
        let peer_link_local = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 8);
        supervisor.brains.insert(
            scope_id,
            AutoWifiBrain::from_link_local_with_group(our_link_local, contract::GROUP_ID),
        );
        let udp_endpoint = udp_discovery_endpoint(8, scope_id);
        let mut snapshot = DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY);
        insert_advertisement(&mut snapshot, "peer", udp_endpoint);
        let newly_active =
            supervisor.reconcile_discovery(&snapshot, NetworkDiscoveryOwner::Platform);
        assert_eq!(
            supervisor
                .prepare_udp_peering_probes(newly_active.iter())
                .len(),
            1
        );

        let peer_token = contract::peering_token(&peer_link_local);
        assert!(matches!(
            supervisor.ingest_beacon(
                SocketAddr::V6(SocketAddrV6::new(
                    peer_link_local,
                    contract::UNICAST_DISCOVERY_PORT,
                    0,
                    scope_id,
                )),
                peer_token.as_bytes(),
                1,
                BeaconChannel::Unicast,
            ),
            PeeringTokenReply::Send(_)
        ));
        assert_eq!(
            supervisor.ingest_beacon(
                SocketAddr::V6(SocketAddrV6::new(
                    peer_link_local,
                    contract::UNICAST_DISCOVERY_PORT,
                    0,
                    scope_id,
                )),
                peer_token.as_bytes(),
                2,
                BeaconChannel::Unicast,
            ),
            PeeringTokenReply::NotRequired
        );
        assert_eq!(supervisor.members.len(), 1);
        assert!(supervisor
            .prepare_udp_peering_probes(
                supervisor
                    .discovered_services
                    .udp_targets
                    .active_endpoints()
            )
            .is_empty());

        supervisor.reconcile_discovery(
            &DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY),
            NetworkDiscoveryOwner::Platform,
        );
        assert!(supervisor
            .discovered_services
            .udp_targets
            .by_service
            .is_empty());
        assert_eq!(
            supervisor.members.len(),
            1,
            "authenticated peer liveness remains owned by AutoInterface"
        );
    }

    #[test]
    fn udp_peer_and_target_state_have_the_shared_u8_ceiling() {
        assert_eq!(
            peer_member_admission(
                PeerMembership::New,
                usize::from(UDP_PEER_CAPACITY.get()) - 1
            ),
            PeerMemberAdmission::Available
        );
        assert_eq!(
            peer_member_admission(PeerMembership::New, usize::from(UDP_PEER_CAPACITY.get())),
            PeerMemberAdmission::AtCapacity
        );
        assert_eq!(
            peer_member_admission(
                PeerMembership::Existing,
                usize::from(UDP_PEER_CAPACITY.get())
            ),
            PeerMemberAdmission::Existing
        );

        let mut auto_interface_protocol = AutoWifiBrain::from_link_local_with_group(
            "fe80::ffff".parse().unwrap(),
            contract::GROUP_ID,
        );
        for suffix in 1..=u16::from(UDP_PEER_CAPACITY.get()) + 1 {
            let peer = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, suffix);
            let token = contract::peering_token(&peer);
            auto_interface_protocol.ingest_discovery_datagram(peer, token.as_bytes(), 1);
        }
        assert_eq!(
            auto_interface_protocol.peer_count(),
            usize::from(UDP_PEER_CAPACITY.get())
        );

        let one_target = udp_discovery_endpoint(8, 7);
        let mut snapshot = DiscoverySnapshot::new(contract::DEFAULT_DISCOVERY_SERVICE_CAPACITY);
        for index in 0..u8::MAX {
            insert_advertisement(&mut snapshot, &format!("peer-{index}"), one_target);
        }
        let mut targets = DiscoveredUdpTargets::new();
        let selected = selected_discovery_endpoints(
            &snapshot,
            DiscoveryTransport::Udp,
            NetworkDiscoveryOwner::Platform,
            &[],
        );
        targets.reconcile(selected);
        assert_eq!(targets.by_service.len(), usize::from(u8::MAX));
        assert_eq!(targets.active_endpoints(), BTreeSet::from([one_target]));
    }

    #[tokio::test]
    async fn udp_probes_use_one_shared_socket() {
        let receiver = UdpSocket::bind("[::1]:0").await.unwrap();
        let sender = UdpSocket::bind("[::1]:0").await.unwrap();
        let SocketAddr::V6(target) = receiver.local_addr().unwrap() else {
            panic!("test receiver is IPv6");
        };
        let peering_token = [0xa5; 32];
        send_udp_peering_probes(
            Some(&sender),
            vec![UdpPeeringProbe {
                target,
                peering_token,
            }],
        )
        .await;

        let mut received = [0u8; 32];
        let (received_bytes, _source) = receiver.recv_from(&mut received).await.unwrap();
        assert_eq!(received_bytes, peering_token.len());
        assert_eq!(received, peering_token);
    }

    #[test]
    fn multihomed_services_choose_the_shared_subnet_and_deduplicate_endpoints() {
        let wrong = DiscoveryEndpoint::tcp("10.0.0.7:42699".parse().unwrap()).unwrap();
        let right = DiscoveryEndpoint::tcp("192.168.50.7:42699".parse().unwrap()).unwrap();
        let mut first = contract::ServiceAdvertisement::new(
            contract::DiscoveryServiceName::from_instance(
                "first",
                contract::DiscoveryTransport::Tcp,
            )
            .unwrap(),
        );
        let _ = first.insert(wrong);
        let _ = first.insert(right);
        let mut second = contract::ServiceAdvertisement::new(
            contract::DiscoveryServiceName::from_instance(
                "second",
                contract::DiscoveryTransport::Tcp,
            )
            .unwrap(),
        );
        let _ = second.insert(right);
        let mut snapshot = DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY);
        let _ = snapshot.insert(first);
        let _ = snapshot.insert(second);
        let prefixes = [prefix([192, 168, 50, 2], [255, 255, 255, 0])];

        let selected = selected_discovery_endpoints(
            &snapshot,
            DiscoveryTransport::Tcp,
            NetworkDiscoveryOwner::Host,
            &prefixes,
        );
        assert_eq!(
            selected.values().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([right])
        );
    }

    #[test]
    fn link_local_candidates_must_match_the_local_interface_scope() {
        let prefix = LocalPrefix {
            addr: "fe80::1".parse().unwrap(),
            netmask: "ffff:ffff:ffff:ffff::".parse().unwrap(),
            index: 7,
        };
        let matching = DiscoveryEndpoint::tcp(SocketAddr::V6(SocketAddrV6::new(
            "fe80::2".parse().unwrap(),
            contract::TCP_RENDEZVOUS_PORT,
            0,
            7,
        )))
        .unwrap();
        let other_interface = DiscoveryEndpoint::tcp(SocketAddr::V6(SocketAddrV6::new(
            "fe80::2".parse().unwrap(),
            contract::TCP_RENDEZVOUS_PORT,
            0,
            8,
        )))
        .unwrap();
        assert!(endpoint_is_eligible(
            matching,
            NetworkDiscoveryOwner::Host,
            &[prefix]
        ));
        assert!(!endpoint_is_eligible(
            other_interface,
            NetworkDiscoveryOwner::Host,
            &[prefix]
        ));
    }

    #[tokio::test]
    async fn custom_group_nodes_never_dial_the_stock_loopback_rendezvous() {
        let (mut supervisor, _guard) = test_supervisor();
        let custom = AutoWifiSettings::new(
            b"field-mesh".to_vec(),
            contract::DiscoveryScope::Site,
            contract::MulticastAddressType::Permanent,
            30_000,
            40_000,
            AutoWifiDevicePolicy::default(),
        )
        .expect("ports are valid");
        supervisor.settings = custom;
        supervisor.ensure_loopback();
        assert!(supervisor.loopback.is_none());

        supervisor.settings = AutoWifiSettings::default();
        supervisor.ensure_loopback();
        assert!(supervisor.loopback.is_some());
        supervisor.remove_loopback();
    }

    #[test]
    fn only_address_in_use_is_classified_as_a_local_satellite() {
        assert_eq!(
            classify_rendezvous_bind_error(&io::Error::from(io::ErrorKind::AddrInUse)),
            RendezvousBindFailure::Satellite
        );
        assert_eq!(
            classify_rendezvous_bind_error(&io::Error::from(io::ErrorKind::PermissionDenied)),
            RendezvousBindFailure::Unavailable(io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn the_parent_connection_follows_the_best_live_child_state() {
        let id = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, contract::GROUP_ID);
        let status = AutoWifiStatus::new(id);
        let first = TokioInterfaceStatus::new_unaccounted(
            InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"first"),
            ConnectionState::Initializing,
        );
        let second = TokioInterfaceStatus::new_unaccounted(
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
        let status = AutoWifiStatus::new(id);
        let first = TokioInterfaceStatus::new_unaccounted(
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

        let reconnected = TokioInterfaceStatus::new_unaccounted(
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
