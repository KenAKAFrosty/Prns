mod fanout;

use ::core::cell::Cell;
use ::core::net::Ipv6Addr;

use embassy_futures::join::join;
use embassy_futures::select::{select, select3, select4, Either, Either3, Either4};
use embassy_net::udp::{RecvError, UdpMetadata, UdpSocket};
use embassy_net::{IpAddress, Stack};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, RawMutex};
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Instant, Ticker};
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::{
    BitrateBps, ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus, MacAddress,
};
use prns_runtime::runtime::EmbassyFleet as Fleet;

use fanout::{dispatch_fanout, send_beacon, FanoutPlan, UdpFanoutSender};

const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
const SEND_TIMEOUT: Duration = Duration::from_millis(300);

async fn wait_for_stack<const MEMBERS: usize>(stack: &Stack<'_>, status: AutoWifiStatus<MEMBERS>) {
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
) -> bool {
    loop {
        if !status.is_enabled() {
            status.wait_until_enabled().await;
        }
        match select(
            with_timeout(Duration::from_secs(10), stack.wait_config_up()),
            status.wait_until_disabled(),
        )
        .await
        {
            Either::First(result) => return result.is_ok(),
            Either::Second(()) => {}
        }
    }
}

/// The reusable slot's peer id uses a critical-section cell so cross-core readers see it coherently.
pub struct WifiMemberStatus {
    id: CriticalSectionMutex<Cell<InterfaceId>>,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    active: AtomicBool,
}

impl WifiMemberStatus {
    const fn new() -> Self {
        Self {
            id: CriticalSectionMutex::new(Cell::new(InterfaceId::new([0u8; 8]))),
            connection: AtomicU8::new(ConnectionState::Disconnected.as_u8()),
            rx: AtomicU64::new(0),
            tx: AtomicU64::new(0),
            active: AtomicBool::new(false),
        }
    }

    fn assign(&self, id: InterfaceId) {
        self.id.lock(|cell| cell.set(id));
        self.connection
            .store(ConnectionState::Connected.as_u8(), Ordering::Relaxed);
        self.rx.store(0, Ordering::Relaxed);
        self.tx.store(0, Ordering::Relaxed);
        self.active.store(true, Ordering::Relaxed);
    }

    fn retire(&self) {
        self.connection
            .store(ConnectionState::Disconnected.as_u8(), Ordering::Relaxed);
        self.active.store(false, Ordering::Relaxed);
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
        self.rx.load(Ordering::Relaxed)
    }

    fn tx_bytes(&self) -> u64 {
        self.tx.load(Ordering::Relaxed)
    }
}

