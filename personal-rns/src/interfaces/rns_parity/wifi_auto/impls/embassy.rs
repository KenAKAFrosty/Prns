//! The embassy WiFi/LAN `AutoInterface`: the no_std twin of the tokio supervisor, driven over
//! embassy-net UDP on a board's WiFi stack (esp-radio on the S3). It speaks the same wire-exact
//! [`core`] protocol brain, beacons its peering token on the IPv6 link-local multicast group,
//! validates inbound beacons, and stands a per-peer member up through the embassy [`Fleet`] it is
//! handed. The supervisor owns no spawn: it drives its own bounded pool of member I/O loops inline
//! (a `select_array` over their outbound lanes), so a peer's wire reuses its slot in place. The
//! board hands it the stack, the two UDP sockets (their `static` buffers are the board's), and its
//! WiFi MAC (the EUI-64 link-local the brain hashes its token over). No alloc.

use ::core::cell::Cell;
use ::core::net::Ipv6Addr;

use embassy_futures::select::{select4, Either4};
use embassy_net::udp::UdpSocket;
use embassy_net::{IpAddress, Stack};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_time::{Duration, Ticker};
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::engine::FanTarget;
use crate::interfaces::rns_parity::wifi_auto::core;
use crate::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus, MacAddress};
use crate::runtime::Fleet;

/// How often the supervisor multicasts its peering token, matching the tokio cadence.
const BEACON_INTERVAL: Duration = Duration::from_millis(1600);
/// Consecutive failed beacons before the card reports its egress down (two intervals, so one
/// transient send error does not flap it).
const EGRESS_DOWN_AFTER: u32 = 2;

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
    up: AtomicBool,
    egress_down: AtomicBool,
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
            up: AtomicBool::new(false),
            egress_down: AtomicBool::new(false),
            peers: AtomicU32::new(0),
            members: [const { WifiMemberStatus::new() }; MEMBERS],
        }
    }
}

/// The WiFi/LAN auto-interface's aggregate live status: one "WiFi" card whose connection is Failed
/// (no stack / egress refused), Disconnected (up, no peers), or Connected (peers), with the
/// per-peer members exposed through [`members`](Self::members) for the face to render beside it.
/// `Copy` — a `&'static` borrow of the board's shared state.
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

    fn mark_up(&self) {
        self.shared.up.store(true, Ordering::Relaxed);
    }

    fn set_egress_down(&self, down: bool) {
        self.shared.egress_down.store(down, Ordering::Relaxed);
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

    /// True once the stack is up but recent beacons could not be sent at all — the "cannot transmit"
    /// canary (an AP dropping multicast, the radio wedged), distinct from a stack that never came up.
    #[must_use]
    pub fn egress_down(&self) -> bool {
        self.shared.up.load(Ordering::Relaxed) && self.shared.egress_down.load(Ordering::Relaxed)
    }
}

impl<const MEMBERS: usize> InterfaceStatus for AutoWifiStatus<MEMBERS> {
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
        }
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
    ) where
        M: RawMutex + 'static,
    {
        // Wait for the link and our IPv6 config before binding — beaconing into a down link is
        // pointless, and a multicast join can fail before the interface is up.
        self.stack.wait_config_up().await;
        if self.discovery.bind(core::DEFAULT_DISCOVERY_PORT).is_err()
            || self.data.bind(core::DEFAULT_DATA_PORT).is_err()
            || self
                .stack
                .join_multicast_group(IpAddress::Ipv6(core::DISCOVERY_GROUP))
                .is_err()
        {
            return;
        }
        self.status.mark_up();

        let mut peers: [Option<Ipv6Addr>; MEMBERS] = [None; MEMBERS];
        let mut ids: [InterfaceId; MEMBERS] = [InterfaceId::new([0u8; 8]); MEMBERS];

        let token = *self.brain.our_peering_token().as_bytes();
        let mut beacon = Ticker::every(BEACON_INTERVAL);
        let mut consecutive_tx_failures: u32 = 0;
        let mut now_ms: u64 = 0;
        let mut discovery_buf = [0u8; 64];
        let mut data_buf = [0u8; core::HARDWARE_MTU];

        loop {
            match select4(
                self.discovery.recv_from(&mut discovery_buf),
                self.data.recv_from(&mut data_buf),
                beacon.next(),
                fleet.next_outbound::<{ core::HARDWARE_MTU }>(),
            )
            .await
            {
                Either4::First(received) => {
                    if let Ok((len, meta)) = received {
                        if let IpAddress::Ipv6(src) = meta.endpoint.addr {
                            ingest_beacon(
                                &mut self.brain,
                                &mut peers,
                                &mut ids,
                                &self.status,
                                &fleet,
                                self.bitrate_bps,
                                src,
                                &discovery_buf[..len],
                                now_ms,
                            );
                        }
                    }
                }
                Either4::Second(received) => {
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
                Either4::Third(()) => {
                    now_ms = now_ms.wrapping_add(BEACON_INTERVAL.as_millis());
                    let sent = self
                        .discovery
                        .send_to(
                            &token,
                            (
                                IpAddress::Ipv6(core::DISCOVERY_GROUP),
                                core::DEFAULT_DISCOVERY_PORT,
                            ),
                        )
                        .await
                        .is_ok();
                    note_beacon(&mut consecutive_tx_failures, &self.status, sent);
                    retire_stale(
                        &mut self.brain,
                        &mut peers,
                        &ids,
                        &self.status,
                        &fleet,
                        now_ms,
                    );
                }
                Either4::Fourth((target, fan, frame)) => {
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
                            if self
                                .data
                                .send_to(&frame, (IpAddress::Ipv6(peer), core::DEFAULT_DATA_PORT))
                                .await
                                .is_ok()
                            {
                                self.status.member(slot).add_tx(frame.len() as u64);
                            }
                        }
                    }
                }
            }
        }
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
    status: &AutoWifiStatus<MEMBERS>,
    fleet: &Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    bitrate_bps: u32,
    src: Ipv6Addr,
    bytes: &[u8],
    now_ms: u64,
) {
    let core::BeaconVerdict::Peer(addr) = brain.ingest_discovery_datagram(src, bytes, now_ms)
    else {
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

fn note_beacon<const MEMBERS: usize>(
    consecutive_tx_failures: &mut u32,
    status: &AutoWifiStatus<MEMBERS>,
    sent: bool,
) {
    if sent {
        *consecutive_tx_failures = 0;
        status.set_egress_down(false);
    } else {
        *consecutive_tx_failures = consecutive_tx_failures.saturating_add(1);
        if *consecutive_tx_failures >= EGRESS_DOWN_AFTER {
            status.set_egress_down(true);
        }
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
        status.member(slot).retire();
    }
    status.republish_peer_count();
}
