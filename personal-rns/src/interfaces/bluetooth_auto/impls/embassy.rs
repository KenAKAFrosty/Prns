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

use embassy_futures::select::{select3, select_array, Either3};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_time::{with_timeout, Duration, Instant};
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::engine::FanTarget;
use crate::interfaces::bluetooth_auto::core::{
    self, arrangement, l2cap_plan, BleAddress, BleIdentity, Endpoint, Established, Handshake,
    HandshakeRole, L2capPlan, LinkCapabilities, Local, Outcome,
};
use crate::interfaces::bluetooth_auto::manager::{
    role_for, AdvertisingMode, ConnectionManager, ManagerAction, ManagerInput, ScanningMode,
};
use crate::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};
use crate::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus};
use crate::runtime::Fleet;

/// The dial/suppress backoff table size for the embedded brain — a few addresses mid-dial or cooling
/// off, distinct from settled peers. Tiny, fixed, and independent of the member ceiling.
const DIAL_TRACK: usize = 6;

/// The most actions one `ManagerInput` can emit (evict + admit + advertising + scanning), with a
/// little headroom. The driver drains the queue after every `handle`, so it never accumulates more.
const ACTION_CAP: usize = 6;

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

/// A settled peer the supervisor is carrying: its identity (the brain's key), engine id and status
/// slot, the address it reports closed under, and the split data halves it pumps.
struct Active<L: BleLink> {
    identity: BleIdentity,
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
        let mut manager = ConnectionManager::<MEMBERS, DIAL_TRACK>::new(local);
        let mut members: [Option<Active<B::Link>>; MEMBERS] = [const { None }; MEMBERS];
        let mut inbufs: [[u8; core::BLE_HW_MTU]; MEMBERS] = [[0u8; core::BLE_HW_MTU]; MEMBERS];
        let mut pending: heapless::Vec<ManagerAction, ACTION_CAP> = heapless::Vec::new();
        status.mark_up();
        manager.start(&mut |action| {
            let _ = pending.push(action);
        });
        apply_radio(
            &mut pending,
            &status,
            &mut fleet,
            &mut backend,
            &mut members,
        )
        .await;

        loop {
            let outcome = select3(
                backend.next_event(),
                fleet.outbound_ready(),
                recv_any(&mut members, &mut inbufs),
            )
            .await;
            let now_ms = Instant::now().as_millis();
            match outcome {
                Either3::First(BleEvent::Sighting { address, .. }) => {
                    manager.handle(ManagerInput::Sighting { address, now_ms }, &mut |action| {
                        let _ = pending.push(action);
                    });
                    apply_radio(
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                    )
                    .await;
                }
                Either3::First(BleEvent::Inbound(link)) => {
                    settle_into_fleet(
                        link,
                        Origin::Accepted,
                        local,
                        bitrate_bps,
                        &mut manager,
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                        &mut inbufs,
                    )
                    .await;
                }
                Either3::First(BleEvent::LinkReady { link, origin, .. }) => {
                    settle_into_fleet(
                        link,
                        origin,
                        local,
                        bitrate_bps,
                        &mut manager,
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                        &mut inbufs,
                    )
                    .await;
                }
                Either3::First(BleEvent::DialFailed { address }) => {
                    manager.handle(
                        ManagerInput::DialFailed { address, now_ms },
                        &mut |action| {
                            let _ = pending.push(action);
                        },
                    );
                    apply_radio(
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                    )
                    .await;
                }
                Either3::Second(()) => {
                    drain_outbound(
                        &mut manager,
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                    )
                    .await;
                }
                Either3::Third((index, received)) => {
                    deliver_inbound(
                        index,
                        received.map_err(|_| ()),
                        &mut manager,
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                        &mut inbufs,
                    )
                    .await;
                }
            }
        }
    }
}

