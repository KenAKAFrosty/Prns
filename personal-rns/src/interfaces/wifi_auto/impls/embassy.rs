//! The embassy WiFi/LAN `AutoInterface`: the no_std twin of the tokio supervisor, driven over
//! embassy-net UDP on a board's WiFi stack (esp-radio on the S3). It speaks the same wire-exact
//! [`core`] protocol brain, beacons its peering token on the IPv6 link-local multicast group,
//! validates inbound beacons, and stands a per-peer member up through the embassy [`Fleet`] it is
//! handed. The supervisor owns no spawn: it drives its own bounded pool of member I/O loops inline
//! (a `select_array` over their outbound lanes), so a peer's wire reuses its slot in place. The
//! board hands it the stack, the two UDP sockets (their `static` buffers are the board's), the two
//! MTU receive buffers, and its WiFi MAC (the EUI-64 link-local the brain hashes its token over). No alloc.

use ::core::cell::Cell;
use ::core::net::Ipv6Addr;

use embassy_futures::select::{select, select4, Either, Either4};
use embassy_net::udp::{RecvError, UdpMetadata, UdpSocket};
use embassy_net::{IpAddress, Stack};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_time::{with_timeout, Duration, Ticker, Timer};
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::engine::FanTarget;
use crate::interfaces::wifi_auto::core;
use crate::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus, MacAddress};
use crate::runtime::EmbassyFleet as Fleet;

/// How often the supervisor multicasts its peering token, matching the tokio cadence.
const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
/// Hard ceiling on any single UDP `send_to` in the run loop. A multicast/unicast send normally
/// completes in microseconds, but under WiFi+BLE coex the radio can stall the WiFi netif's TX so the
/// socket buffer fills and `send_to` blocks indefinitely — which would freeze the whole supervisor
/// (no beacons, no RX) the moment a BLE link goes active. UDP discovery/data is lossy by design, so a
/// send that can't complete in this window is dropped and the loop carries on rather than wedging.
const SEND_TIMEOUT: Duration = Duration::from_millis(300);

/// One confirmed peer's live status on the no_std host: a settable id (the slot reused for
/// successive peers carries each peer's medium-derived id, so the card shows the *peer*, not the
/// slot), plus connection and byte counters as lock-free atomics. The id rides a critical-section
/// cell so a cross-core reader (the render task) sees it coherently; the counters are true `u64`.
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

/// The supervisor's shared live state, parked by the board in a `static` so the render task reads it
/// lock-free across cores: the aggregate counters and a fixed `WifiMemberStatus` per member slot.
pub struct AutoWifiShared<const MEMBERS: usize> {
    id: InterfaceId,
    enabled: AtomicBool,
    peers: AtomicU32,
    members: [WifiMemberStatus; MEMBERS],
}

impl<const MEMBERS: usize> AutoWifiShared<MEMBERS> {
    /// The supervisor's shared state, every slot free — `const`, so it lives in a `static`. `id` is
    /// the aggregate WiFi card's id; the board passes the same id the supervisor mints from
    /// [`core::GROUP_ID`].
    #[must_use]
    pub const fn new(id: InterfaceId) -> Self {
        Self {
            id,
            enabled: AtomicBool::new(true),
            peers: AtomicU32::new(0),
            members: [const { WifiMemberStatus::new() }; MEMBERS],
        }
    }
}

/// The WiFi/LAN auto-interface's aggregate live status: one "WiFi" card whose connection is
/// Disconnected (no peers) or Connected (peers), with the per-peer members exposed through
/// [`members`](Self::members) for the face to render beside it. `Copy` — a `&'static` borrow of the
/// board's shared state.
#[derive(Clone, Copy)]
pub struct AutoWifiStatus<const MEMBERS: usize> {
    shared: &'static AutoWifiShared<MEMBERS>,
}

impl<const MEMBERS: usize> AutoWifiStatus<MEMBERS> {
    /// Wrap the board's shared state. The supervisor and the render task each hold one of these.
    #[must_use]
    pub fn new(shared: &'static AutoWifiShared<MEMBERS>) -> Self {
        Self { shared }
    }