pub struct AutoWifiShared<const MEMBERS: usize> {
    id: InterfaceId,
    enabled: AtomicBool,
    enabled_changed: Signal<CriticalSectionRawMutex, bool>,
    radio_enabled_changed: Signal<CriticalSectionRawMutex, bool>,
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
            radio_enabled_changed: Signal::new(),
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
        self.shared.enabled_changed.signal(enabled);
        self.shared.radio_enabled_changed.signal(enabled);
    }

    fn update_enabled(&self, enabled: bool) {
        if self.shared.enabled.swap(enabled, Ordering::Relaxed) != enabled {
            self.shared.enabled_changed.signal(enabled);
            self.shared.radio_enabled_changed.signal(enabled);
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    pub async fn wait_until_enabled(&self) {
        self.wait_for_enabled_state(true, &self.shared.enabled_changed)
            .await;
    }

    pub async fn wait_until_disabled(&self) {
        self.wait_for_enabled_state(false, &self.shared.enabled_changed)
            .await;
    }

    pub async fn wait_until_radio_enabled(&self) {
        self.wait_for_enabled_state(true, &self.shared.radio_enabled_changed)
            .await;
    }

    pub async fn wait_until_radio_disabled(&self) {
        self.wait_for_enabled_state(false, &self.shared.radio_enabled_changed)
            .await;
    }

    async fn wait_for_enabled_state(
        &self,
        enabled: bool,
        changed: &Signal<CriticalSectionRawMutex, bool>,
    ) {
        loop {
            if self.is_enabled() == enabled {
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
}

impl<const MEMBERS: usize> InterfaceStatus for AutoWifiStatus<MEMBERS> {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            return ConnectionState::Disabled;
        }
        let lifecycle = ConnectionState::from_u8(self.shared.lifecycle.load(Ordering::Relaxed));
        match lifecycle {
            ConnectionState::Initializing
            | ConnectionState::Degraded
            | ConnectionState::Reconnecting
            | ConnectionState::Failed
            | ConnectionState::Unknown => lifecycle,
            ConnectionState::Connected
            | ConnectionState::Disconnected
            | ConnectionState::Disabled => {
                if self.shared.peers.load(Ordering::Relaxed) > 0 {
                    ConnectionState::Connected
                } else {
                    ConnectionState::Disconnected
                }
            }
        }
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

pub struct AutoWifi<'a, const MEMBERS: usize> {
    stack: Stack<'a>,
    discovery: UdpSocket<'a>,
    data: UdpSocket<'a>,
    brain: contract::FixedAutoInterfaceProtocol<MEMBERS>,
    status: AutoWifiStatus<MEMBERS>,
    bitrate: BitrateBps,
    secondary_stack: Option<Stack<'a>>,
    secondary_discovery: Option<UdpSocket<'a>>,
    secondary_data: Option<UdpSocket<'a>>,
    /// Must be derived from the secondary link-local address because peers validate the token against the beacon source.
    secondary_token: Option<[u8; 32]>,
}

impl<'a, const MEMBERS: usize> AutoWifi<'a, MEMBERS> {
    #[must_use]
    pub fn aggregate_id() -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, contract::GROUP_ID)
    }

    #[must_use]
    pub fn new(
        stack: Stack<'a>,
        discovery: UdpSocket<'a>,
        data: UdpSocket<'a>,
        mac: [u8; 6],
        shared: &'static AutoWifiShared<MEMBERS>,
    ) -> Self {
        Self {
            stack,
            discovery,
            data,
            brain: contract::FixedAutoInterfaceProtocol::new(MacAddress::new(mac)),
            status: AutoWifiStatus::new(shared),
            bitrate: contract::WIFI_LAN_BITRATE_BPS
                .min(contract::WIFI_EMBEDDED_BITRATE_CEILING_BPS),
            secondary_stack: None,
            secondary_discovery: None,
            secondary_data: None,
            secondary_token: None,
        }
    }

    #[must_use]
    pub fn with_secondary_netif(
        mut self,
        stack: Stack<'a>,
        discovery: UdpSocket<'a>,
        data: UdpSocket<'a>,
        mac: [u8; 6],
    ) -> Self {
        let link_local = contract::link_local_from_mac(MacAddress::new(mac));
        self.secondary_stack = Some(stack);
        self.secondary_discovery = Some(discovery);
        self.secondary_data = Some(data);
        self.secondary_token = Some(*contract::peering_token(&link_local).as_bytes());
        self
    }

    #[must_use]
    pub fn status(&self) -> AutoWifiStatus<MEMBERS> {
        self.status
    }

    #[allow(irrefutable_let_patterns, clippy::expect_used)]
    pub async fn run<M, const FRAME: usize, const NOTIFY: usize, const LIFECYCLE: usize>(
        mut self,
        mut fleet: Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
        data_buf: &mut [u8],
        sec_data_buf: &mut [u8],
    ) where
        M: RawMutex + 'static,
    {
        wait_for_stack(&self.stack, self.status).await;
        let primary_ok = self
            .discovery
            .bind(contract::DEFAULT_DISCOVERY_PORT)
            .is_ok()
            && self.data.bind(contract::DEFAULT_DATA_PORT).is_ok()
            && self
                .stack
                .join_multicast_group(IpAddress::Ipv6(contract::DISCOVERY_GROUP))
                .is_ok();
        crate::diagnostic_log::debug!(
            "wifi-auto: primary segment {}",
            if primary_ok { "up" } else { "down" }
        );
        if !primary_ok {
            self.status.set_lifecycle(ConnectionState::Failed);
            return;
        }

        let secondary_configured = self.secondary_stack.is_some();
        let secondary_ok = if let (Some(stack), Some(discovery), Some(data)) = (
            self.secondary_stack.as_ref(),
            self.secondary_discovery.as_mut(),
            self.secondary_data.as_mut(),
        ) {
            wait_for_secondary_stack(stack, self.status).await
                && discovery.bind(contract::DEFAULT_DISCOVERY_PORT).is_ok()
                && data.bind(contract::DEFAULT_DATA_PORT).is_ok()
                && stack
                    .join_multicast_group(IpAddress::Ipv6(contract::DISCOVERY_GROUP))
                    .is_ok()
        } else {
            false
        };
        if !secondary_ok {
            self.secondary_stack = None;
            self.secondary_discovery = None;
            self.secondary_data = None;
        }
        crate::diagnostic_log::debug!(
            "wifi-auto: secondary segment {}",
            if secondary_ok { "up" } else { "down" }
        );
        self.status
            .set_lifecycle(if secondary_configured && !secondary_ok {
                ConnectionState::Degraded
            } else {
                ConnectionState::Disconnected
            });

        let mut peers: [Option<Ipv6Addr>; MEMBERS] = [None; MEMBERS];
        let mut ids: [InterfaceId; MEMBERS] = [InterfaceId::new([0u8; 8]); MEMBERS];
        let mut peer_on_secondary: [bool; MEMBERS] = [false; MEMBERS];

        let token = *self.brain.our_peering_token().as_bytes();
        let secondary_token = self.secondary_token;
        let mut beacon = Ticker::every(BEACON_INTERVAL);
        let mut fanout_start = 0;
        let mut discovery_buf = [0u8; 64];
        let mut sec_discovery_buf = [0u8; 64];

        loop {
            if !self.status.is_enabled() {
                clear_members(
                    &mut peers,
                    &ids,
                    &mut peer_on_secondary,
                    &self.status,
                    &fleet,
                )
                .await;
                self.status.wait_until_enabled().await;
            }
            match select(
                select4(
                    self.discovery.recv_from(&mut discovery_buf),
                    self.data.recv_from(&mut data_buf[..]),
                    beacon.next(),
                    fleet.next_outbound(),
                ),
                select3(
                    recv_or_pending(&self.secondary_discovery, &mut sec_discovery_buf),
                    recv_or_pending(&self.secondary_data, &mut sec_data_buf[..]),
                    self.status.wait_until_disabled(),
                ),
            )
            .await
            {
                Either::First(Either4::First(received)) => {
                    if let Ok((len, meta)) = received {
                        if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                            ingest_beacon(
                                &mut self.brain,
                                &mut peers,
                                &mut ids,
                                &mut peer_on_secondary,
                                &self.status,
                                &fleet,
                                self.bitrate,
                                src,
                                &discovery_buf[..len],
                                Instant::now().as_millis(),
                                false,
                            )
                            .await;
                        }
                    }
                }
                Either::First(Either4::Second(received)) => {
                    if let Ok((len, meta)) = received {
                        if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                            route_inbound(
                                &mut fleet,
                                &peers,
                                &ids,
                                &self.status,
                                src,
                                &data_buf[..len],
                            );
                        }
                    }
                }
                Either::First(Either4::Third(())) => {
                    let sends = with_timeout(
                        SEND_TIMEOUT,
                        join(
                            send_beacon(Some(&self.discovery), Some(&token)),
                            send_beacon(
                                self.secondary_discovery.as_ref(),
                                secondary_token.as_ref(),
                            ),
                        ),
                    );
                    if matches!(
                        select(self.status.wait_until_disabled(), sends).await,
                        Either::First(())
                    ) {
                        continue;
                    }
                    retire_stale(
                        &mut self.brain,
                        &mut peers,
                        &ids,
                        &mut peer_on_secondary,
                        &self.status,
                        &fleet,
                        Instant::now().as_millis(),
                    )
                    .await;
                }
                Either::First(Either4::Fourth(outbound)) => {
                    if !outbound.is_empty() {
                        let mut plan =
                            FanoutPlan::new(outbound.target(), &peers, &ids, fanout_start);
                        if MEMBERS > 0 {
                            fanout_start = (fanout_start + 1) % MEMBERS;
                        }
                        let mut sender = UdpFanoutSender {
                            primary: &self.data,
                            secondary: self.secondary_data.as_ref(),
                            peers: &peers,
                            peer_on_secondary: &peer_on_secondary,
                            status: self.status,
                            bytes: outbound.bytes(),
                        };
                        let dispatch = dispatch_fanout(&mut plan, &mut sender, SEND_TIMEOUT);
                        let _ = select(self.status.wait_until_disabled(), dispatch).await;
                    }
                }
                Either::Second(Either3::First(received)) => {
                    if let Ok((len, meta)) = received {
                        if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                            ingest_beacon(
                                &mut self.brain,
                                &mut peers,
                                &mut ids,
                                &mut peer_on_secondary,
                                &self.status,
                                &fleet,
                                self.bitrate,
                                src,
                                &sec_discovery_buf[..len],
                                Instant::now().as_millis(),
                                true,
                            )
                            .await;
                        }
                    }
                }
                Either::Second(Either3::Second(received)) => {
                    if let Ok((len, meta)) = received {
                        if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                            route_inbound(
                                &mut fleet,
                                &peers,
                                &ids,
                                &self.status,
                                src,
                                &sec_data_buf[..len],
                            );
                        }
                    }
                }
                Either::Second(Either3::Third(())) => {}
            }
        }
    }
}

