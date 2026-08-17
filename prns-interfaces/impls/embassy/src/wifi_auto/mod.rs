mod fanout;
mod rendezvous;
mod service_discovery;

use ::core::cell::Cell;
use ::core::net::Ipv6Addr;

use embassy_futures::join::join;
use embassy_futures::select::{select, select4, select5, Either, Either4, Either5};
use embassy_net::udp::{BindError as UdpBindError, RecvError, UdpMetadata, UdpSocket};
use embassy_net::{IpAddress, MulticastError, Stack};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, RawMutex};
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;
use embassy_time::{with_timeout, Duration, Instant, Ticker};
use heapless::Vec;
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::{
    BitrateBps, ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus, MacAddress,
};
use prns_runtime::runtime::{EmbassyFleet as Fleet, OutboundFrame};

use fanout::{dispatch_fanout, send_beacon, target_includes, FanoutPlan, UdpFanoutSender};
use rendezvous::TcpRendezvousEvent;
pub use rendezvous::{
    tcp_rendezvous, TcpRendezvousBuffers, TcpRendezvousClient, TcpRendezvousExitCause,
    TcpRendezvousServer, TcpRendezvousStorage, TcpRendezvousWireSlot, TcpRendezvousWriteFailure,
    TCP_RENDEZVOUS_FRAMED_LEN, TCP_RENDEZVOUS_FRAME_CAP, TCP_RENDEZVOUS_LIVENESS_TIMEOUT,
    TCP_RENDEZVOUS_READ_BUFFER_BYTES, TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES,
};
use service_discovery::{
    DiscoveryParticipationReceiver, EmbeddedDiscoveryParticipation,
    EMBEDDED_SERVICE_DISCOVERY_CAPACITY as SERVICE_DISCOVERY_INSTANCES,
};
pub use service_discovery::{
    UdpServiceDiscovery, UdpServiceDiscoveryConstructionError, UdpServiceDiscoveryStorage,
    EMBEDDED_SERVICE_DISCOVERY_CAPACITY, UDP_SERVICE_DISCOVERY_PACKET_BYTES,
    UDP_SERVICE_DISCOVERY_RECEIVE_PACKET_BYTES, UDP_SERVICE_DISCOVERY_RX_QUEUED_PACKETS,
    UDP_SERVICE_DISCOVERY_RX_SOCKET_BYTES, UDP_SERVICE_DISCOVERY_RX_SOCKET_METADATA,
    UDP_SERVICE_DISCOVERY_SOCKET_COUNT, UDP_SERVICE_DISCOVERY_TX_QUEUED_PACKETS,
    UDP_SERVICE_DISCOVERY_TX_SOCKET_BYTES, UDP_SERVICE_DISCOVERY_TX_SOCKET_METADATA,
};