/// Run the handshake to a settled peer with the role its origin dictates (a dialed link opens with
/// `Hello`, an accepted one listens and replies `Welcome`) and split the link into its data halves,
/// or `None` if it aborts or times out.
async fn settle<L: BleLink>(
    mut link: L,
    role: HandshakeRole,
    local: Local,
) -> Option<(Established, L::Source, L::Sink)> {
    let established = drive_handshake(&mut link, role, local).await?;
    let lane = l2cap_plan(
        arrangement(local.endpoint, established.endpoint),
        role,
        local.endpoint,
        &local.capabilities,
        &established.capabilities,
    );
    if !matches!(lane, L2capPlan::None) {
        let _ = link.upgrade(&lane).await;
    }
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

/// One settled member's next inbound frame into its own buffer, or a never-resolving wait when the
/// slot is empty — so a fixed `[_; MEMBERS]` of these always has one entry per slot for `select_array`.
async fn recv_or_pending<L: BleLink>(
    member: &mut Option<Active<L>>,
    buf: &mut [u8; core::BLE_HW_MTU],
) -> Result<usize, <L::Source as BleSource>::Error> {
    match member {
        Some(active) => active.source.recv_frame(buf).await,
        None => ::core::future::pending().await,
    }
}

/// Race every settled member's inbound recv, returning the slot index that produced a frame (into its
/// own `inbufs` slot) and the result. A fixed `[_; MEMBERS]` of `recv_or_pending` futures — empty
/// slots park forever — built with disjoint borrows via `from_fn` over a zipped iterator, no `unsafe`.
/// At `MEMBERS = 1` this is a single-future select carrying one buffer: no RAM over the old path.
async fn recv_any<L: BleLink, const MEMBERS: usize>(
    members: &mut [Option<Active<L>>; MEMBERS],
    bufs: &mut [[u8; core::BLE_HW_MTU]; MEMBERS],
) -> (usize, Result<usize, <L::Source as BleSource>::Error>) {
    let mut pairs = members.iter_mut().zip(bufs.iter_mut());
    let futures: [_; MEMBERS] = ::core::array::from_fn(|_| {
        let (member, buf) = pairs.next().expect("one pair per member slot");
        recv_or_pending(member, buf)
    });
    let (result, index) = select_array(futures).await;
    (index, result)
}

/// Apply one brain action that needs no freshly-settled link in hand. `Admit`/`Reject` are handled in
/// [`settle_into_fleet`], where the just-handshook source/sink are still in scope.
async fn apply_one<
    B: BleBackend,
    M: RawMutex + 'static,
    const SLOT: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    action: ManagerAction,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) {
    match action {
        ManagerAction::Dial(address) => backend.dial(address).await,
        ManagerAction::Evict { slot, .. } => {
            if let Some(member) = members[slot].take() {
                fleet.deregister_member(member.id);
                status.member(slot).retire();
                status.republish_peer_count();
            }
        }
        ManagerAction::NotifyClosed(address) => backend.on_link_closed(address).await,
        ManagerAction::SetAdvertising(mode) => {
            let _ = backend
                .set_advertising(matches!(mode, AdvertisingMode::On))
                .await;
        }
        ManagerAction::SetScanning(mode) => {
            let _ = backend.set_scanning(matches!(mode, ScanningMode::On)).await;
        }
        ManagerAction::Admit { .. } | ManagerAction::Reject { .. } => {}
    }
}

/// Drain and apply the queued link-free actions.
async fn apply_radio<
    B: BleBackend,
    M: RawMutex + 'static,
    const SLOT: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    pending: &mut heapless::Vec<ManagerAction, ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) {
    let actions = ::core::mem::take(pending);
    for action in actions {
        apply_one(action, status, fleet, backend, members).await;
    }
}

/// Retire the member in `slot` (its link died or a send failed): tear it off the fleet and status,
/// tell the brain so it frees the slot and re-reconciles the radio, then apply that reconcile.
#[allow(clippy::too_many_arguments)]
async fn close_member<
    B: BleBackend,
    M: RawMutex + 'static,
    const SLOT: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    slot: usize,
    manager: &mut ConnectionManager<MEMBERS, DIAL_TRACK>,
    pending: &mut heapless::Vec<ManagerAction, ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) {
    let Some(member) = members[slot].take() else {
        return;
    };
    fleet.deregister_member(member.id);
    status.member(slot).retire();
    status.republish_peer_count();
    manager.handle(
        ManagerInput::Closed {
            identity: member.identity,
            address: member.address,
        },
        &mut |action| {
            let _ = pending.push(action);
        },
    );
    apply_radio(pending, status, fleet, backend, members).await;
}

