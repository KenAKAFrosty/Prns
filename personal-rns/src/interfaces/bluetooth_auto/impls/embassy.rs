//! The embassy Bluetooth supervisor: the no_std twin of the tokio [`BluetoothAuto`], driving a
//! board's native BLE backend through the same wire-exact [`core`] handshake brain. The board hands
//! it a [`BleBackend`] (the embedded radio bridge), this node's identity/endpoint/capabilities, and
//! the shared status; it advertises, accepts a central, settles the link over the engine's own
//! [`Handshake`], and stands the peer up as an engine interface through the embassy [`Fleet`]. The
//! supervisor owns no spawn: it drives the accept→handshake and the member's frame pump inline in
//! one `select` loop. A single concurrent peer (the embedded radio carries one connection), so the
//! settled member is held inline rather than in a slot array; the status keeps a slot array only so
//! the face renders the peer beside the aggregate, exactly as the WiFi supervisor does. No alloc.

use ::core::cell::Cell;

use embassy_futures::select::{select3, Either3};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_time::{with_timeout, Duration};
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::engine::FanTarget;
use crate::interfaces::bluetooth_auto::core::{
    self, BleAddress, BleIdentity, Endpoint, Established, Handshake, HandshakeRole,
    LinkCapabilities, Local, Outcome,
};
use crate::interfaces::bluetooth_auto::seam::{BleBackend, BleEvent, BleLink, BleSink, BleSource};
use crate::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus};
use crate::runtime::Fleet;

/// A central that connects but never finishes the handshake must not wedge the accept loop, so the
/// settle is bounded — matching the tokio supervisor's window.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// One confirmed peer's live status on the no_std host: a settable id (the slot reused for the next
/// peer carries each peer's identity-derived id, so the card shows the *peer*), plus connection and
/// byte counters as lock-free atomics. The id rides a critical-section cell so a render task on
/// another core sees it coherently; the counters are true `u64`.
pub struct BluetoothMemberStatus {
    id: CriticalSectionMutex<Cell<InterfaceId>>,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    active: AtomicBool,
}

impl BluetoothMemberStatus {
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

impl InterfaceStatus for BluetoothMemberStatus {
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
/// lock-free across cores: the aggregate up/peer flags and a fixed `BluetoothMemberStatus` per slot.
pub struct BluetoothAutoShared<const MEMBERS: usize> {
    id: InterfaceId,
    up: AtomicBool,
    peers: AtomicU32,
    members: [BluetoothMemberStatus; MEMBERS],
}

impl<const MEMBERS: usize> BluetoothAutoShared<MEMBERS> {
    /// The supervisor's shared state, every slot free — `const`, so it lives in a `static`. `id` is
    /// the aggregate BLE card's id; the board passes the same id it keys the fleet lane with, so the
    /// card and the supervisor agree.
    #[must_use]
    pub const fn new(id: InterfaceId) -> Self {
        Self {
            id,
            up: AtomicBool::new(false),
            peers: AtomicU32::new(0),
            members: [const { BluetoothMemberStatus::new() }; MEMBERS],
        }
    }
}

/// The Bluetooth supervisor's aggregate live status: one "BLE" card whose connection is Initializing
/// (radio not yet up), Disconnected (up, scanning, no peer), or Connected (a peer linked), with the
/// per-peer members exposed through [`members`](Self::members) for the face to render beside it.
/// `Copy` — a `&'static` borrow of the board's shared state.
#[derive(Clone, Copy)]
pub struct BluetoothAutoStatus<const MEMBERS: usize> {
    shared: &'static BluetoothAutoShared<MEMBERS>,
}

impl<const MEMBERS: usize> BluetoothAutoStatus<MEMBERS> {
    /// Wrap the board's shared state. The supervisor and the render task each hold one of these.
    #[must_use]
    pub fn new(shared: &'static BluetoothAutoShared<MEMBERS>) -> Self {
        Self { shared }
    }

    fn mark_up(&self) {
        self.shared.up.store(true, Ordering::Relaxed);
    }

    fn member(&self, slot: usize) -> &'static BluetoothMemberStatus {
        &self.shared.members[slot]
    }