const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
const UNICAST_REPEER_EVERY: u32 = 3;
const SEND_TIMEOUT: Duration = Duration::from_millis(300);
const BEACON_FAILURES_BEFORE_DEGRADED: u8 = 3;
const SECONDARY_STACK_CONFIGURATION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecondaryStackReadiness {
    Ready,
    Disabled,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentActivationError {
    MulticastDiscoveryBind(UdpBindError),
    UnicastDiscoveryBind(UdpBindError),
    DataBind(UdpBindError),
    MulticastJoin(MulticastError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecondaryActivationError {
    StackConfigurationTimedOut,
    Segment(SegmentActivationError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopologyState {
    PrimaryOnly,
    DualSegment,
    SecondaryUnavailable,
}

impl TopologyState {
    const fn connection_state(self) -> ConnectionState {
        match self {
            Self::PrimaryOnly | Self::DualSegment => ConnectionState::Connected,
            Self::SecondaryUnavailable => ConnectionState::Degraded,
        }
    }

    const fn requires_secondary_health(self) -> bool {
        matches!(self, Self::DualSegment | Self::SecondaryUnavailable)
    }
}

async fn wait_for_primary_stack<const MEMBERS: usize>(
    stack: &Stack<'_>,
    status: AutoWifiStatus<MEMBERS>,
) {
    loop {
        if !status.is_enabled() {
            status.wait_until_enabled().await;
        }
        match select(stack.wait_config_up(), status.wait_until_disabled()).await {
            Either::First(()) => return,
            Either::Second(()) => {}
        }
    }
}

async fn wait_for_secondary_stack<const MEMBERS: usize>(
    stack: &Stack<'_>,
    status: AutoWifiStatus<MEMBERS>,
) -> SecondaryStackReadiness {
    if !status.is_enabled() {
        return SecondaryStackReadiness::Disabled;
    }
    match select(
        with_timeout(
            SECONDARY_STACK_CONFIGURATION_TIMEOUT,
            stack.wait_config_up(),
        ),
        status.wait_until_disabled(),
    )
    .await
    {
        Either::First(Ok(())) => SecondaryStackReadiness::Ready,
        Either::First(Err(_timeout)) => SecondaryStackReadiness::TimedOut,
        Either::Second(()) => SecondaryStackReadiness::Disabled,
    }
}

fn activate_segment(segment: &mut AutoWifiSegment<'_>) -> Result<(), SegmentActivationError> {
    segment
        .discovery
        .bind(contract::DEFAULT_DISCOVERY_PORT)
        .map_err(SegmentActivationError::MulticastDiscoveryBind)?;
    segment
        .unicast_discovery
        .bind(contract::UNICAST_DISCOVERY_PORT)
        .map_err(SegmentActivationError::UnicastDiscoveryBind)?;
    segment
        .data
        .bind(contract::DEFAULT_DATA_PORT)
        .map_err(SegmentActivationError::DataBind)?;
    segment
        .stack
        .join_multicast_group(IpAddress::Ipv6(contract::DISCOVERY_GROUP))
        .map_err(SegmentActivationError::MulticastJoin)
}

async fn activate_secondary_segment<const MEMBERS: usize>(
    segment: &mut AutoWifiSegment<'_>,
    status: AutoWifiStatus<MEMBERS>,
) -> Result<(), SecondaryActivationError> {
    loop {
        match wait_for_secondary_stack(&segment.stack, status).await {
            SecondaryStackReadiness::Ready => {
                return activate_segment(segment).map_err(SecondaryActivationError::Segment);
            }
            SecondaryStackReadiness::Disabled => status.wait_until_enabled().await,
            SecondaryStackReadiness::TimedOut => {
                return Err(SecondaryActivationError::StackConfigurationTimedOut);
            }
        }
    }
}

/// The reusable slot's peer id uses a critical-section cell so cross-core readers see it coherently.
pub struct WifiMemberStatus {
    id: CriticalSectionMutex<Cell<InterfaceId>>,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    session_rx_start: AtomicU64,
    session_tx_start: AtomicU64,
    active: AtomicBool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbeddedDiscoveryTargets<const TARGETS: usize> {
    addresses: Vec<Ipv6Addr, TARGETS>,
}

impl<const TARGETS: usize> EmbeddedDiscoveryTargets<TARGETS> {
    pub(crate) const fn new() -> Self {
        Self {
            addresses: Vec::new(),
        }
    }

    pub(crate) fn insert(&mut self, address: Ipv6Addr) {
        let Err(index) = self.addresses.binary_search(&address) else {
            return;
        };
        let _ = self.addresses.insert(index, address);
    }

    fn iter(&self) -> impl Iterator<Item = Ipv6Addr> + '_ {
        self.addresses.iter().copied()
    }

    fn len(&self) -> usize {
        self.addresses.len()
    }
}

impl WifiMemberStatus {
    const fn new() -> Self {
        Self {
            id: CriticalSectionMutex::new(Cell::new(InterfaceId::new([0u8; 8]))),
            connection: AtomicU8::new(ConnectionState::Disconnected.as_u8()),
            rx: AtomicU64::new(0),
            tx: AtomicU64::new(0),
            session_rx_start: AtomicU64::new(0),
            session_tx_start: AtomicU64::new(0),
            active: AtomicBool::new(false),
        }
    }

    fn assign(&self, id: InterfaceId) {
        self.id.lock(|cell| cell.set(id));
        self.connection
            .store(ConnectionState::Connected.as_u8(), Ordering::Relaxed);
        self.session_rx_start
            .store(self.rx.load(Ordering::Relaxed), Ordering::Relaxed);
        self.session_tx_start
            .store(self.tx.load(Ordering::Relaxed), Ordering::Relaxed);
        self.active.store(true, Ordering::Relaxed);
    }

    fn add_rx(&self, bytes: u64) {
        self.rx.fetch_add(bytes, Ordering::Relaxed);
    }

    fn add_tx(&self, bytes: u64) {
        self.tx.fetch_add(bytes, Ordering::Relaxed);
    }
}

impl InterfaceStatus for WifiMemberStatus {
    fn id(&self) -> InterfaceId {
        self.id.lock(|cell| cell.get())
    }

    fn connection(&self) -> ConnectionState {
        ConnectionState::from_u8(self.connection.load(Ordering::Relaxed))
    }

    fn rx_bytes(&self) -> u64 {
        self.rx
            .load(Ordering::Relaxed)
            .saturating_sub(self.session_rx_start.load(Ordering::Relaxed))
    }

    fn tx_bytes(&self) -> u64 {
        self.tx
            .load(Ordering::Relaxed)
            .saturating_sub(self.session_tx_start.load(Ordering::Relaxed))
    }
}

pub struct AutoWifiShared<const MEMBERS: usize> {
    id: InterfaceId,
    enabled: AtomicBool,
    enabled_changed: Signal<CriticalSectionRawMutex, bool>,
    discovery_participation: Watch<
        CriticalSectionRawMutex,
        EmbeddedDiscoveryParticipation,
        { SERVICE_DISCOVERY_INSTANCES as usize },
    >,
    discovery_targets: Signal<CriticalSectionRawMutex, EmbeddedDiscoveryTargets<MEMBERS>>,
    station_uplink_enabled: AtomicBool,
    station_uplink_enabled_changed: Signal<CriticalSectionRawMutex, bool>,
    lifecycle: AtomicU8,
    peers: AtomicU32,
    members: [WifiMemberStatus; MEMBERS],
}

impl<const MEMBERS: usize> AutoWifiShared<MEMBERS> {
    #[must_use]
    pub const fn new(id: InterfaceId) -> Self {
        Self {
            id,
            enabled: AtomicBool::new(true),
            enabled_changed: Signal::new(),
            discovery_participation: Watch::new_with(EmbeddedDiscoveryParticipation::Central),
            discovery_targets: Signal::new(),
            station_uplink_enabled: AtomicBool::new(true),
            station_uplink_enabled_changed: Signal::new(),
            lifecycle: AtomicU8::new(ConnectionState::Initializing.as_u8()),
            peers: AtomicU32::new(0),
            members: [const { WifiMemberStatus::new() }; MEMBERS],
        }
    }
}

#[derive(Clone, Copy)]
pub struct AutoWifiStatus<const MEMBERS: usize> {
    shared: &'static AutoWifiShared<MEMBERS>,
}

impl<const MEMBERS: usize> AutoWifiStatus<MEMBERS> {
    #[must_use]
    pub fn new(shared: &'static AutoWifiShared<MEMBERS>) -> Self {
        Self { shared }
    }

    pub fn enable(&self) {
        self.update_enabled(true);
    }

    pub fn disable(&self) {
        self.update_enabled(false);
    }

    pub fn toggle_enabled(&self) {
        let enabled = !self.shared.enabled.fetch_xor(true, Ordering::Relaxed);
        self.publish_enabled_change(enabled);
    }

    fn update_enabled(&self, enabled: bool) {
        if self.shared.enabled.swap(enabled, Ordering::Relaxed) != enabled {
            self.publish_enabled_change(enabled);
        }
    }

    fn publish_enabled_change(&self, enabled: bool) {
        self.shared.enabled_changed.signal(enabled);
        self.shared
            .discovery_participation
            .sender()
            .send(if enabled {
                EmbeddedDiscoveryParticipation::Central
            } else {
                EmbeddedDiscoveryParticipation::Inactive
            });
    }

    fn discovery_participation_receiver(
        &self,
    ) -> Result<
        DiscoveryParticipationReceiver,
        service_discovery::UdpServiceDiscoveryConstructionError,
    > {
        self.shared.discovery_participation.receiver().ok_or(
            service_discovery::UdpServiceDiscoveryConstructionError::DiscoveryCapacityExhausted,
        )
    }

    fn publish_discovery_targets(&self, targets: EmbeddedDiscoveryTargets<MEMBERS>) {
        self.shared.discovery_targets.signal(targets);
    }

    async fn wait_for_discovery_targets(&self) -> EmbeddedDiscoveryTargets<MEMBERS> {
        self.shared.discovery_targets.wait().await
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    pub async fn wait_until_enabled(&self) {
        Self::wait_for_state(true, &self.shared.enabled, &self.shared.enabled_changed).await;
    }

    pub async fn wait_until_disabled(&self) {
        Self::wait_for_state(false, &self.shared.enabled, &self.shared.enabled_changed).await;
    }

    pub fn enable_station_uplink(&self) {
        self.update_station_uplink_enabled(true);
    }

    pub fn disable_station_uplink(&self) {
        self.update_station_uplink_enabled(false);
    }

    pub fn toggle_station_uplink(&self) {
        let enabled = !self
            .shared
            .station_uplink_enabled
            .fetch_xor(true, Ordering::Relaxed);
        self.shared.station_uplink_enabled_changed.signal(enabled);
    }

    fn update_station_uplink_enabled(&self, enabled: bool) {
        if self
            .shared
            .station_uplink_enabled
            .swap(enabled, Ordering::Relaxed)
            != enabled
        {
            self.shared.station_uplink_enabled_changed.signal(enabled);
        }
    }

    #[must_use]
    pub fn is_station_uplink_enabled(&self) -> bool {
        self.shared.station_uplink_enabled.load(Ordering::Relaxed)
    }

    pub async fn wait_until_station_uplink_enabled(&self) {
        Self::wait_for_state(
            true,
            &self.shared.station_uplink_enabled,
            &self.shared.station_uplink_enabled_changed,
        )
        .await;
    }

    pub async fn wait_until_station_uplink_disabled(&self) {
        Self::wait_for_state(
            false,
            &self.shared.station_uplink_enabled,
            &self.shared.station_uplink_enabled_changed,
        )
        .await;
    }

    async fn wait_for_state(
        enabled: bool,
        state: &AtomicBool,
        changed: &Signal<CriticalSectionRawMutex, bool>,
    ) {
        loop {
            if state.load(Ordering::Relaxed) == enabled {
                return;
            }
            if changed.wait().await == enabled {
                return;
            }
        }
    }

    fn member(&self, slot: usize) -> &'static WifiMemberStatus {
        &self.shared.members[slot]
    }

    fn retire_member(&self, slot: usize) {
        let member = self.member(slot);
        if !member.active.swap(false, Ordering::Relaxed) {
            return;
        }
        member
            .connection
            .store(ConnectionState::Disconnected.as_u8(), Ordering::Relaxed);
    }

    fn republish_peer_count(&self) {
        let count = self
            .shared
            .members
            .iter()
            .filter(|member| member.active.load(Ordering::Relaxed))
            .count();
        self.shared.peers.store(count as u32, Ordering::Relaxed);
    }

    fn set_lifecycle(&self, connection: ConnectionState) {
        self.shared
            .lifecycle
            .store(connection.as_u8(), Ordering::Relaxed);
    }

    pub fn members(&self) -> impl Iterator<Item = &'static WifiMemberStatus> {
        self.shared
            .members
            .iter()
            .filter(|member| member.active.load(Ordering::Relaxed))
    }

    pub fn peer_count(&self) -> u32 {
        self.shared.peers.load(Ordering::Relaxed)
    }
}

impl<const MEMBERS: usize> InterfaceStatus for AutoWifiStatus<MEMBERS> {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            return ConnectionState::Disabled;
        }
        ConnectionState::from_u8(self.shared.lifecycle.load(Ordering::Relaxed))
    }

    fn rx_bytes(&self) -> u64 {
        self.shared
            .members
            .iter()
            .map(|member| member.rx.load(Ordering::Relaxed))
            .sum()
    }

    fn tx_bytes(&self) -> u64 {
        self.shared
            .members
            .iter()
            .map(|member| member.tx.load(Ordering::Relaxed))
            .sum()
    }
}

pub struct AutoWifiSegment<'a> {
    pub stack: Stack<'a>,
    pub discovery: UdpSocket<'a>,
    pub unicast_discovery: UdpSocket<'a>,
    pub data: UdpSocket<'a>,
    pub mac: [u8; 6],
}

pub struct AutoWifiTopology<'a> {
    pub primary: AutoWifiSegment<'a>,
    pub secondary: Option<AutoWifiSegment<'a>>,
    pub rendezvous: Option<TcpRendezvousClient<'a>>,
}