    /// Turn WiFi-auto discovery and member forwarding off or back on from the application.
    pub fn set_enabled(&self, enabled: bool) {
        self.shared.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether WiFi-auto should be discovering and carrying peers.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
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

    /// Each confirmed peer's own live status, for rendering the members as ordinary interface cards.
    /// Only the occupied slots, so the face gets one card per real peer.
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

/// The RNS WiFi/LAN `AutoInterface` as an embassy supervisor: the board hands it the WiFi `stack`,
/// two UDP sockets (their `static` buffers the board's), the board's WiFi `mac` (the EUI-64
/// link-local the brain hashes over, which the board's IPv6 config must match), and the shared
/// status. Driven by `node.run(join(.., wifi.run(fleet)))`.
pub struct AutoWifi<'a, const MEMBERS: usize> {
    stack: Stack<'a>,
    discovery: UdpSocket<'a>,
    data: UdpSocket<'a>,
    brain: core::FixedAutoInterfaceProtocol<MEMBERS>,
    status: AutoWifiStatus<MEMBERS>,
    bitrate_bps: u32,
    /// An optional second netif folded into the SAME umbrella (one card, one fleet): the SoftAP
    /// segment. When present, the supervisor beacons + listens on both stacks, and each peer is
    /// served back over the socket of the segment it was discovered on.
    secondary_stack: Option<Stack<'a>>,
    secondary_discovery: Option<UdpSocket<'a>>,
    secondary_data: Option<UdpSocket<'a>>,
    /// The peering token for the SECONDARY segment, hashed over ITS own link-local — not the primary's.
    /// The token is bound to the source address ([`core::peering_token`]); a receiver validates it
    /// against the address the beacon arrived from. The two segments have different link-locals, so
    /// reusing the primary token on the secondary makes every secondary beacon fail validation (a
    /// station-to-station peer never forms). Computed from the secondary's MAC in `with_secondary_netif`.
    secondary_token: Option<[u8; 32]>,
}

impl<'a, const MEMBERS: usize> AutoWifi<'a, MEMBERS> {
    /// The aggregate id every supervisor mints from [`core::GROUP_ID`] — the board passes the same
    /// id to [`AutoWifiShared::new`] so the card and the supervisor agree.
    #[must_use]
    pub fn aggregate_id() -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, core::GROUP_ID)
    }

    /// Build the supervisor over the board's stack, sockets, and WiFi MAC, reporting into `shared`.
    /// Clamps the wifi LAN pipe to the board's 2.4 GHz radio ceiling for the members' announce pacing.
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
            brain: core::FixedAutoInterfaceProtocol::new(MacAddress::new(mac)),
            status: AutoWifiStatus::new(shared),
            bitrate_bps: core::WIFI_LAN_BITRATE_BPS.min(core::WIFI_EMBEDDED_BITRATE_CEILING_BPS),
            secondary_stack: None,
            secondary_discovery: None,
            secondary_data: None,
            secondary_token: None,
        }
    }

    /// Fold a second netif (its stack + the two UDP sockets bound on it) into this same supervisor,
    /// so the SoftAP segment rides the one WiFi-auto umbrella rather than a separate fleet/card. The
    /// supervisor beacons + listens on both segments and serves each peer back over its own segment.
    #[must_use]
    pub fn with_secondary_netif(
        mut self,
        stack: Stack<'a>,
        discovery: UdpSocket<'a>,
        data: UdpSocket<'a>,
        mac: [u8; 6],
    ) -> Self {
        let link_local = core::link_local_from_mac(MacAddress::new(mac));
        self.secondary_stack = Some(stack);
        self.secondary_discovery = Some(discovery);
        self.secondary_data = Some(data);
        self.secondary_token = Some(*core::peering_token(&link_local).as_bytes());
        self
    }

    /// A copy of this supervisor's aggregate status handle, for the render task. Call before
    /// [`run`](Self::run) consumes the supervisor.
    #[must_use]
    pub fn status(&self) -> AutoWifiStatus<MEMBERS> {
        self.status
    }

    /// Drive the auto-interface forever: bind the sockets, join the multicast group, then race
    /// discovery, inbound data (demuxed to each member by source), the beacon timer, and every
    /// member's outbound lane. A confirmed beacon stands a peer up on a free slot through `fleet`; a
    /// stale peer is torn back down. `M`/`SLOT`/`NOTIFY`/`LIFECYCLE` come from the node's pool.
    ///
    /// The `IpAddress::Ipv6` matches are irrefutable under a `proto-ipv6`-only stack (the board's
    /// config) yet stay refutable in general, so a future dual-stack build still type-checks. The
    /// fleet is built with exactly `MEMBERS` wires, so taking slot `0..MEMBERS` once each never
    /// misses.
    #[allow(irrefutable_let_patterns, clippy::expect_used)]
    pub async fn run<M, const SLOT: usize, const NOTIFY: usize, const LIFECYCLE: usize>(
        mut self,
        mut fleet: Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
        data_buf: &mut [u8],
        sec_data_buf: &mut [u8],
    ) where
        M: RawMutex + 'static,
    {
        // Wait for the link and our IPv6 config before binding — beaconing into a down link is
        // pointless, and a multicast join can fail before the interface is up.
        self.stack.wait_config_up().await;
        let primary_ok = self.discovery.bind(core::DEFAULT_DISCOVERY_PORT).is_ok()
            && self.data.bind(core::DEFAULT_DATA_PORT).is_ok()
            && self
                .stack
                .join_multicast_group(IpAddress::Ipv6(core::DISCOVERY_GROUP))
                .is_ok();
        log::info!(
            "wifi-auto: primary segment {}",
            if primary_ok { "up" } else { "down" }
        );
        if !primary_ok {
            return;
        }

        // Fold the SoftAP segment into the same umbrella: bind + join the group on its stack too. If
        // it can't come up, drop it and run the primary segment alone — never wedge the station.
        let secondary_ok = if let (Some(stack), Some(discovery), Some(data)) = (
            self.secondary_stack.as_ref(),
            self.secondary_discovery.as_mut(),
            self.secondary_data.as_mut(),
        ) {
            // Bounded so a SoftAP netif that never configures can't wedge the station umbrella.
            embassy_time::with_timeout(Duration::from_secs(10), stack.wait_config_up())
                .await
                .is_ok()
                && discovery.bind(core::DEFAULT_DISCOVERY_PORT).is_ok()
                && data.bind(core::DEFAULT_DATA_PORT).is_ok()
                && stack
                    .join_multicast_group(IpAddress::Ipv6(core::DISCOVERY_GROUP))
                    .is_ok()
        } else {
            false
        };
        if !secondary_ok {
            self.secondary_stack = None;
            self.secondary_discovery = None;
            self.secondary_data = None;
        }
        log::info!(
            "wifi-auto: secondary segment {}",
            if secondary_ok { "up" } else { "down" }
        );

        let mut peers: [Option<Ipv6Addr>; MEMBERS] = [None; MEMBERS];
        let mut ids: [InterfaceId; MEMBERS] = [InterfaceId::new([0u8; 8]); MEMBERS];
        let mut peer_on_secondary: [bool; MEMBERS] = [false; MEMBERS];

        let token = *self.brain.our_peering_token().as_bytes();
        // The secondary segment beacons a token over ITS own link-local; falling back to the primary
        // token would bind it to the wrong source address and the peer would reject every beacon.
        let secondary_token = self.secondary_token;
        let mut beacon = Ticker::every(BEACON_INTERVAL);
        let mut now_ms: u64 = 0;
        let mut discovery_buf = [0u8; 64];
        let mut sec_discovery_buf = [0u8; 64];

        loop {
            while !self.status.is_enabled() {
                clear_members(
                    &mut peers,
                    &ids,
                    &mut peer_on_secondary,
                    &self.status,
                    &fleet,
                );
                Timer::after(BEACON_INTERVAL).await;
            }
            match select(
                select4(
                    self.discovery.recv_from(&mut discovery_buf),
                    self.data.recv_from(&mut data_buf[..]),
                    beacon.next(),
                    fleet.next_outbound::<{ core::HARDWARE_MTU }>(),
                ),
                select(
                    recv_or_pending(&self.secondary_discovery, &mut sec_discovery_buf),
                    recv_or_pending(&self.secondary_data, &mut sec_data_buf[..]),
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
                                self.bitrate_bps,
                                src,
                                &discovery_buf[..len],
                                now_ms,
                                false,
                            );
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
                                    IpAddress::Ipv6(core::DISCOVERY_GROUP),
                                    core::DEFAULT_DISCOVERY_PORT,
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
                                    IpAddress::Ipv6(core::DISCOVERY_GROUP),
                                    core::DEFAULT_DISCOVERY_PORT,
                                ),
                            ),
                        )
                        .await
                        {
                            Ok(Ok(())) => secondary_sent = true,
                            Ok(Err(e)) => log::warn!("wifi-auto: sec beacon send err: {e:?}"),
                            Err(_) => log::warn!("wifi-auto: sec beacon send timeout"),
                        }
                    }
                    log::info!(
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
                    );
                }
                Either::First(Either4::Fourth((target, fan, frame))) => {
                    if !frame.is_empty() {
                        for slot in 0..MEMBERS {
                            let Some(peer) = peers[slot] else { continue };
                            let selected = match fan {
                                None => ids[slot] == target,
                                Some(FanTarget::Only(id)) => ids[slot] == id,
                                Some(FanTarget::All) => true,
                                Some(FanTarget::AllExcept(id)) => ids[slot] != id,
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
                                            &frame,
                                            (IpAddress::Ipv6(peer), core::DEFAULT_DATA_PORT),
                                        ),
                                    )
                                    .await,
                                    Ok(Ok(()))
                                ) {
                                    self.status.member(slot).add_tx(frame.len() as u64);
                                }
                            }
                        }
                    }
                }
                Either::Second(Either::First(received)) => {
                    if let Ok((len, meta)) = received {
                        if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                            ingest_beacon(
                                &mut self.brain,
                                &mut peers,
                                &mut ids,
                                &mut peer_on_secondary,
                                &self.status,
                                &fleet,
                                self.bitrate_bps,
                                src,
                                &sec_discovery_buf[..len],
                                now_ms,
                                true,
                            );
                        }
                    }
                }
                Either::Second(Either::Second(received)) => {
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
            }
        }
    }
}