    fn first_free_slot(&self) -> Option<usize> {
        self.shared
            .members
            .iter()
            .position(|member| !member.active.load(Ordering::Relaxed))
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
    pub fn members(&self) -> impl Iterator<Item = &'static BluetoothMemberStatus> {
        self.shared
            .members
            .iter()
            .filter(|member| member.active.load(Ordering::Relaxed))
    }
}

impl<const MEMBERS: usize> InterfaceStatus for BluetoothAutoStatus<MEMBERS> {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.shared.up.load(Ordering::Relaxed) {
            ConnectionState::Initializing
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

/// The one settled peer the supervisor is carrying: its engine id and status slot, the address it
/// reports closed under, and the split data halves it pumps.
struct Active<L: BleLink> {
    id: InterfaceId,
    slot: usize,
    address: BleAddress,
    source: L::Source,
    sink: L::Sink,
}

/// The native-Bluetooth auto-interface as an embassy supervisor: the board hands it the radio
/// `backend`, this node's `Local` (identity/endpoint/capabilities the brain hashes and negotiates),
/// and the shared status. Driven by `node.run(join(.., ble.run(fleet)))`.
pub struct BluetoothAuto<B, const MEMBERS: usize> {
    backend: B,
    local: Local,
    status: BluetoothAutoStatus<MEMBERS>,
    bitrate_bps: u32,
}

impl<B: BleBackend, const MEMBERS: usize> BluetoothAuto<B, MEMBERS> {
    /// Build the supervisor over the board's `backend`, reporting into `shared`. The id the board
    /// keyed `shared` with is the aggregate card's id and the fleet lane's key.
    #[must_use]
    pub fn new(
        backend: B,
        identity: BleIdentity,
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
        shared: &'static BluetoothAutoShared<MEMBERS>,
    ) -> Self {
        Self {
            backend,
            local: Local {
                identity,
                endpoint,
                capabilities,
            },
            status: BluetoothAutoStatus::new(shared),
            bitrate_bps: core::BLE_BITRATE_GUESS_BPS,
        }
    }

    /// A copy of this supervisor's aggregate status handle, for the render task. Call before
    /// [`run`](Self::run) consumes the supervisor.
    #[must_use]
    pub fn status(&self) -> BluetoothAutoStatus<MEMBERS> {
        self.status
    }