pub struct AutoWifi<'a, const MEMBERS: usize> {
    primary: AutoWifiSegment<'a>,
    secondary: Option<AutoWifiSegment<'a>>,
    brain: contract::FixedAutoInterfaceProtocol<MEMBERS>,
    status: AutoWifiStatus<MEMBERS>,
    bitrate: BitrateBps,
    rendezvous: Option<TcpRendezvousClient<'a>>,
}

type DatagramReceiveResult = Result<(usize, UdpMetadata), RecvError>;

enum AutoWifiEvent<'a, const FRAME: usize, const MEMBERS: usize> {
    PrimaryMulticast(DatagramReceiveResult),
    PrimaryUnicast(DatagramReceiveResult),
    PrimaryData(DatagramReceiveResult),
    BeaconTick,
    Outbound(OutboundFrame<FRAME>),
    SecondaryMulticast(DatagramReceiveResult),
    SecondaryUnicast(DatagramReceiveResult),
    SecondaryData(DatagramReceiveResult),
    Rendezvous(&'a mut TcpRendezvousWireSlot),
    DiscoveryTargets(EmbeddedDiscoveryTargets<MEMBERS>),
    Disabled,
}

struct AutoWifiReceiveBuffers<'a> {
    primary_multicast: &'a mut [u8],
    primary_unicast: &'a mut [u8],
    primary_data: &'a mut [u8],
    secondary_multicast: &'a mut [u8],
    secondary_unicast: &'a mut [u8],
    secondary_data: &'a mut [u8],
}

struct AutoWifiRunState<const MEMBERS: usize> {
    topology: TopologyState,
    peers: [Option<Ipv6Addr>; MEMBERS],
    ids: [InterfaceId; MEMBERS],
    peer_on_secondary: [bool; MEMBERS],
    tcp_slot: Option<usize>,
    tcp_peer: Option<TcpPeer>,
    primary_token: [u8; contract::PEERING_TOKEN_BYTES],
    secondary_token: Option<[u8; contract::PEERING_TOKEN_BYTES]>,
    fanout_start: usize,
    consecutive_beacon_failures: u8,
    beacon_cycle: u32,
    discovered_targets: EmbeddedDiscoveryTargets<MEMBERS>,
}

impl<const MEMBERS: usize> AutoWifiRunState<MEMBERS> {
    fn new(auto_wifi: &AutoWifi<'_, MEMBERS>, topology: TopologyState) -> Self {
        let primary_token = *auto_wifi.brain.our_peering_token().as_bytes();
        let secondary_token = auto_wifi.secondary.as_ref().map(|segment| {
            let link_local = contract::link_local_from_mac(MacAddress::new(segment.mac));
            *contract::peering_token(&link_local).as_bytes()
        });
        Self {
            topology,
            peers: [None; MEMBERS],
            ids: [InterfaceId::new([0u8; 8]); MEMBERS],
            peer_on_secondary: [false; MEMBERS],
            tcp_slot: auto_wifi
                .rendezvous
                .as_ref()
                .and_then(|_| MEMBERS.checked_sub(1)),
            tcp_peer: None,
            primary_token,
            secondary_token,
            fanout_start: 0,
            consecutive_beacon_failures: 0,
            beacon_cycle: 0,
            discovered_targets: EmbeddedDiscoveryTargets::new(),
        }
    }