/// Receive on an optional secondary socket, or stay pending forever when there is none — so the run
/// loop's select keeps the same shape whether or not the SoftAP segment is folded in.
async fn recv_or_pending(
    socket: &Option<UdpSocket<'_>>,
    buf: &mut [u8],
) -> Result<(usize, UdpMetadata), RecvError> {
    match socket {
        Some(socket) => socket.recv_from(buf).await,
        None => ::core::future::pending().await,
    }
}

#[allow(clippy::too_many_arguments)]
fn ingest_beacon<
    M: RawMutex + 'static,
    const SLOT: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    brain: &mut core::FixedAutoInterfaceProtocol<MEMBERS>,
    peers: &mut [Option<Ipv6Addr>; MEMBERS],
    ids: &mut [InterfaceId; MEMBERS],
    peer_on_secondary: &mut [bool; MEMBERS],
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    bitrate_bps: u32,
    src: Ipv6Addr,
    bytes: &[u8],
    now_ms: u64,
    on_secondary: bool,
) {
    let verdict = brain.ingest_discovery_datagram(src, bytes, now_ms);
    log::info!(
        "wifi-auto: rx beacon src={src} sec={on_secondary} len={} peer={}",
        bytes.len(),
        matches!(verdict, core::BeaconVerdict::Peer(_))
    );
    let core::BeaconVerdict::Peer(addr) = verdict else {
        return;
    };
    if peers.contains(&Some(addr)) {
        return;
    }
    let Some(slot) = peers.iter().position(Option::is_none) else {
        return;
    };
    let id = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, &addr.octets());
    peers[slot] = Some(addr);
    ids[slot] = id;
    peer_on_secondary[slot] = on_secondary;
    status.member(slot).assign(id);
    status.republish_peer_count();
    fleet.register_member(core::descriptor(id, bitrate_bps));
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
    if fleet.deliver_inbound(ids[slot], bytes) {
        status.member(slot).add_rx(bytes.len() as u64);
    }
}

fn clear_members<
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
        fleet.deregister_member(ids[slot]);
        peers[slot] = None;
        peer_on_secondary[slot] = false;
        status.member(slot).retire();
        changed = true;
    }
    if changed {
        status.republish_peer_count();
    }
}

fn retire_stale<
    M: RawMutex + 'static,
    const SLOT: usize,
    const MEMBERS: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    brain: &mut core::FixedAutoInterfaceProtocol<MEMBERS>,
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
        fleet.deregister_member(ids[slot]);
        peers[slot] = None;
        peer_on_secondary[slot] = false;
        status.member(slot).retire();
    }
    status.republish_peer_count();
}