async fn recv_or_pending(
    socket: &Option<UdpSocket<'_>>,
    buf: &mut [u8],
) -> Result<(usize, UdpMetadata), RecvError> {
    match socket {
        Some(socket) => socket.recv_from(buf).await,
        None => ::core::future::pending().await,
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
) {
    let verdict = brain.ingest_discovery_datagram(src, bytes, now_ms);
    let contract::BeaconVerdict::Peer(addr) = verdict else {
        return;
    };
    if let Some(slot) = peers.iter().position(|peer| *peer == Some(addr)) {
        peer_on_secondary[slot] = on_secondary;
        return;
    }
    let Some(slot) = peers.iter().position(Option::is_none) else {
        return;
    };
    let id = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, &addr.octets());
    fleet
        .register_member(contract::descriptor(
            id,
            contract::policy_for_bitrate(bitrate),
        ))
        .await;
    peers[slot] = Some(addr);
    ids[slot] = id;
    peer_on_secondary[slot] = on_secondary;
    status.member(slot).assign(id);
    status.republish_peer_count();
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
        status.member(slot).retire();
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
        status.member(slot).retire();
    }
    status.republish_peer_count();
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::{block_on, join::join3};

    static SHARED: AutoWifiShared<1> = AutoWifiShared::new(InterfaceId::new([0x5B; 8]));
    static LIFECYCLE_SHARED: AutoWifiShared<1> = AutoWifiShared::new(InterfaceId::new([0x5C; 8]));

    fn id(suffix: u8) -> InterfaceId {
        InterfaceId::new([InterfaceKind::WifiPeer as u8, 0, 0, 0, 0, 0, 0, suffix])
    }

    #[test]
    fn enabled_state_changes_wake_all_waiters() {
        let status = AutoWifiStatus::new(&SHARED);

        block_on(async {
            join3(
                status.wait_until_disabled(),
                status.wait_until_radio_disabled(),
                async { status.disable() },
            )
            .await;
            join3(
                status.wait_until_enabled(),
                status.wait_until_radio_enabled(),
                async { status.enable() },
            )
            .await;
        });
    }

    #[test]
    fn aggregate_status_preserves_initialization_degradation_and_failure() {
        let status = AutoWifiStatus::new(&LIFECYCLE_SHARED);

        assert_eq!(status.connection(), ConnectionState::Initializing);
        status.set_lifecycle(ConnectionState::Disconnected);
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        status.member(0).assign(id(1));
        status.republish_peer_count();
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
}