    fn secondary_token(&self) -> &[u8; contract::PEERING_TOKEN_BYTES] {
        match self.secondary_token.as_ref() {
            Some(token) => token,
            None => &self.primary_token,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeaconMaintenanceOutcome {
    Completed,
    Disabled,
}

impl<'a, const MEMBERS: usize> AutoWifi<'a, MEMBERS> {
    #[must_use]
    pub fn aggregate_id() -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, contract::GROUP_ID)
    }

    #[must_use]
    pub fn new(topology: AutoWifiTopology<'a>, shared: &'static AutoWifiShared<MEMBERS>) -> Self {
        let brain =
            contract::FixedAutoInterfaceProtocol::new(MacAddress::new(topology.primary.mac));
        Self {
            primary: topology.primary,
            secondary: topology.secondary,
            brain,
            status: AutoWifiStatus::new(shared),
            bitrate: contract::WIFI_LAN_BITRATE_BPS
                .min(contract::WIFI_EMBEDDED_BITRATE_CEILING_BPS),
            rendezvous: topology.rendezvous,
        }
    }

    #[must_use]
    pub fn status(&self) -> AutoWifiStatus<MEMBERS> {
        self.status
    }

    async fn handle_disablement<
        M: RawMutex + 'static,
        const FRAME: usize,
        const NOTIFY: usize,
        const LIFECYCLE: usize,
    >(
        &mut self,
        state: &mut AutoWifiRunState<MEMBERS>,
        fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    ) {
        clear_members(
            &mut state.peers,
            &state.ids,
            &mut state.peer_on_secondary,
            &self.status,
            fleet,
        )
        .await;
        clear_tcp_member(
            &mut self.rendezvous,
            &mut state.tcp_peer,
            &self.status,
            fleet,
        )
        .await;
        state.discovered_targets = EmbeddedDiscoveryTargets::new();
        self.status.wait_until_enabled().await;
    }

    async fn handle_primary_multicast<
        M: RawMutex + 'static,
        const FRAME: usize,
        const NOTIFY: usize,
        const LIFECYCLE: usize,
    >(
        &mut self,
        state: &mut AutoWifiRunState<MEMBERS>,
        fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
        received: DatagramReceiveResult,
        bytes: &[u8],
    ) {
        if let Ok((len, meta)) = received {
            if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                ingest_beacon(
                    &mut self.brain,
                    &mut state.peers,
                    &mut state.ids,
                    &mut state.peer_on_secondary,
                    &self.status,
                    fleet,
                    self.bitrate,
                    src,
                    &bytes[..len],
                    Instant::now().as_millis(),
                    false,
                    state.tcp_slot,
                    BeaconChannel::Multicast,
                    &state.primary_token,
                )
                .await;
            }
        }
    }

