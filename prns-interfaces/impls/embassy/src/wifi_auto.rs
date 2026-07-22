use ::core::cell::Cell;
use ::core::net::Ipv6Addr;

use embassy_futures::select::{select, select3, select4, Either, Either3, Either4};
use embassy_net::udp::{RecvError, UdpMetadata, UdpSocket};
use embassy_net::{IpAddress, Stack};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, RawMutex};
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Ticker};
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use prns_core::engine::FanTarget;
use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::{
    BitrateBps, ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus, MacAddress,
};
use prns_runtime::reactor::grant::FrameTarget;
use prns_runtime::runtime::EmbassyFleet as Fleet;

const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
/// WiFi and BLE coexistence can stall a full WiFi TX buffer indefinitely, so lossy UDP sends are bounded rather than freezing beacons and receive handling.
const SEND_TIMEOUT: Duration = Duration::from_millis(300);

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
            ConnectionState::Disabled
        } else if self.shared.peers.load(Ordering::Relaxed) > 0 {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
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
    pub async fn run<M, const SLOT: usize, const NOTIFY: usize, const LIFECYCLE: usize>(
        mut self,
        mut fleet: Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
        data_buf: &mut [u8],
        sec_data_buf: &mut [u8],
    ) where
        M: RawMutex + 'static,
    {
        // Wait for link and IPv6 configuration because a multicast join can fail before the interface is up.
        self.stack.wait_config_up().await;
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
            return;
        }

        // A secondary netif is best-effort so its failure cannot wedge the primary station segment.
        let secondary_ok = if let (Some(stack), Some(discovery), Some(data)) = (
            self.secondary_stack.as_ref(),
            self.secondary_discovery.as_mut(),
            self.secondary_data.as_mut(),
        ) {
            // Bound secondary configuration so a stalled SoftAP cannot wedge the primary segment.
            embassy_time::with_timeout(Duration::from_secs(10), stack.wait_config_up())
                .await
                .is_ok()
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

        let mut peers: [Option<Ipv6Addr>; MEMBERS] = [None; MEMBERS];
        let mut ids: [InterfaceId; MEMBERS] = [InterfaceId::new([0u8; 8]); MEMBERS];
        let mut peer_on_secondary: [bool; MEMBERS] = [false; MEMBERS];

        let token = *self.brain.our_peering_token().as_bytes();
        // Never send the primary token over the secondary link-local address; peers would reject every beacon.
        let secondary_token = self.secondary_token;
        let mut beacon = Ticker::every(BEACON_INTERVAL);
        let mut now_ms: u64 = 0;
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
                                now_ms,
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
                    now_ms = now_ms.wrapping_add(BEACON_INTERVAL.as_millis());
                    let primary_sent = matches!(
                        with_timeout(
                            SEND_TIMEOUT,
                            self.discovery.send_to(
                                &token,
                                (
                                    IpAddress::Ipv6(contract::DISCOVERY_GROUP),
                                    contract::DEFAULT_DISCOVERY_PORT,
                                ),
                            ),
                        )
                        .await,
                        Ok(Ok(()))
                    );
                    let mut secondary_sent = false;
                    if let Some(secondary) = self.secondary_discovery.as_ref() {
                        match with_timeout(
                            SEND_TIMEOUT,
                            secondary.send_to(
                                secondary_token.as_ref().unwrap_or(&token),
                                (
                                    IpAddress::Ipv6(contract::DISCOVERY_GROUP),
                                    contract::DEFAULT_DISCOVERY_PORT,
                                ),
                            ),
                        )
                        .await
                        {
                            Ok(Ok(())) => secondary_sent = true,
                            Ok(Err(e)) => crate::diagnostic_log::debug!(
                                "wifi-auto: sec beacon send err: {e:?}"
                            ),
                            Err(_) => {
                                crate::diagnostic_log::debug!("wifi-auto: sec beacon send timeout")
                            }
                        }
                    }
                    crate::diagnostic_log::debug!(
                        "wifi-auto: tx beacon pri={primary_sent} sec={secondary_sent} has_sec={}",
                        self.secondary_discovery.is_some()
                    );
                    retire_stale(
                        &mut self.brain,
                        &mut peers,
                        &ids,
                        &mut peer_on_secondary,
                        &self.status,
                        &fleet,
                        now_ms,
                    )
                    .await;
                }
                Either::First(Either4::Fourth(outbound)) => {
                    if !outbound.is_empty() {
                        for slot in 0..MEMBERS {
                            let Some(peer) = peers[slot] else { continue };
                            let selected = match outbound.target() {
                                FrameTarget::Direct(id) => ids[slot] == id,
                                FrameTarget::Fan(FanTarget::Only(id)) => ids[slot] == id,
                                FrameTarget::Fan(FanTarget::All) => true,
                                FrameTarget::Fan(FanTarget::AllExcept(id)) => ids[slot] != id,
                            };
                            if !selected {
                                continue;
                            }
                            let socket = if peer_on_secondary[slot] {
                                self.secondary_data.as_ref()
                            } else {
                                Some(&self.data)
                            };
                            if let Some(socket) = socket {
                                if matches!(
                                    with_timeout(
                                        SEND_TIMEOUT,
                                        socket.send_to(
                                            outbound.bytes(),
                                            (IpAddress::Ipv6(peer), contract::DEFAULT_DATA_PORT),
                                        ),
                                    )
                                    .await,
                                    Ok(Ok(()))
                                ) {
                                    self.status.member(slot).add_tx(outbound.len() as u64);
                                }
                            }
                        }
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
                                now_ms,
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
    const SLOT: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    brain: &mut contract::FixedAutoInterfaceProtocol<MEMBERS>,
    peers: &mut [Option<Ipv6Addr>; MEMBERS],
    ids: &mut [InterfaceId; MEMBERS],
    peer_on_secondary: &mut [bool; MEMBERS],
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    bitrate: BitrateBps,
    src: Ipv6Addr,
    bytes: &[u8],
    now_ms: u64,
    on_secondary: bool,
) {
    let verdict = brain.ingest_discovery_datagram(src, bytes, now_ms);
    crate::diagnostic_log::debug!(
        "wifi-auto: rx beacon src={src} sec={on_secondary} len={} peer={}",
        bytes.len(),
        matches!(verdict, contract::BeaconVerdict::Peer(_))
    );
    let contract::BeaconVerdict::Peer(addr) = verdict else {
        return;
    };
    if peers.contains(&Some(addr)) {
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
    const SLOT: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    fleet: &mut Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
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
    const SLOT: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    peers: &mut [Option<Ipv6Addr>; MEMBERS],
    ids: &[InterfaceId; MEMBERS],
    peer_on_secondary: &mut [bool; MEMBERS],
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
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
    const SLOT: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    brain: &mut contract::FixedAutoInterfaceProtocol<MEMBERS>,
    peers: &mut [Option<Ipv6Addr>; MEMBERS],
    ids: &[InterfaceId; MEMBERS],
    peer_on_secondary: &mut [bool; MEMBERS],
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
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
}