#[allow(clippy::too_many_arguments)]
async fn drain_outbound<
    B: BleBackend,
    M: RawMutex + 'static,
    const SLOT: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    manager: &mut ConnectionManager<MEMBERS, DIAL_TRACK>,
    pending: &mut heapless::Vec<ManagerAction, ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) {
    while let Some((target, fan, frame)) = fleet.try_next_outbound::<{ core::BLE_HW_MTU }>() {
        if frame.is_empty() {
            continue;
        }
        for slot in 0..MEMBERS {
            let selected = match members[slot].as_ref() {
                Some(member) => match fan {
                    None => member.id == target,
                    Some(FanTarget::Only(id)) => member.id == id,
                    Some(FanTarget::All) => true,
                    Some(FanTarget::AllExcept(id)) => member.id != id,
                },
                None => false,
            };
            if !selected {
                continue;
            }
            let sent = match members[slot].as_mut() {
                Some(member) => member.sink.send_frame(&frame).await.is_ok(),
                None => false,
            };
            if sent {
                status.member(slot).add_tx(frame.len() as u64);
            } else {
                close_member(slot, manager, pending, status, fleet, backend, members).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn deliver_inbound<
    B: BleBackend,
    M: RawMutex + 'static,
    const SLOT: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    index: usize,
    received: Result<usize, ()>,
    manager: &mut ConnectionManager<MEMBERS, DIAL_TRACK>,
    pending: &mut heapless::Vec<ManagerAction, ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
    inbufs: &mut [[u8; core::BLE_HW_MTU]; MEMBERS],
) {
    match received {
        Ok(0) => {}
        Ok(len) => {
            if let Some(member) = members[index].as_ref() {
                if fleet.deliver_inbound(member.id, &inbufs[index][..len]) {
                    status.member(member.slot).add_rx(len as u64);
                }
            }
        }
        Err(()) => {
            close_member(index, manager, pending, status, fleet, backend, members).await;
        }
    }
}

/// Handshake a fresh link (dialed or accepted) and let the brain resolve it: `Admit` stands the peer
/// up as a fleet member in its slot, `Reject` drops it, `Evict` retires the incumbent it beat; the
/// radio actions route through [`apply_one`]. The just-handshook source/sink are held until `Admit`
/// claims them.
#[allow(clippy::too_many_arguments)]
async fn settle_into_fleet<
    B: BleBackend,
    M: RawMutex + 'static,
    const SLOT: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    link: B::Link,
    origin: Origin,
    local: Local,
    bitrate_bps: u32,
    manager: &mut ConnectionManager<MEMBERS, DIAL_TRACK>,
    pending: &mut heapless::Vec<ManagerAction, ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, SLOT, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
    inbufs: &mut [[u8; core::BLE_HW_MTU]; MEMBERS],
) {
    let address = link.address();
    let role = role_for(origin);
    let mut handshake = ::core::pin::pin!(settle(link, role, local));
    let settled = loop {
        match select3(
            handshake.as_mut(),
            fleet.outbound_ready(),
            recv_any(members, inbufs),
        )
        .await
        {
            Either3::First(result) => break result,
            Either3::Second(()) => {
                drain_outbound(manager, pending, status, fleet, backend, members).await;
            }
            Either3::Third((index, received)) => {
                deliver_inbound(
                    index,
                    received.map_err(|_| ()),
                    manager,
                    pending,
                    status,
                    fleet,
                    backend,
                    members,
                    inbufs,
                )
                .await;
            }
        }
    };
    let now_ms = Instant::now().as_millis();
    let Some((established, source, sink)) = settled else {
        manager.handle(
            ManagerInput::HandshakeFailed { address, origin },
            &mut |action| {
                let _ = pending.push(action);
            },
        );
        apply_radio(pending, status, fleet, backend, members).await;
        return;
    };
    manager.handle(
        ManagerInput::Settled {
            address,
            origin,
            established,
            now_ms,
        },
        &mut |action| {
            let _ = pending.push(action);
        },
    );
    let actions = ::core::mem::take(pending);
    let mut held = Some((source, sink));
    for action in actions {
        match action {
            ManagerAction::Admit {
                identity,
                slot,
                address,
                ..
            } => {
                if let Some((source, sink)) = held.take() {
                    let id = InterfaceId::from_channel_tag(
                        InterfaceKind::BluetoothPeer,
                        identity.as_bytes(),
                    );
                    status.member(slot).assign(id);
                    status.republish_peer_count();
                    let _ = fleet.register_member(core::descriptor(id, bitrate_bps));
                    members[slot] = Some(Active {
                        identity,
                        id,
                        slot,
                        address,
                        source,
                        sink,
                    });
                }
            }
            ManagerAction::Reject { .. } => held = None,
            other => apply_one(other, status, fleet, backend, members).await,
        }
    }
}