    async fn handle_primary_unicast<
        M: RawMutex + 'static,
        const FRAME: usize,
        const NOTIFY: usize,
        const LIFECYCLE: usize,
    >(
        &mut self,
        state: &mut AutoWifiRunState<MEMBERS>,
        fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
        received: DatagramReceiveResult,
        bytes: &[u8],
    ) {
        if let Ok((len, meta)) = received {
            if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                let peering_token_reply = ingest_beacon(
                    &mut self.brain,
                    &mut state.peers,
                    &mut state.ids,
                    &mut state.peer_on_secondary,
                    &self.status,
                    fleet,
                    self.bitrate,
                    src,
                    &bytes[..len],
                    Instant::now().as_millis(),
                    false,
                    state.tcp_slot,
                    BeaconChannel::Unicast,
                    &state.primary_token,
                )
                .await;
                send_peering_token_reply(&self.primary.unicast_discovery, peering_token_reply)
                    .await;
            }
        }
    }

    async fn handle_secondary_multicast<
        M: RawMutex + 'static,
        const FRAME: usize,
        const NOTIFY: usize,
        const LIFECYCLE: usize,
    >(
        &mut self,
        state: &mut AutoWifiRunState<MEMBERS>,
        fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
        received: DatagramReceiveResult,
        bytes: &[u8],
    ) {
        if let Ok((len, meta)) = received {
            if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                let secondary_token = *state.secondary_token();
                ingest_beacon(
                    &mut self.brain,
                    &mut state.peers,
                    &mut state.ids,
                    &mut state.peer_on_secondary,
                    &self.status,
                    fleet,
                    self.bitrate,
                    src,
                    &bytes[..len],
                    Instant::now().as_millis(),
                    true,
                    state.tcp_slot,
                    BeaconChannel::Multicast,
                    &secondary_token,
                )
                .await;
            }
        }
    }

    async fn handle_secondary_unicast<
        M: RawMutex + 'static,
        const FRAME: usize,
        const NOTIFY: usize,
        const LIFECYCLE: usize,
    >(
        &mut self,
        state: &mut AutoWifiRunState<MEMBERS>,
        fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
        received: DatagramReceiveResult,
        bytes: &[u8],
    ) {
        if let Ok((len, meta)) = received {
            if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                let secondary_token = *state.secondary_token();
                let peering_token_reply = ingest_beacon(
                    &mut self.brain,
                    &mut state.peers,
                    &mut state.ids,
                    &mut state.peer_on_secondary,
                    &self.status,
                    fleet,
                    self.bitrate,
                    src,
                    &bytes[..len],
                    Instant::now().as_millis(),
                    true,
                    state.tcp_slot,
                    BeaconChannel::Unicast,
                    &secondary_token,
                )
                .await;
                if let Some(secondary) = self.secondary.as_ref() {
                    send_peering_token_reply(&secondary.unicast_discovery, peering_token_reply)
                        .await;
                }
            }
        }
    }

    fn handle_inbound_data<
        M: RawMutex + 'static,
        const FRAME: usize,
        const NOTIFY: usize,
        const LIFECYCLE: usize,
    >(
        &self,
        state: &AutoWifiRunState<MEMBERS>,
        fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
        received: DatagramReceiveResult,
        bytes: &[u8],
    ) {
        if let Ok((len, meta)) = received {
            if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                route_inbound(
                    fleet,
                    &state.peers,
                    &state.ids,
                    &self.status,
                    src,
                    &bytes[..len],
                );
            }
        }
    }

    async fn handle_beacon_tick<
        M: RawMutex + 'static,
        const FRAME: usize,
        const NOTIFY: usize,
        const LIFECYCLE: usize,
    >(
        &mut self,
        state: &mut AutoWifiRunState<MEMBERS>,
        fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    ) -> BeaconMaintenanceOutcome {
        state.beacon_cycle = state.beacon_cycle.wrapping_add(1);
        let sends = with_timeout(
            SEND_TIMEOUT,
            join(
                send_beacon(Some(&self.primary.discovery), Some(&state.primary_token)),
                send_beacon(
                    self.secondary.as_ref().map(|segment| &segment.discovery),
                    state.secondary_token.as_ref(),
                ),
            ),
        );
        let sent = match select(self.status.wait_until_disabled(), sends).await {
            Either::First(()) => return BeaconMaintenanceOutcome::Disabled,
            Either::Second(Ok((primary, secondary))) => {
                primary && (!state.topology.requires_secondary_health() || secondary)
            }
            Either::Second(Err(_timeout)) => false,
        };
        if sent {
            state.consecutive_beacon_failures = 0;
            self.status.set_lifecycle(state.topology.connection_state());
        } else {
            state.consecutive_beacon_failures = state.consecutive_beacon_failures.saturating_add(1);
            if state.consecutive_beacon_failures >= BEACON_FAILURES_BEFORE_DEGRADED {
                self.status.set_lifecycle(ConnectionState::Degraded);
            }
        }
        retire_stale(
            &mut self.brain,
            &mut state.peers,
            &state.ids,
            &mut state.peer_on_secondary,
            &self.status,
            fleet,
            Instant::now().as_millis(),
        )
        .await;
        if state.beacon_cycle.is_multiple_of(UNICAST_REPEER_EVERY) {
            send_discovery_probes(
                &self.primary.unicast_discovery,
                &state.discovered_targets,
                &self.brain,
            )
            .await;
        }
        BeaconMaintenanceOutcome::Completed
    }

    async fn handle_outbound<const FRAME: usize>(
        &mut self,
        state: &mut AutoWifiRunState<MEMBERS>,
        outbound: OutboundFrame<FRAME>,
    ) {
        if outbound.is_empty() {
            return;
        }
        let mut plan = FanoutPlan::new(
            outbound.target(),
            &state.peers,
            &state.ids,
            state.fanout_start,
        );
        if MEMBERS > 0 {
            state.fanout_start = (state.fanout_start + 1) % MEMBERS;
        }
        let mut sender = UdpFanoutSender {
            primary: &self.primary.data,
            secondary: self.secondary.as_ref().map(|segment| &segment.data),
            peers: &state.peers,
            peer_on_secondary: &state.peer_on_secondary,
            status: self.status,
            bytes: outbound.bytes(),
        };
        let dispatch = dispatch_fanout(&mut plan, &mut sender, SEND_TIMEOUT);
        let _ = select(self.status.wait_until_disabled(), dispatch).await;
        if let (Some(rendezvous), Some(peer)) = (self.rendezvous.as_mut(), state.tcp_peer) {
            if target_includes(outbound.target(), peer.id) {
                let send = rendezvous.send_frame(peer.session, outbound.bytes());
                if matches!(
                    select(self.status.wait_until_disabled(), send).await,
                    Either::Second(Ok(()))
                ) {
                    self.status.member(peer.slot).add_tx(outbound.len() as u64);
                }
            }
        }
    }

    async fn handle_discovery_targets(
        &self,
        state: &mut AutoWifiRunState<MEMBERS>,
        targets: EmbeddedDiscoveryTargets<MEMBERS>,
    ) {
        state.discovered_targets = targets;
        send_discovery_probes(
            &self.primary.unicast_discovery,
            &state.discovered_targets,
            &self.brain,
        )
        .await;
    }

    pub async fn run<M, const FRAME: usize, const NOTIFY: usize, const LIFECYCLE: usize>(
        mut self,
        mut fleet: Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
        data_buf: &mut [u8],
        sec_data_buf: &mut [u8],
    ) where
        M: RawMutex + 'static,
    {
        wait_for_primary_stack(&self.primary.stack, self.status).await;
        match activate_segment(&mut self.primary) {
            Ok(()) => crate::diagnostic_log::debug!("wifi-auto: primary segment active"),
            Err(error) => {
                crate::diagnostic_log::warn!(
                    "wifi-auto: primary segment activation failed: {error:?}"
                );
                self.status.set_lifecycle(ConnectionState::Failed);
                return;
            }
        }

        let topology = match self.secondary.as_mut() {
            None => {
                crate::diagnostic_log::debug!("wifi-auto: secondary segment not configured");
                TopologyState::PrimaryOnly
            }
            Some(segment) => match activate_secondary_segment(segment, self.status).await {
                Ok(()) => {
                    crate::diagnostic_log::debug!("wifi-auto: secondary segment active");
                    TopologyState::DualSegment
                }
                Err(error) => {
                    crate::diagnostic_log::warn!(
                        "wifi-auto: secondary segment unavailable: {error:?}"
                    );
                    self.secondary = None;
                    TopologyState::SecondaryUnavailable
                }
            },
        };
        self.status.set_lifecycle(topology.connection_state());

        let mut state = AutoWifiRunState::new(&self, topology);
        let mut beacon = Ticker::every(BEACON_INTERVAL);
        let mut primary_multicast = [0u8; 64];
        let mut primary_unicast = [0u8; 64];
        let mut secondary_multicast = [0u8; 64];
        let mut secondary_unicast = [0u8; 64];
        let mut receive_buffers = AutoWifiReceiveBuffers {
            primary_multicast: &mut primary_multicast,
            primary_unicast: &mut primary_unicast,
            primary_data: data_buf,
            secondary_multicast: &mut secondary_multicast,
            secondary_unicast: &mut secondary_unicast,
            secondary_data: sec_data_buf,
        };

        loop {
            if !self.status.is_enabled() {
                self.handle_disablement(&mut state, &fleet).await;
            }
            let event = next_auto_wifi_event(
                &self.primary,
                &self.secondary,
                &mut self.rendezvous,
                self.status,
                &mut fleet,
                &mut beacon,
                &mut receive_buffers,
            )
            .await;
            match event {
                AutoWifiEvent::PrimaryMulticast(received) => {
                    self.handle_primary_multicast(
                        &mut state,
                        &fleet,
                        received,
                        receive_buffers.primary_multicast,
                    )
                    .await;
                }
                AutoWifiEvent::PrimaryUnicast(received) => {
                    self.handle_primary_unicast(
                        &mut state,
                        &fleet,
                        received,
                        receive_buffers.primary_unicast,
                    )
                    .await;
                }
                AutoWifiEvent::PrimaryData(received) => {
                    self.handle_inbound_data(
                        &state,
                        &mut fleet,
                        received,
                        receive_buffers.primary_data,
                    );
                }
                AutoWifiEvent::BeaconTick => {
                    match self.handle_beacon_tick(&mut state, &fleet).await {
                        BeaconMaintenanceOutcome::Completed => {}
                        BeaconMaintenanceOutcome::Disabled => continue,
                    }
                }
                AutoWifiEvent::Outbound(outbound) => {
                    self.handle_outbound(&mut state, outbound).await;
                }
                AutoWifiEvent::SecondaryMulticast(received) => {
                    self.handle_secondary_multicast(
                        &mut state,
                        &fleet,
                        received,
                        receive_buffers.secondary_multicast,
                    )
                    .await;
                }
                AutoWifiEvent::SecondaryUnicast(received) => {
                    self.handle_secondary_unicast(
                        &mut state,
                        &fleet,
                        received,
                        receive_buffers.secondary_unicast,
                    )
                    .await;
                }
                AutoWifiEvent::SecondaryData(received) => {
                    self.handle_inbound_data(
                        &state,
                        &mut fleet,
                        received,
                        receive_buffers.secondary_data,
                    );
                }
                AutoWifiEvent::Rendezvous(event_slot) => {
                    let rejected_session = match event_slot.event() {
                        Some(event) => {
                            handle_rendezvous_event(
                                &mut state.tcp_peer,
                                state.tcp_slot,
                                &self.status,
                                &mut fleet,
                                self.bitrate,
                                event,
                            )
                            .await
                        }
                        None => None,
                    };
                    if let Some(rendezvous) = self.rendezvous.as_mut() {
                        rendezvous.event_received();
                        if let Some(session) = rejected_session {
                            rendezvous.disconnect(session);
                        }
                    }
                }
                AutoWifiEvent::DiscoveryTargets(targets) => {
                    self.handle_discovery_targets(&mut state, targets).await;
                }
                AutoWifiEvent::Disabled => {}
            }
        }
    }
}