    /// Drive the auto-interface forever: bring the radio up advertising, then race a new inbound
    /// link (handshook inline to a settled peer that joins the fleet), the engine's outbound frames
    /// (fanned to the member's sink), and the member's inbound frames (delivered to the fleet). A
    /// source or sink error retires the peer back off the fleet. `M`/`SLOT`/`NOTIFY`/`LIFECYCLE`
    /// come from the node's pool.
    pub async fn run<M, const SLOT: usize, const NOTIFY: usize, const LIFECYCLE: usize>(
        self,
        mut fleet: Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    ) where
        M: RawMutex + 'static,
    {
        let Self {
            mut backend,
            local,
            status,
            bitrate_bps,
        } = self;
        let _ = backend.set_advertising(true).await;
        status.mark_up();

        let mut active: Option<Active<B::Link>> = None;
        let mut inbound = [0u8; core::BLE_HW_MTU];

        loop {
            let outcome = select3(
                backend.next_event(),
                fleet.outbound_ready(),
                recv_active::<B::Link>(&mut active, &mut inbound),
            )
            .await;
            match outcome {
                Either3::First(event) => {
                    let Some(link) = inbound_link(event) else {
                        continue;
                    };
                    if active.is_some() {
                        continue;
                    }
                    let address = link.address();
                    let Some((established, source, sink)) = settle(link, local).await else {
                        backend.on_link_closed(address).await;
                        continue;
                    };
                    let Some(slot) = status.first_free_slot() else {
                        continue;
                    };
                    let id = InterfaceId::from_channel_tag(
                        InterfaceKind::BluetoothPeer,
                        established.identity.as_bytes(),
                    );
                    status.member(slot).assign(id);
                    status.republish_peer_count();
                    let registered = fleet.register_member(core::descriptor(id, bitrate_bps));
                    log::info!("ble supervisor: member registered slot {slot} ok={registered}");
                    active = Some(Active {
                        id,
                        slot,
                        address,
                        source,
                        sink,
                    });
                    let _ = backend.set_advertising(false).await;
                }
                Either3::Second(()) => {
                    while let Some((target, fan, frame)) =
                        fleet.try_next_outbound::<{ core::BLE_HW_MTU }>()
                    {
                        if frame.is_empty() {
                            continue;
                        }
                        let Some(member) = active.as_mut() else {
                            continue;
                        };
                        let selected = match fan {
                            None => member.id == target,
                            Some(FanTarget::Only(id)) => member.id == id,
                            Some(FanTarget::All) => true,
                            Some(FanTarget::AllExcept(id)) => member.id != id,
                        };
                        if !selected {
                            continue;
                        }
                        if member.sink.send_frame(&frame).await.is_ok() {
                            status.member(member.slot).add_tx(frame.len() as u64);
                        } else {
                            close_active(&mut active, &status, &mut fleet, &mut backend).await;
                            break;
                        }
                    }
                }
                Either3::Third(received) => match received {
                    Ok(0) => {}
                    Ok(len) => {
                        if let Some(member) = active.as_ref() {
                            if fleet.deliver_inbound(member.id, &inbound[..len]) {
                                status.member(member.slot).add_rx(len as u64);
                            }
                        }
                    }
                    Err(_) => {
                        close_active(&mut active, &status, &mut fleet, &mut backend).await;
                    }
                },
            }
        }
    }
}

/// The link an event carries, if any — a freshly accepted central or a completed dial. A bare
/// sighting carries none (this supervisor is listener-only; dialing belongs to a later rung).
fn inbound_link<L: BleLink>(event: BleEvent<L>) -> Option<L> {
    match event {
        BleEvent::Inbound(link) | BleEvent::LinkReady { link, .. } => Some(link),
        BleEvent::Sighting { .. } => None,
    }
}

/// Run the handshake to a settled peer and split the link into its data halves, or `None` if it
/// aborts or times out.
async fn settle<L: BleLink>(
    mut link: L,
    local: Local,
) -> Option<(Established, L::Source, L::Sink)> {
    let established = drive_handshake(&mut link, HandshakeRole::Listener, local).await?;
    let (source, sink) = link.into_data();
    Some((established, source, sink))
}

/// Drive the engine's [`Handshake`] over the link's control lane to a settled peer, bounded by
/// [`HANDSHAKE_TIMEOUT`]. The wire-exact twin of the tokio supervisor's `drive_handshake`.
async fn drive_handshake<L: BleLink>(
    link: &mut L,
    role: HandshakeRole,
    local: Local,
) -> Option<Established> {
    with_timeout(HANDSHAKE_TIMEOUT, async {
        let (mut handshake, opening) = Handshake::begin(role, local, None);
        if let Some(msg) = opening {
            link.control_send(&msg).await.ok()?;
        }
        loop {
            let msg = link.control_recv().await.ok()?;
            let reaction = handshake.absorb(msg);
            if let Some(reply) = reaction.reply {
                link.control_send(&reply).await.ok()?;
            }
            match reaction.outcome {
                Outcome::Settled(established) => return Some(established),
                Outcome::Aborted(_) => return None,
                Outcome::Pending => {}
            }
        }
    })
    .await
    .ok()
    .flatten()
}

/// The active member's next inbound frame, or a never-resolving wait when no peer is linked — so the
/// `select` arm is always present yet only fires once a peer exists.
async fn recv_active<L: BleLink>(
    active: &mut Option<Active<L>>,
    buf: &mut [u8],
) -> Result<usize, <L::Source as BleSource>::Error> {
    match active {
        Some(member) => member.source.recv_frame(buf).await,
        None => ::core::future::pending().await,
    }
}

/// Retire the linked peer off the fleet and the status, and tell the backend its link closed.
async fn close_active<
    B: BleBackend,
    const MEMBERS: usize,
    M: RawMutex + 'static,
    const SLOT: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
>(
    active: &mut Option<Active<B::Link>>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    backend: &mut B,
) {
    if let Some(member) = active.take() {
        fleet.deregister_member(member.id);
        status.member(member.slot).retire();
        status.republish_peer_count();
        backend.on_link_closed(member.address).await;
        let _ = backend.set_advertising(true).await;
    }
}