enum SecondaryDatagram {
    Discovery(DatagramReceiveResult),
    UnicastDiscovery(DatagramReceiveResult),
    Data(DatagramReceiveResult),
}

async fn next_auto_wifi_event<
    'r,
    M: RawMutex + 'static,
    const FRAME: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    primary: &AutoWifiSegment<'_>,
    secondary: &Option<AutoWifiSegment<'_>>,
    rendezvous: &'r mut Option<TcpRendezvousClient<'_>>,
    status: AutoWifiStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    beacon: &mut Ticker,
    buffers: &mut AutoWifiReceiveBuffers<'_>,
) -> AutoWifiEvent<'r, FRAME, MEMBERS> {
    match select(
        select5(
            primary.discovery.recv_from(buffers.primary_multicast),
            primary.unicast_discovery.recv_from(buffers.primary_unicast),
            primary.data.recv_from(buffers.primary_data),
            beacon.next(),
            fleet.next_outbound(),
        ),
        select4(
            next_secondary_datagram(
                secondary,
                buffers.secondary_multicast,
                buffers.secondary_unicast,
                buffers.secondary_data,
            ),
            next_rendezvous_event(rendezvous),
            status.wait_for_discovery_targets(),
            status.wait_until_disabled(),
        ),
    )
    .await
    {
        Either::First(Either5::First(received)) => AutoWifiEvent::PrimaryMulticast(received),
        Either::First(Either5::Second(received)) => AutoWifiEvent::PrimaryUnicast(received),
        Either::First(Either5::Third(received)) => AutoWifiEvent::PrimaryData(received),
        Either::First(Either5::Fourth(())) => AutoWifiEvent::BeaconTick,
        Either::First(Either5::Fifth(outbound)) => AutoWifiEvent::Outbound(outbound),
        Either::Second(Either4::First(SecondaryDatagram::Discovery(received))) => {
            AutoWifiEvent::SecondaryMulticast(received)
        }
        Either::Second(Either4::First(SecondaryDatagram::UnicastDiscovery(received))) => {
            AutoWifiEvent::SecondaryUnicast(received)
        }
        Either::Second(Either4::First(SecondaryDatagram::Data(received))) => {
            AutoWifiEvent::SecondaryData(received)
        }
        Either::Second(Either4::Second(event_slot)) => AutoWifiEvent::Rendezvous(event_slot),
        Either::Second(Either4::Third(targets)) => AutoWifiEvent::DiscoveryTargets(targets),
        Either::Second(Either4::Fourth(())) => AutoWifiEvent::Disabled,
    }
}

#[derive(Clone, Copy)]
struct TcpPeer {
    session: u32,
    id: InterfaceId,
    slot: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeaconChannel {
    Multicast,
    Unicast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeeringTokenReply {
    NotRequired,
    Send {
        address: Ipv6Addr,
        peering_token: [u8; contract::PEERING_TOKEN_BYTES],
    },
}

async fn next_rendezvous_event<'a>(
    rendezvous: &'a mut Option<TcpRendezvousClient<'_>>,
) -> &'a mut TcpRendezvousWireSlot {
    match rendezvous {
        Some(rendezvous) => rendezvous.next_event_slot().await,
        None => ::core::future::pending().await,
    }
}

async fn handle_rendezvous_event<
    M: RawMutex + 'static,
    const FRAME: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    tcp_peer: &mut Option<TcpPeer>,
    tcp_slot: Option<usize>,
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    bitrate: BitrateBps,
    event: TcpRendezvousEvent<'_>,
) -> Option<u32> {
    match event {
        TcpRendezvousEvent::Connected { session, id } => {
            let Some(slot) = tcp_slot else {
                return Some(session);
            };
            if let Some(previous) = tcp_peer.take() {
                fleet.deregister_member(previous.id).await;
                status.retire_member(previous.slot);
            }
            fleet
                .register_member(contract::descriptor(
                    id,
                    contract::policy_for_bitrate(bitrate),
                ))
                .await;
            status.member(slot).assign(id);
            *tcp_peer = Some(TcpPeer { session, id, slot });
            status.republish_peer_count();
            None
        }
        TcpRendezvousEvent::Frame { session, id, bytes } => {
            let peer = (*tcp_peer)?;
            if peer.session != session || peer.id != id {
                return None;
            }
            if fleet.deliver_inbound(id, bytes).await.is_ok() {
                status.member(peer.slot).add_rx(bytes.len() as u64);
            }
            None
        }
        TcpRendezvousEvent::Disconnected { session, id } => {
            let peer = (*tcp_peer)?;
            if peer.session != session || peer.id != id {
                return None;
            }
            fleet.deregister_member(id).await;
            status.retire_member(peer.slot);
            *tcp_peer = None;
            status.republish_peer_count();
            None
        }
    }
}

async fn clear_tcp_member<
    M: RawMutex + 'static,
    const FRAME: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    rendezvous: &mut Option<TcpRendezvousClient<'_>>,
    tcp_peer: &mut Option<TcpPeer>,
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
) {
    let Some(peer) = tcp_peer.take() else {
        return;
    };
    if let Some(rendezvous) = rendezvous.as_mut() {
        rendezvous.disconnect(peer.session);
    }
    fleet.deregister_member(peer.id).await;
    status.retire_member(peer.slot);
    status.republish_peer_count();
}

async fn next_secondary_datagram(
    segment: &Option<AutoWifiSegment<'_>>,
    discovery_buf: &mut [u8],
    unicast_discovery_buf: &mut [u8],
    data_buf: &mut [u8],
) -> SecondaryDatagram {
    let Some(segment) = segment else {
        return ::core::future::pending().await;
    };
    match embassy_futures::select::select3(
        segment.discovery.recv_from(discovery_buf),
        segment.unicast_discovery.recv_from(unicast_discovery_buf),
        segment.data.recv_from(data_buf),
    )
    .await
    {
        embassy_futures::select::Either3::First(received) => SecondaryDatagram::Discovery(received),
        embassy_futures::select::Either3::Second(received) => {
            SecondaryDatagram::UnicastDiscovery(received)
        }
        embassy_futures::select::Either3::Third(received) => SecondaryDatagram::Data(received),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "embedded serve-loop internals pass the loop's split-borrowed locals; bundling awaits an on-hardware validation pass"
)]
async fn ingest_beacon<
    M: RawMutex + 'static,
    const FRAME: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    brain: &mut contract::FixedAutoInterfaceProtocol<MEMBERS>,
    peers: &mut [Option<Ipv6Addr>; MEMBERS],
    ids: &mut [InterfaceId; MEMBERS],
    peer_on_secondary: &mut [bool; MEMBERS],
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    bitrate: BitrateBps,
    src: Ipv6Addr,
    bytes: &[u8],
    now_ms: u64,
    on_secondary: bool,
    reserved_slot: Option<usize>,
    beacon_channel: BeaconChannel,
    local_token: &[u8; contract::PEERING_TOKEN_BYTES],
) -> PeeringTokenReply {
    let existing_slot = peers.iter().position(|peer| *peer == Some(src));
    let available_slot = peers
        .iter()
        .enumerate()
        .position(|(slot, peer)| peer.is_none() && Some(slot) != reserved_slot);
    if existing_slot.is_none() && available_slot.is_none() {
        return PeeringTokenReply::NotRequired;
    }
    let contract::BeaconObservation::AuthenticatedPeer {
        address,
        peer_observation,
    } = brain.observe_discovery_datagram(src, bytes, now_ms)
    else {
        return PeeringTokenReply::NotRequired;
    };
    if peer_observation == contract::PeerObservation::TableFull {
        return PeeringTokenReply::NotRequired;
    }
    if let Some(slot) = existing_slot {
        peer_on_secondary[slot] = on_secondary;
    } else if peer_observation == contract::PeerObservation::NewlyDiscovered {
        let Some(slot) = available_slot else {
            return PeeringTokenReply::NotRequired;
        };
        let id = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, &address.octets());
        fleet
            .register_member(contract::descriptor(
                id,
                contract::policy_for_bitrate(bitrate),
            ))
            .await;
        peers[slot] = Some(address);
        ids[slot] = id;
        peer_on_secondary[slot] = on_secondary;
        status.member(slot).assign(id);
        status.republish_peer_count();
    }
    peering_token_reply(beacon_channel, peer_observation, address, *local_token)
}

fn peering_token_reply(
    beacon_channel: BeaconChannel,
    peer_observation: contract::PeerObservation,
    address: Ipv6Addr,
    local_peering_token: [u8; contract::PEERING_TOKEN_BYTES],
) -> PeeringTokenReply {
    match (beacon_channel, peer_observation) {
        (BeaconChannel::Unicast, contract::PeerObservation::NewlyDiscovered) => {
            PeeringTokenReply::Send {
                address,
                peering_token: local_peering_token,
            }
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

async fn send_peering_token_reply(
    unicast_discovery_socket: &UdpSocket<'_>,
    peering_token_reply: PeeringTokenReply,
) {
    let PeeringTokenReply::Send {
        address,
        peering_token,
    } = peering_token_reply
    else {
        return;
    };
    let _ = unicast_discovery_socket
        .send_to(
            &peering_token,
            (IpAddress::Ipv6(address), contract::UNICAST_DISCOVERY_PORT),
        )
        .await;
}

async fn send_discovery_probes<const TARGETS: usize, const PEERS: usize>(
    unicast_discovery_socket: &UdpSocket<'_>,
    discovery_targets: &EmbeddedDiscoveryTargets<TARGETS>,
    brain: &contract::FixedAutoInterfaceProtocol<PEERS>,
) {
    let token = brain.our_peering_token();
    for address in discovery_targets.iter().filter(|address| {
        !brain
            .known_peer_addresses()
            .any(|known_peer| known_peer == *address)
    }) {
        let _ = unicast_discovery_socket
            .send_to(
                token.as_bytes(),
                (IpAddress::Ipv6(address), contract::UNICAST_DISCOVERY_PORT),
            )
            .await;
    }
}

fn route_inbound<
    M: RawMutex + 'static,
    const FRAME: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    peers: &[Option<Ipv6Addr>; MEMBERS],
    ids: &[InterfaceId; MEMBERS],
    status: &AutoWifiStatus<MEMBERS>,
    src: Ipv6Addr,
    bytes: &[u8],
) {
    if bytes.is_empty() {
        return;
    }
    let Some(slot) = peers.iter().position(|peer| *peer == Some(src)) else {
        return;
    };
    if fleet.try_deliver_inbound(ids[slot], bytes).is_ok() {
        status.member(slot).add_rx(bytes.len() as u64);
    }
}

async fn clear_members<
    M: RawMutex + 'static,
    const FRAME: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    peers: &mut [Option<Ipv6Addr>; MEMBERS],
    ids: &[InterfaceId; MEMBERS],
    peer_on_secondary: &mut [bool; MEMBERS],
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
) {
    let mut changed = false;
    for slot in 0..MEMBERS {
        if peers[slot].is_none() {
            continue;
        }
        fleet.deregister_member(ids[slot]).await;
        peers[slot] = None;
        peer_on_secondary[slot] = false;
        status.retire_member(slot);
        changed = true;
    }
    if changed {
        status.republish_peer_count();
    }
}

async fn retire_stale<
    M: RawMutex + 'static,
    const FRAME: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    brain: &mut contract::FixedAutoInterfaceProtocol<MEMBERS>,
    peers: &mut [Option<Ipv6Addr>; MEMBERS],
    ids: &[InterfaceId; MEMBERS],
    peer_on_secondary: &mut [bool; MEMBERS],
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    now_ms: u64,
) {
    if brain.prune_stale_peers(now_ms) == 0 {
        return;
    }
    for slot in 0..MEMBERS {
        let Some(addr) = peers[slot] else { continue };
        if brain.known_peer_addresses().any(|known| known == addr) {
            continue;
        }
        fleet.deregister_member(ids[slot]).await;
        peers[slot] = None;
        peer_on_secondary[slot] = false;
        status.retire_member(slot);
    }
    status.republish_peer_count();
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::{block_on, join::join};

    static SHARED: AutoWifiShared<1> = AutoWifiShared::new(InterfaceId::new([0x5B; 8]));
    static UPLINK_SHARED: AutoWifiShared<1> = AutoWifiShared::new(InterfaceId::new([0x5E; 8]));
    static LIFECYCLE_SHARED: AutoWifiShared<1> = AutoWifiShared::new(InterfaceId::new([0x5C; 8]));
    static ACCOUNTING_SHARED: AutoWifiShared<1> = AutoWifiShared::new(InterfaceId::new([0x5D; 8]));
    static DISCOVERY_SHARED: AutoWifiShared<1> = AutoWifiShared::new(InterfaceId::new([0x5F; 8]));

    fn id(suffix: u8) -> InterfaceId {
        InterfaceId::new([InterfaceKind::WifiPeer as u8, 0, 0, 0, 0, 0, 0, suffix])
    }

    #[test]
    fn topology_health_preserves_a_configured_secondary_failure() {
        assert_eq!(
            TopologyState::PrimaryOnly.connection_state(),
            ConnectionState::Connected
        );
        assert!(!TopologyState::PrimaryOnly.requires_secondary_health());
        assert_eq!(
            TopologyState::DualSegment.connection_state(),
            ConnectionState::Connected
        );
        assert!(TopologyState::DualSegment.requires_secondary_health());
        assert_eq!(
            TopologyState::SecondaryUnavailable.connection_state(),
            ConnectionState::Degraded
        );
        assert!(TopologyState::SecondaryUnavailable.requires_secondary_health());
    }

    #[test]
    fn interface_enablement_does_not_change_station_uplink() {
        let status = AutoWifiStatus::new(&SHARED);
        status.enable();
        status.enable_station_uplink();

        block_on(async {
            join(status.wait_until_disabled(), async { status.disable() }).await;
            join(status.wait_until_enabled(), async { status.enable() }).await;
        });

        assert!(status.is_station_uplink_enabled());
    }

    #[test]
    fn station_uplink_enablement_does_not_change_interface() {
        let status = AutoWifiStatus::new(&UPLINK_SHARED);
        status.enable();
        status.enable_station_uplink();

        block_on(async {
            join(status.wait_until_station_uplink_disabled(), async {
                status.disable_station_uplink()
            })
            .await;
            join(status.wait_until_station_uplink_enabled(), async {
                status.enable_station_uplink()
            })
            .await;
        });

        assert!(status.is_enabled());
    }

    #[test]
    fn discovery_participation_tracks_every_interface_transition(
    ) -> Result<(), UdpServiceDiscoveryConstructionError> {
        let status = AutoWifiStatus::new(&DISCOVERY_SHARED);
        status.enable();
        let mut participation = status.discovery_participation_receiver()?;

        assert_eq!(
            participation.try_get(),
            Some(EmbeddedDiscoveryParticipation::Central)
        );
        status.disable();
        assert_eq!(
            participation.try_changed(),
            Some(EmbeddedDiscoveryParticipation::Inactive)
        );
        status.toggle_enabled();
        assert_eq!(
            participation.try_changed(),
            Some(EmbeddedDiscoveryParticipation::Central)
        );
        Ok(())
    }

    #[test]
    fn service_discovery_instances_are_bounded_by_capacity(
    ) -> Result<(), UdpServiceDiscoveryConstructionError> {
        static BOUNDED_SHARED: AutoWifiShared<1> = AutoWifiShared::new(InterfaceId::new([0x60; 8]));
        let status = AutoWifiStatus::new(&BOUNDED_SHARED);
        let first = status.discovery_participation_receiver()?;
        assert_eq!(
            status.discovery_participation_receiver().err(),
            Some(UdpServiceDiscoveryConstructionError::DiscoveryCapacityExhausted)
        );

        drop(first);
        assert!(status.discovery_participation_receiver().is_ok());
        Ok(())
    }

    #[test]
    fn aggregate_status_keeps_network_health_separate_from_peer_count() {
        let status = AutoWifiStatus::new(&LIFECYCLE_SHARED);

        assert_eq!(status.connection(), ConnectionState::Initializing);
        status.set_lifecycle(ConnectionState::Disconnected);
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert_eq!(status.peer_count(), 0);
        status.member(0).assign(id(1));
        status.republish_peer_count();
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert_eq!(status.peer_count(), 1);
        status.set_lifecycle(ConnectionState::Connected);
        assert_eq!(status.connection(), ConnectionState::Connected);
        status.set_lifecycle(ConnectionState::Degraded);
        assert_eq!(status.connection(), ConnectionState::Degraded);
        status.disable();
        assert_eq!(status.connection(), ConnectionState::Disabled);
        status.enable();
        assert_eq!(status.connection(), ConnectionState::Degraded);
        status.set_lifecycle(ConnectionState::Failed);
        assert_eq!(status.connection(), ConnectionState::Failed);
    }

    #[test]
    fn retired_and_reconnected_member_traffic_is_monotonic() {
        let status = AutoWifiStatus::new(&ACCOUNTING_SHARED);
        status.set_lifecycle(ConnectionState::Connected);
        status.member(0).assign(id(1));
        status.member(0).add_rx(90);
        status.member(0).add_tx(45);

        assert_eq!((status.rx_bytes(), status.tx_bytes()), (90, 45));
        assert_eq!(
            (status.member(0).rx_bytes(), status.member(0).tx_bytes()),
            (90, 45)
        );
        status.retire_member(0);
        status.republish_peer_count();
        assert_eq!(status.connection(), ConnectionState::Connected);
        assert_eq!(status.peer_count(), 0);
        assert_eq!((status.rx_bytes(), status.tx_bytes()), (90, 45));

        status.member(0).assign(id(2));
        status.republish_peer_count();
        status.member(0).add_rx(30);
        status.member(0).add_tx(15);
        assert_eq!(status.connection(), ConnectionState::Connected);
        assert_eq!(status.peer_count(), 1);
        assert_eq!((status.rx_bytes(), status.tx_bytes()), (120, 60));
        assert_eq!(
            (status.member(0).rx_bytes(), status.member(0).tx_bytes()),
            (30, 15)
        );
    }

    #[test]
    fn reciprocal_peering_is_one_shot_and_uses_the_local_token() {
        let peer = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 2);
        let local_token = [0x5a; 32];

        assert_eq!(
            peering_token_reply(
                BeaconChannel::Unicast,
                contract::PeerObservation::NewlyDiscovered,
                peer,
                local_token,
            ),
            PeeringTokenReply::Send {
                address: peer,
                peering_token: local_token,
            }
        );
        assert_eq!(
            peering_token_reply(
                BeaconChannel::Unicast,
                contract::PeerObservation::Refreshed,
                peer,
                local_token,
            ),
            PeeringTokenReply::NotRequired
        );
        assert_eq!(
            peering_token_reply(
                BeaconChannel::Multicast,
                contract::PeerObservation::NewlyDiscovered,
                peer,
                local_token,
            ),
            PeeringTokenReply::NotRequired
        );
    }
}
