//! The embassy driver: the same reactor as `tokio_reactor`, proven against the no_std host.
//! It races the same three inputs and runs the same engine sink-methods through the same
//! [`fire_due_reason`]/[`wait_for_due_reason`]; only the channel and select primitives differ, and
//! the interface boundary is the same [`InterfaceSeam`] contract behind an
//! [`EmbassyInterfaceSeam`]. That the dispatch *and* the seam are shared is the point: the sync core's shape holds across std and no_std.

use crate::interfaces::AttachedInterfaces;
use embassy_futures::select::{select4, select5, Either4, Either5};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use embassy_time::{Duration, Timer};
use heapless::Vec as HeaplessVec;

use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::engine::{
    ClassifiedInboundPacket, EngineReaction, EngineState, FanTarget, IngestIo, InstantMillis,
    IssuedCommand, Journaled, ProofRequest,
};
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{
    AirtimeUtilization, ConnectionState, FrameSink, InboundPacket, InterfaceDescriptor,
    InterfaceId, InterfaceKind, InterfaceStatus, PacketPhyStats, TransferRates,
};
use crate::reactor::announce_pacer::{AnnouncePacer, FixedPacerQueue};
use crate::reactor::grant::{
    AnyGrantConsumer, AnyGrantProducer, FrameSlot, FrameTarget, GrantConsumer, GrantProducer,
};
use crate::reactor::interface_seam::{
    InterfaceSeam, EMBEDDED_MAX_LINK_MTU, EMBEDDED_MAX_WIRE_FRAME_LEN,
};
use crate::reactor::kernel::{
    fire_due_reason, merge_wake_schedules_delta, route_reaction as route_engine_reaction,
    AnnounceDirective, DirectiveEgress,
};
use crate::reactor::timebase::EmbassyTimebase;
use crate::reactor::timers::{wait_for_due_reason, wait_for_pacer};
use crate::reactor::AppDeciders;
use crate::reactor::Host;
use crate::routing::links::resources::ResourceOffer;
use crate::runtime::{EmbassyInterfaceStore, InterfaceInspectionStore, NoInterfaceInspectionStore};
use crate::storage::DirtyInterfaceSet;
use crate::storage::StorageLayout;

/// A [`Host`] backed by embassy's clock and a caller-supplied entropy source: an
/// [`EmbassyTimebase`] owns the clock and `draw_entropy` is whatever the board hands it. The
/// engine never reads either; it asks the host.
pub struct EmbassyHost<E> {
    timebase: EmbassyTimebase,
    draw_entropy: E,
}

impl<E> EmbassyHost<E>
where
    E: FnMut(&mut [u8]),
{
    pub fn new(draw_entropy: E) -> Self {
        Self::new_with_timebase(EmbassyTimebase::capture_now(), draw_entropy)
    }

    pub fn new_with_timebase(timebase: EmbassyTimebase, draw_entropy: E) -> Self {
        Self {
            timebase,
            draw_entropy,
        }
    }
}

impl<E> Host for EmbassyHost<E>
where
    E: FnMut(&mut [u8]),
{
    fn now(&self) -> InstantMillis {
        self.timebase.now()
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        let remaining = deadline.0.saturating_sub(self.timebase.now().0);
        Timer::after(Duration::from_millis(remaining)).await;
    }

    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        (self.draw_entropy)(bytes);
    }
}

/// One interface's live state on the no_std host, shared by reference (a `&'static` parked in
/// a `StaticCell`) rather than an `Arc`: the interface writes it as the wire moves, the
/// display task reads it lock-free on its own render cadence. The byte counters are true `u64`
/// (a board left running must never wrap a card's numbers); the 32-bit esp32s3 has no native
/// 64-bit atomic, so `portable-atomic` carries them through critical-section.
pub struct EmbassyInterfaceStatus {
    id: AtomicU64,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    airtime: AtomicU32,
    transfer_rates: AtomicU64,
    enabled: AtomicBool,
}

const AIRTIME_UNPUBLISHED: u32 = u32::MAX;
const RATES_UNPUBLISHED: u64 = u64::MAX;

impl EmbassyInterfaceStatus {
    #[must_use]
    pub const fn new(id: InterfaceId, connection: ConnectionState) -> Self {
        Self {
            id: AtomicU64::new(u64::from_be_bytes(*id.as_bytes())),
            connection: AtomicU8::new(connection.as_u8()),
            rx: AtomicU64::new(0),
            tx: AtomicU64::new(0),
            airtime: AtomicU32::new(AIRTIME_UNPUBLISHED),
            transfer_rates: AtomicU64::new(RATES_UNPUBLISHED),
            enabled: AtomicBool::new(true),
        }
    }

    pub fn set_connection(&self, connection: ConnectionState) {
        self.connection.store(connection.as_u8(), Ordering::Relaxed);
    }

    pub fn set_id(&self, id: InterfaceId) {
        self.id
            .store(u64::from_be_bytes(*id.as_bytes()), Ordering::Relaxed);
    }

    /// Turn this interface off or back on from the application. The driver reads
    /// [`is_enabled`](Self::is_enabled) and goes dormant (wire closed, nothing ingested, egress
    /// drained and discarded) while off, holding its slot and routes for an instant resume.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether the interface is enabled (the default). The driver polls this to leave or re-enter
    /// its dormant state.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn add_rx(&self, bytes: u64) {
        self.rx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_tx(&self, bytes: u64) {
        self.tx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn set_airtime(&self, utilization: AirtimeUtilization) {
        let packed =
            (u32::from(utilization.short_per_mille) << 16) | u32::from(utilization.long_per_mille);
        self.airtime.store(packed, Ordering::Relaxed);
    }

    pub fn set_transfer_rates(&self, rates: TransferRates) {
        let packed = (u64::from(rates.rx_bps) << 32) | u64::from(rates.tx_bps);
        self.transfer_rates.store(packed, Ordering::Relaxed);
    }
}

impl InterfaceStatus for EmbassyInterfaceStatus {
    fn id(&self) -> InterfaceId {
        InterfaceId::new(self.id.load(Ordering::Relaxed).to_be_bytes())
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            return ConnectionState::Disabled;
        }
        ConnectionState::from_u8(self.connection.load(Ordering::Relaxed))
    }

    fn rx_bytes(&self) -> u64 {
        self.rx.load(Ordering::Relaxed)
    }

    fn tx_bytes(&self) -> u64 {
        self.tx.load(Ordering::Relaxed)
    }

    fn airtime(&self) -> Option<AirtimeUtilization> {
        let packed = self.airtime.load(Ordering::Relaxed);
        if packed == AIRTIME_UNPUBLISHED {
            return None;
        }
        Some(AirtimeUtilization {
            short_per_mille: (packed >> 16) as u16,
            long_per_mille: packed as u16,
        })
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        let packed = self.transfer_rates.load(Ordering::Relaxed);
        if packed == RATES_UNPUBLISHED {
            return None;
        }
        Some(TransferRates {
            rx_bps: (packed >> 32) as u32,
            tx_bps: packed as u32,
        })
    }
}

/// One interface's grant lane on embassy: the slots live in a caller-parked buffer
/// (a `StaticCell` on firmware) and recycle through a `zerocopy_channel`, so granting,
/// committing, and releasing move ring indices while the frame bytes stay where they
/// were written — and each interface's buffer is sized to its own `HW_MTU`.
pub fn embassy_grant_lane<'a, M: RawMutex, const SLOT: usize>(
    channel: &'a mut zerocopy_channel::Channel<'a, M, FrameSlot<SLOT>>,
) -> (
    EmbassyGrantProducer<'a, M, SLOT>,
    EmbassyGrantConsumer<'a, M, SLOT>,
) {
    let (sender, receiver) = channel.split();
    (
        EmbassyGrantProducer {
            sender,
            granted: false,
            wake: None,
        },
        EmbassyGrantConsumer {
            receiver,
            peeked: false,
        },
    )
}

/// `granted`/`peeked` guard the ring's done-calls: the channel's `send_done`/
/// `receive_done` assert an open grant, so a `commit` or `release` with nothing
/// outstanding must be a no-op, exactly as it is on the tokio lanes.
pub struct EmbassyGrantProducer<'a, M: RawMutex, const SLOT: usize> {
    sender: zerocopy_channel::Sender<'a, M, FrameSlot<SLOT>>,
    granted: bool,
    wake: Option<&'a Signal<M, ()>>,
}

impl<'a, M: RawMutex, const SLOT: usize> EmbassyGrantProducer<'a, M, SLOT> {
    /// Arm this lane to signal `wake` whenever a frame is committed onto it. A fleet's shared
    /// egress lane is consumed by a supervisor on another task, which parks on this signal
    /// rather than the channel's own consumer waker, so the reactor's commit reliably rouses
    /// the cross-task drain. A 1:1 lane whose consumer is the reactor itself leaves this unset.
    pub fn set_outbound_wake(&mut self, wake: &'a Signal<M, ()>) {
        self.wake = Some(wake);
    }
}

impl<M: RawMutex, const SLOT: usize> GrantProducer<SLOT> for EmbassyGrantProducer<'_, M, SLOT> {
    fn try_grant(&mut self) -> Option<&mut FrameSlot<SLOT>> {
        let granted = &mut self.granted;
        let slot = self.sender.try_send()?;
        *granted = true;
        Some(slot)
    }

    async fn grant(&mut self) -> &mut FrameSlot<SLOT> {
        let granted = &mut self.granted;
        let slot = self.sender.send().await;
        *granted = true;
        slot
    }

    fn commit(&mut self) {
        if self.granted {
            self.granted = false;
            self.sender.send_done();
            if let Some(wake) = self.wake {
                wake.signal(());
            }
        }
    }
}

impl<M: RawMutex, const SLOT: usize> AnyGrantProducer for EmbassyGrantProducer<'_, M, SLOT> {
    fn try_fill_frame_for(&mut self, interface_id: InterfaceId, frame: &[u8]) -> bool {
        if frame.len() > SLOT {
            return false;
        }
        let Some(slot) = GrantProducer::try_grant(self) else {
            return false;
        };
        slot.fill_for(interface_id, frame);
        GrantProducer::commit(self);
        true
    }

    fn try_fill_frame_fan(&mut self, fan: FanTarget, frame: &[u8]) -> bool {
        if frame.len() > SLOT {
            return false;
        }
        let Some(slot) = GrantProducer::try_grant(self) else {
            return false;
        };
        slot.fill_for_fan(fan, frame);
        GrantProducer::commit(self);
        true
    }
}

pub struct EmbassyGrantConsumer<'a, M: RawMutex, const SLOT: usize> {
    receiver: zerocopy_channel::Receiver<'a, M, FrameSlot<SLOT>>,
    peeked: bool,
}

impl<M: RawMutex, const SLOT: usize> GrantConsumer<SLOT> for EmbassyGrantConsumer<'_, M, SLOT> {
    fn try_peek(&mut self) -> Option<&mut FrameSlot<SLOT>> {
        let peeked = &mut self.peeked;
        let slot = self.receiver.try_receive()?;
        *peeked = true;
        Some(slot)
    }

    async fn peek(&mut self) -> &mut FrameSlot<SLOT> {
        let peeked = &mut self.peeked;
        let slot = self.receiver.receive().await;
        *peeked = true;
        slot
    }

    fn release(&mut self) {
        if self.peeked {
            self.peeked = false;
            self.receiver.receive_done();
        }
    }
}

impl<M: RawMutex, const SLOT: usize> AnyGrantConsumer for EmbassyGrantConsumer<'_, M, SLOT> {
    fn try_peek_frame(&mut self) -> Option<(FrameTarget, PacketPhyStats, &mut [u8])> {
        let slot = GrantConsumer::try_peek(self)?;
        let target = slot.target;
        let packet_phy = slot.packet_phy;
        Some((target, packet_phy, slot.frame_mut()))
    }

    fn release_frame(&mut self) {
        GrantConsumer::release(self);
    }
}

/// The embassy side of one interface's seam: `next_inbound` fills this interface's own
/// inbound grant lane in place and announces the commit on the reactor's notify funnel,
/// and `next_outbound` parks on its outbound lane until the reactor grants a frame in,
/// lending it from the slot it was filled into.
pub struct EmbassyInterfaceSeam<'a, M: RawMutex, const NOTIFY: usize, const SLOT: usize> {
    id: InterfaceId,
    inbound: EmbassyGrantProducer<'a, M, SLOT>,
    notify: Sender<'a, M, InterfaceId, NOTIFY>,
    outbound: EmbassyGrantConsumer<'a, M, SLOT>,
}

impl<'a, M: RawMutex, const NOTIFY: usize, const SLOT: usize>
    EmbassyInterfaceSeam<'a, M, NOTIFY, SLOT>
{
    #[must_use]
    pub fn new(
        id: InterfaceId,
        inbound: EmbassyGrantProducer<'a, M, SLOT>,
        notify: Sender<'a, M, InterfaceId, NOTIFY>,
        outbound: EmbassyGrantConsumer<'a, M, SLOT>,
    ) -> Self {
        Self {
            id,
            inbound,
            notify,
            outbound,
        }
    }
}

impl<M: RawMutex, const NOTIFY: usize, const SLOT: usize> InterfaceSeam
    for EmbassyInterfaceSeam<'_, M, NOTIFY, SLOT>
{
    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        let slot = self.inbound.grant().await;
        slot.target = FrameTarget::Direct(self.id);
        slot
    }

    async fn commit_inbound(&mut self) {
        let slot = self.inbound.grant().await;
        if slot.len == 0 {
            return;
        }
        self.inbound.commit();
        let _ = self.notify.try_send(self.id);
    }

    async fn next_inbound_with_phy(&mut self, frame: &[u8], packet_phy: PacketPhyStats) {
        let slot = self.inbound.grant().await;
        slot.clear();
        if slot.extend_from_slice(frame).is_err() {
            return;
        }
        slot.packet_phy = packet_phy;
        self.commit_inbound().await;
    }

    async fn next_outbound(&mut self) -> &[u8] {
        self.outbound.release();
        self.outbound.peek().await.frame()
    }
}

/// Whether the lane keyed by `lane_key` carries traffic for `target`: an exact id match (a
/// dedicated 1:1 lane owns exactly its interface), or `target` being a child of the fleet
/// supervisor `lane_key` names. The second is how one shared lane serves a whole fleet with no
/// per-child routing entry: a `WifiPeer` frame finds the lane keyed by the `AutoWifi`
/// supervisor by the kind byte alone. With no supervisor lane registered, only the exact match fires.
fn lane_serves(lane_key: InterfaceId, target: InterfaceId) -> bool {
    if lane_key == target {
        return true;
    }
    match (lane_key.kind(), target.kind()) {
        (Some(supervisor), Some(child)) => supervisor.member_kind() == Some(child),
        _ => false,
    }
}

/// The reactor's egress: it routes one engine `Directive::Send` into the target interface's
/// outbound grant lane, granted, filled in place, committed; a full lane drops the frame
/// rather than stalling the reactor. [`EmbassyEgress`] and [`PooledEgress`] both satisfy it.
pub trait ReactorEgress {
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]);
    /// Route one fleet broadcast: find the standing lane owned by a supervisor of `supervisor` kind
    /// and commit a single frame carrying `fan`, for the supervisor to fan across the members it
    /// selects. A full lane drops it, exactly as [`enqueue`](Self::enqueue) does for a direct send.
    fn enqueue_broadcast(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]);
    fn lane_for(&self, target: InterfaceId) -> Option<InterfaceId> {
        Some(target)
    }
    fn fleet_lane(&self, _supervisor: InterfaceKind) -> Option<InterfaceId> {
        None
    }
}

/// The fixed-set egress: each lane's slot size is erased, so lanes sized to different
/// interfaces share one slice. No alloc — the lanes are a borrowed slice the caller owns.
pub struct EmbassyEgress<'a> {
    lanes: &'a mut [(InterfaceId, &'a mut dyn AnyGrantProducer)],
}

impl<'a> EmbassyEgress<'a> {
    #[must_use]
    pub fn new(lanes: &'a mut [(InterfaceId, &'a mut dyn AnyGrantProducer)]) -> Self {
        Self { lanes }
    }
}

impl ReactorEgress for EmbassyEgress<'_> {
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]) {
        for (id, producer) in self.lanes.iter_mut() {
            if lane_serves(*id, target) {
                let _ = producer.try_fill_frame_for(target, bytes);
                return;
            }
        }
    }

    fn enqueue_broadcast(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]) {
        for (id, producer) in self.lanes.iter_mut() {
            if id.kind() == Some(supervisor) {
                let _ = producer.try_fill_frame_fan(fan, bytes);
                return;
            }
        }
    }

    fn lane_for(&self, target: InterfaceId) -> Option<InterfaceId> {
        self.lanes
            .iter()
            .map(|(id, _)| *id)
            .find(|id| lane_serves(*id, target))
    }

    fn fleet_lane(&self, supervisor: InterfaceKind) -> Option<InterfaceId> {
        self.lanes
            .iter()
            .map(|(id, _)| *id)
            .find(|id| id.kind() == Some(supervisor))
    }
}

/// Run the reactor loop on an embassy executor until the task is dropped. Each turn parks on
/// the three inputs and runs the one sync engine method the winner names. `Idle` arms no
/// timer, so the select rests on the two channels and the core truly sleeps: the dormancy an
/// MCU is built for.
/// Everything the reactor is wired to for one run. All borrowed: the caller owns every lane
/// for the reactor's whole life.
pub struct ReactorWiring<'run, 'lane, M: RawMutex, const NOTIFY: usize, const COMMANDS: usize> {
    pub interfaces: AttachedInterfaces<'run>,
    pub ifacs: &'run [InterfaceIfac],
    /// The inbound doorbell, not a queue: seams and supervisors ring it with `try_send` — a
    /// full channel means a sweep is already owed, so a dropped ring is safe — and one ring
    /// sweeps every lane dry, so queued rings collapse into it and a frame committed during
    /// the sweep either meets it or rings afresh.
    pub notify: Receiver<'run, M, InterfaceId, NOTIFY>,
    pub inbound_lanes: &'run mut [(InterfaceId, &'lane mut dyn AnyGrantConsumer)],
    pub commands: Receiver<'run, M, IssuedCommand, COMMANDS>,
    pub egress: EmbassyEgress<'run>,
}

fn retain_packet_phy<Store: InterfaceInspectionStore>(
    store: &Store,
    packet: &ClassifiedInboundPacket<'_>,
    packet_phy: PacketPhyStats,
) {
    if !Store::RETAINS_PACKET_PHY || packet_phy.is_empty() {
        return;
    }
    if let Some(packet_hash) = packet.packet_hash() {
        store.remember_packet_phy(packet_hash, packet_phy);
    }
}

pub async fn run<S, H, M, const NOTIFY: usize, const COMMANDS: usize>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring<'_, '_, M, NOTIFY, COMMANDS>,
    on_journaled: impl FnMut(Journaled<'_>),
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex,
{
    run_with_deciders(
        engine,
        host,
        wiring,
        on_journaled,
        crate::reactor::decline_all(),
    )
    .await
}

pub async fn run_with_deciders<S, H, M, P, A, const NOTIFY: usize, const COMMANDS: usize>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring<'_, '_, M, NOTIFY, COMMANDS>,
    on_journaled: impl FnMut(Journaled<'_>),
    deciders: AppDeciders<P, A>,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex,
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        deciders,
        &NoInterfaceInspectionStore,
    )
    .await
}

pub async fn run_with_store<
    S,
    H,
    M,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const INTERFACES: usize,
    const PACKET_PHY_CAPACITY: usize,
    const PACKET_PHY_INDEX_BUCKETS: usize,
>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring<'_, '_, M, NOTIFY, COMMANDS>,
    on_journaled: impl FnMut(Journaled<'_>),
    store: &EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex + Sync,
{
    assert!(
        INTERFACES >= wiring.interfaces.len(),
        "EmbassyInterfaceStore INTERFACES must cover every attached interface"
    );
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        crate::reactor::decline_all(),
        store,
    )
    .await
}

async fn run_inner<S, H, M, P, A, Store, const NOTIFY: usize, const COMMANDS: usize>(
    mut engine: EngineState<S>,
    mut host: H,
    wiring: ReactorWiring<'_, '_, M, NOTIFY, COMMANDS>,
    mut on_journaled: impl FnMut(Journaled<'_>),
    deciders: AppDeciders<P, A>,
    store: &Store,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex,
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
    Store: InterfaceInspectionStore,
{
    let AppDeciders {
        mut should_prove,
        mut should_accept_resource,
    } = deciders;
    let ReactorWiring {
        interfaces,
        ifacs,
        notify,
        inbound_lanes,
        commands,
        mut egress,
    } = wiring;
    let mut wake_schedules = engine.wake_schedules(interfaces);
    let mut pacers: HeaplessVec<InterfacePacer, MAX_PACED_INTERFACES> = HeaplessVec::new();
    for descriptor in interfaces {
        let _ = pacers.push(InterfacePacer {
            id: descriptor.id,
            pacer: AnnouncePacer::new(descriptor.announce_bandwidth_cap, descriptor.bitrate),
        });
    }
    loop {
        let wake = wake_schedules.soonest(host.now());
        let pacer_wake = soonest_pacer_release(&pacers);

        match select4(
            notify.receive(),
            commands.receive(),
            wait_for_due_reason(&host, wake),
            wait_for_pacer(&host, pacer_wake),
        )
        .await
        {
            Either4::First(_) => {
                while notify.try_receive().is_ok() {}
                for (lane_id, lane) in inbound_lanes.iter_mut() {
                    while let Some((target, packet_phy, frame)) = lane.try_peek_frame() {
                        let FrameTarget::Direct(source) = target else {
                            lane.release_frame();
                            continue;
                        };
                        let mut unmasked = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
                        let bytes = match ifac_for(ifacs, *lane_id) {
                            Some(entry) => {
                                let Some(clean_len) =
                                    entry.context.unmask_inbound(frame, &mut unmasked)
                                else {
                                    lane.release_frame();
                                    continue;
                                };
                                &mut unmasked[..clean_len]
                            }
                            None => frame,
                        };
                        let now = host.now();
                        let packet = ClassifiedInboundPacket::classify(InboundPacket {
                            arrived_at: now,
                            source_interface: source,
                            bytes,
                        });
                        retain_packet_phy(store, &packet, packet_phy);
                        let delta = engine.ingest_classified_into(
                            packet,
                            IngestIo {
                                interfaces,
                                now,
                                fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                                should_prove: &mut should_prove,
                                should_accept_resource: &mut should_accept_resource,
                                sink: &mut |reaction| {
                                    route_reaction(
                                        reaction,
                                        &mut egress,
                                        ifacs,
                                        &mut pacers,
                                        now,
                                        &mut on_journaled,
                                    )
                                },
                            },
                        );
                        lane.release_frame();
                        merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
                    }
                }
            }
            Either4::Second(issued) => {
                let now = host.now();
                let delta = engine.ingest_command_into(
                    issued,
                    interfaces,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut egress,
                            ifacs,
                            &mut pacers,
                            now,
                            &mut on_journaled,
                        )
                    },
                );
                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
            }
            Either4::Third(reason) => {
                let now = host.now();
                let delta = fire_due_reason(
                    &mut engine,
                    reason,
                    now,
                    interfaces,
                    &mut |bytes| host.fill_entropy(bytes),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut egress,
                            ifacs,
                            &mut pacers,
                            now,
                            &mut on_journaled,
                        )
                    },
                );
                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
            }
            Either4::Fourth(()) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &mut egress, ifacs);
            }
        }
        if Store::RETAINS_COUNTS {
            let mut dirty = engine.take_dirty_interfaces();
            let mut changed = false;
            dirty.drain(|interface| {
                if interfaces
                    .iter()
                    .any(|descriptor| descriptor.id == interface)
                {
                    store.set_interface_counts(interface, engine.interface_counts(interface));
                } else {
                    store.forget_interface(interface);
                }
                changed = true;
            });
            if changed {
                store.signal_interface_counts_changed();
            }
        }
    }
}

const MAX_PACED_INTERFACES: usize = 2;
const PACER_DEPTH: usize = 2;

struct InterfacePacer {
    id: InterfaceId,
    pacer: AnnouncePacer<FixedPacerQueue<PACER_DEPTH>>,
}

fn route_reaction(
    reaction: EngineReaction<'_>,
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    app: &mut impl FnMut(Journaled<'_>),
) {
    let mut directive_egress = EmbassyDirectiveEgress {
        egress,
        ifacs,
        pacers,
        now,
    };
    route_engine_reaction(reaction, &mut directive_egress, app);
}

struct EmbassyDirectiveEgress<'a, E> {
    egress: &'a mut E,
    ifacs: &'a [InterfaceIfac],
    pacers: &'a mut [InterfacePacer],
    now: InstantMillis,
}

impl<E: ReactorEgress> DirectiveEgress for EmbassyDirectiveEgress<'_, E> {
    fn send(&mut self, target: InterfaceId, bytes: &[u8]) {
        enqueue_for_wire(self.egress, self.ifacs, target, bytes);
    }

    fn send_announce(&mut self, target: InterfaceId, announce: AnnounceDirective<'_>) {
        offer_to_pacer(
            self.pacers,
            target,
            announce.bytes(),
            announce.hops(),
            self.now,
            self.egress,
            self.ifacs,
        );
    }

    fn send_to_fleet(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]) {
        enqueue_broadcast_for_wire(self.egress, self.ifacs, supervisor, fan, bytes);
    }

    fn send_announce_to_fleet(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        announce: AnnounceDirective<'_>,
    ) {
        enqueue_broadcast_for_wire(self.egress, self.ifacs, supervisor, fan, announce.bytes());
    }

    fn emit_frame(
        &mut self,
        target: InterfaceId,
        _size_hint: usize,
        fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
    ) {
        emit_for_wire(self.egress, self.ifacs, target, fill);
    }
}

/// Grant-first lands as build-then-copy here: the erased embassy egress exposes only
/// `try_fill_frame`, so the frame is built in a stack scratch (wire-small on embassy
/// faces) and copied once into the ring. `fill` runs exactly once — the `EmitFrame`
/// contract — whether or not the lane has room.
fn emit_for_wire(
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
) {
    let mut frame = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
    if let Some(len) = fill(&mut frame) {
        enqueue_for_wire(egress, ifacs, target, &frame[..len]);
    }
}

fn ifac_for(ifacs: &[InterfaceIfac], id: InterfaceId) -> Option<&InterfaceIfac> {
    if ifacs.is_empty() {
        return None;
    }
    ifacs.iter().find(|entry| entry.id == id)
}

fn enqueue_for_wire(
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
) {
    let lane = egress.lane_for(target).unwrap_or(target);
    match ifac_for(ifacs, lane) {
        Some(entry) => {
            let mut wire = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
            if let Some(masked_len) = entry.context.mask_outbound(bytes, &mut wire) {
                egress.enqueue(target, &wire[..masked_len]);
            }
        }
        None => egress.enqueue(target, bytes),
    }
}

fn enqueue_broadcast_for_wire(
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
    supervisor: InterfaceKind,
    fan: FanTarget,
    bytes: &[u8],
) {
    match egress
        .fleet_lane(supervisor)
        .and_then(|lane| ifac_for(ifacs, lane))
    {
        Some(entry) => {
            let mut wire = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
            if let Some(masked_len) = entry.context.mask_outbound(bytes, &mut wire) {
                egress.enqueue_broadcast(supervisor, fan, &wire[..masked_len]);
            }
        }
        None => egress.enqueue_broadcast(supervisor, fan, bytes),
    }
}

/// Whether `id` owns one of the reactor's standing lanes outright: an exact-id match, not the
/// kind match a fleet member rides. Only a dedicated-lane owner earns an announce pacer; a
/// fleet member shares its supervisor's lane and so its medium's pacing, never its own ~1 KiB
/// queue. This keeps the pacer pool bounded by lane count, not member count.
fn owns_dedicated_lane<C>(lanes: &[(InterfaceId, C)], id: InterfaceId) -> bool {
    lanes.iter().any(|(lane_id, _)| *lane_id == id)
}

fn offer_to_pacer(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    bytes: &[u8],
    hops: u8,
    now: InstantMillis,
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
) {
    match pacers.iter_mut().find(|entry| entry.id == target) {
        Some(entry) => {
            entry.pacer.offer(bytes, hops, now, |frame| {
                enqueue_for_wire(egress, ifacs, target, frame)
            });
        }
        None => enqueue_for_wire(egress, ifacs, target, bytes),
    }
}

fn flush_due_pacers(
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
) {
    for entry in pacers.iter_mut() {
        let target = entry.id;
        entry
            .pacer
            .release_due(now, |frame| enqueue_for_wire(egress, ifacs, target, frame));
    }
}

fn soonest_pacer_release(pacers: &[InterfacePacer]) -> Option<InstantMillis> {
    pacers
        .iter()
        .filter_map(|entry| entry.pacer.next_release())
        .min_by_key(|deadline| deadline.0)
}

/// The runtime's lever to bring one engine interface up or down between cycles: a supervisor
/// sends `Add` when a peer is confirmed and `Remove` when it drops, and the reactor adds or
/// drops the descriptor (plus an announce pacer only for a dedicated-lane owner). No lane is
/// allocated: frames route to the medium's standing lane by [`lane_serves`], so a fleet of
/// members shares one lane and `Add`/`Remove` only touch the cheap descriptor set.
// repr(C): crosses the dual-core channel; see the layout note on `EngineCommand`.
#[repr(C)]
pub enum InterfaceLifecycle {
    Add {
        descriptor: InterfaceDescriptor,
    },
    Remove {
        id: InterfaceId,
    },
    Retag {
        old_id: InterfaceId,
        new_id: InterfaceId,
        descriptor: InterfaceDescriptor,
    },
}

/// The dynamic egress: a fixed pool of `N` permanently-owned outbound lane endpoints, each
/// tagged with the id of the interface or fleet supervisor it serves. The uniform `SLOT` size
/// (a board's one wire ceiling) lets the pool own concrete endpoints rather than the fixed
/// path's erased `&mut dyn`, and the tag lets [`lane_serves`] route a whole fleet over one lane.
pub struct PooledEgress<M: RawMutex + 'static, const SLOT: usize, const N: usize> {
    lanes: HeaplessVec<(InterfaceId, EmbassyGrantProducer<'static, M, SLOT>), N>,
}

impl<M: RawMutex + 'static, const SLOT: usize, const N: usize> PooledEgress<M, SLOT, N> {
    /// Build the egress over the reactor-side outbound endpoints, each at the slot it shares
    /// with the interface-side half and tagged at boot with the id it serves.
    #[must_use]
    pub fn new(
        lanes: HeaplessVec<(InterfaceId, EmbassyGrantProducer<'static, M, SLOT>), N>,
    ) -> Self {
        Self { lanes }
    }

    pub(crate) fn activate(&mut self, slot: usize, id: InterfaceId) {
        if let Some(entry) = self.lanes.get_mut(slot) {
            entry.0 = id;
        }
    }

    pub(crate) fn retag(&mut self, old_id: InterfaceId, new_id: InterfaceId) {
        for (id, _) in self.lanes.iter_mut() {
            if *id == old_id {
                *id = new_id;
            }
        }
    }
}

impl<M: RawMutex + 'static, const SLOT: usize, const N: usize> ReactorEgress
    for PooledEgress<M, SLOT, N>
{
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]) {
        for (id, producer) in self.lanes.iter_mut() {
            if lane_serves(*id, target) {
                let _ = producer.try_fill_frame_for(target, bytes);
                return;
            }
        }
    }

    fn enqueue_broadcast(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]) {
        for (id, producer) in self.lanes.iter_mut() {
            if id.kind() == Some(supervisor) {
                let _ = producer.try_fill_frame_fan(fan, bytes);
                return;
            }
        }
    }

    fn lane_for(&self, target: InterfaceId) -> Option<InterfaceId> {
        self.lanes
            .iter()
            .map(|(id, _)| *id)
            .find(|id| lane_serves(*id, target))
    }

    fn fleet_lane(&self, supervisor: InterfaceKind) -> Option<InterfaceId> {
        self.lanes
            .iter()
            .map(|(id, _)| *id)
            .find(|id| id.kind() == Some(supervisor))
    }
}

/// Cap an interface's advertised link MTU to what this reactor's lanes can carry. The reactor
/// owns the frame buffers ([`EMBEDDED_MAX_WIRE_FRAME_LEN`]), so it is the authority on the
/// largest wire any interface on this board can move; link negotiation can never settle above
/// the slot the frame must land in. An interface that declares no MTU keeps its broadcast-floor fallback.
fn clamp_to_embedded_ceiling(mut descriptor: InterfaceDescriptor) -> InterfaceDescriptor {
    if let Some(mtu) = descriptor.hardware_mtu {
        descriptor.hardware_mtu = Some(mtu.min(EMBEDDED_MAX_LINK_MTU));
    }
    descriptor
}

/// The runtime's reactor: the same loop as [`run`], over a fixed pool of `N` interface slots
/// whose live descriptor set changes at runtime through the `lifecycle` lane. A supervisor
/// registers a peer with [`InterfaceLifecycle::Add`] (its frames route to the supervisor's
/// lane by kind) and drops it with [`InterfaceLifecycle::Remove`]; on remove the reactor culls
/// the gone interface's routes exactly as the host runtime does. Alloc-free: the endpoints
/// never move, only the active descriptor set changes; pacers are bounded by `LANES`, not `MAX_IFACES`.
/// Everything the pooled reactor is wired to. All borrowed: the recipe owns every lane for the
/// node's life.
pub struct PooledWiring<
    'run,
    M: RawMutex + 'static,
    const SLOT: usize,
    const LANES: usize,
    const MAX_IFACES: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
> {
    pub initial: &'run HeaplessVec<InterfaceDescriptor, MAX_IFACES>,
    pub ifacs: &'run mut HeaplessVec<InterfaceIfac, LANES>,
    pub inbound:
        &'run mut HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, SLOT>), LANES>,
    pub egress: &'run mut PooledEgress<M, SLOT, LANES>,
    pub notify: Receiver<'run, M, InterfaceId, NOTIFY>,
    pub commands: Receiver<'run, M, IssuedCommand, COMMANDS>,
    pub lifecycle: Receiver<'run, M, InterfaceLifecycle, LIFECYCLE>,
}

pub(crate) async fn run_pooled<
    S,
    H,
    M,
    Store,
    const SLOT: usize,
    const LANES: usize,
    const MAX_IFACES: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
>(
    engine: &mut EngineState<S>,
    host: &mut H,
    wiring: PooledWiring<'_, M, SLOT, LANES, MAX_IFACES, NOTIFY, COMMANDS, LIFECYCLE>,
    mut on_journaled: impl FnMut(Journaled<'_>),
    deciders: AppDeciders<impl FnMut(&ProofRequest) -> bool, impl FnMut(&ResourceOffer) -> bool>,
    store: &Store,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex + 'static,
    Store: InterfaceInspectionStore,
{
    let AppDeciders {
        mut should_prove,
        mut should_accept_resource,
    } = deciders;
    let PooledWiring {
        initial,
        ifacs,
        inbound,
        egress,
        notify,
        commands,
        lifecycle,
    } = wiring;
    let mut descriptors: HeaplessVec<InterfaceDescriptor, MAX_IFACES> = HeaplessVec::new();
    let mut pacers: HeaplessVec<InterfacePacer, LANES> = HeaplessVec::new();
    for descriptor in initial {
        let descriptor = clamp_to_embedded_ceiling(*descriptor);
        engine.interface_attached(descriptor.id, host.now());
        let _ = descriptors.push(descriptor);
        if owns_dedicated_lane(inbound, descriptor.id) {
            let _ = pacers.push(InterfacePacer {
                id: descriptor.id,
                pacer: AnnouncePacer::new(descriptor.announce_bandwidth_cap, descriptor.bitrate),
            });
        }
    }
    let mut wake_schedules = engine.wake_schedules(AttachedInterfaces::new(&descriptors));
    loop {
        let wake = wake_schedules.soonest(host.now());
        let pacer_wake = soonest_pacer_release(&pacers);

        match select5(
            notify.receive(),
            commands.receive(),
            wait_for_due_reason(&*host, wake),
            wait_for_pacer(&*host, pacer_wake),
            lifecycle.receive(),
        )
        .await
        {
            Either5::First(_) => {
                while notify.try_receive().is_ok() {}
                for (lane_id, lane) in inbound.iter_mut() {
                    while let Some((target, packet_phy, frame)) = lane.try_peek_frame() {
                        let FrameTarget::Direct(source) = target else {
                            lane.release_frame();
                            continue;
                        };
                        let mut unmasked = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
                        let bytes = match ifac_for(ifacs, *lane_id) {
                            Some(entry) => {
                                let Some(clean_len) =
                                    entry.context.unmask_inbound(frame, &mut unmasked)
                                else {
                                    lane.release_frame();
                                    continue;
                                };
                                &mut unmasked[..clean_len]
                            }
                            None => frame,
                        };
                        let now = host.now();
                        let packet = ClassifiedInboundPacket::classify(InboundPacket {
                            arrived_at: now,
                            source_interface: source,
                            bytes,
                        });
                        retain_packet_phy(store, &packet, packet_phy);
                        let delta = engine.ingest_classified_into(
                            packet,
                            IngestIo {
                                interfaces: AttachedInterfaces::new(&descriptors),
                                now,
                                fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                                should_prove: &mut should_prove,
                                should_accept_resource: &mut should_accept_resource,
                                sink: &mut |reaction| {
                                    route_reaction(
                                        reaction,
                                        &mut *egress,
                                        ifacs,
                                        &mut pacers,
                                        now,
                                        &mut on_journaled,
                                    )
                                },
                            },
                        );
                        lane.release_frame();
                        merge_wake_schedules_delta(
                            &mut wake_schedules,
                            delta,
                            &*engine,
                            AttachedInterfaces::new(&descriptors),
                        );
                    }
                }
            }
            Either5::Second(issued) => {
                let now = host.now();
                let delta = engine.ingest_command_into(
                    issued,
                    AttachedInterfaces::new(&descriptors),
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut *egress,
                            ifacs,
                            &mut pacers,
                            now,
                            &mut on_journaled,
                        )
                    },
                );
                merge_wake_schedules_delta(
                    &mut wake_schedules,
                    delta,
                    &*engine,
                    AttachedInterfaces::new(&descriptors),
                );
            }
            Either5::Third(reason) => {
                let now = host.now();
                let delta = fire_due_reason(
                    &mut *engine,
                    reason,
                    now,
                    AttachedInterfaces::new(&descriptors),
                    &mut |bytes| host.fill_entropy(bytes),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut *egress,
                            ifacs,
                            &mut pacers,
                            now,
                            &mut on_journaled,
                        )
                    },
                );
                merge_wake_schedules_delta(
                    &mut wake_schedules,
                    delta,
                    &*engine,
                    AttachedInterfaces::new(&descriptors),
                );
            }
            Either5::Fourth(()) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &mut *egress, ifacs);
            }
            Either5::Fifth(message) => match message {
                InterfaceLifecycle::Add { descriptor } => {
                    let descriptor = clamp_to_embedded_ceiling(descriptor);
                    let id = descriptor.id;
                    let present = descriptors.iter().any(|existing| existing.id == id);
                    if !present {
                        engine.interface_attached(id, host.now());
                        let _ = descriptors.push(descriptor);
                        if owns_dedicated_lane(inbound, id) {
                            let _ = pacers.push(InterfacePacer {
                                id,
                                pacer: AnnouncePacer::new(
                                    descriptor.announce_bandwidth_cap,
                                    descriptor.bitrate,
                                ),
                            });
                        }
                        wake_schedules =
                            engine.wake_schedules(AttachedInterfaces::new(&descriptors));
                    }
                    #[cfg(feature = "log")]
                    log::info!(
                        "reactor: Add kind={:?} present={present} descriptors={}",
                        id.kind(),
                        descriptors.len()
                    );
                }
                InterfaceLifecycle::Remove { id } => {
                    let found = descriptors
                        .iter()
                        .position(|descriptor| descriptor.id == id);
                    if let Some(pos) = found {
                        let _ = descriptors.swap_remove(pos);
                    }
                    #[cfg(feature = "log")]
                    log::info!(
                        "reactor: Remove kind={:?} found={} descriptors={}",
                        id.kind(),
                        found.is_some(),
                        descriptors.len()
                    );
                    if let Some(pos) = pacers.iter().position(|pacer| pacer.id == id) {
                        let _ = pacers.swap_remove(pos);
                    }
                    let now = host.now();
                    engine.cull_expired_routes(
                        now,
                        AttachedInterfaces::new(&descriptors),
                        &mut |reaction| {
                            route_reaction(
                                reaction,
                                &mut *egress,
                                ifacs,
                                &mut pacers,
                                now,
                                &mut on_journaled,
                            )
                        },
                    );
                    wake_schedules = engine.wake_schedules(AttachedInterfaces::new(&descriptors));
                }
                InterfaceLifecycle::Retag {
                    old_id,
                    new_id,
                    descriptor,
                } => {
                    let descriptor = clamp_to_embedded_ceiling(descriptor);
                    let present = descriptors
                        .iter()
                        .position(|existing| existing.id == old_id);
                    let collides = descriptors.iter().any(|existing| existing.id == new_id);
                    if let (Some(slot), false) = (present, collides) {
                        descriptors[slot] = descriptor;
                        egress.retag(old_id, new_id);
                        if let Some(entry) = inbound.iter_mut().find(|(id, _)| *id == old_id) {
                            entry.0 = new_id;
                        }
                        if let Some(entry) = ifacs.iter_mut().find(|entry| entry.id == old_id) {
                            entry.id = new_id;
                        }
                        if let Some(pos) = pacers.iter().position(|pacer| pacer.id == old_id) {
                            pacers[pos] = InterfacePacer {
                                id: new_id,
                                pacer: AnnouncePacer::new(
                                    descriptor.announce_bandwidth_cap,
                                    descriptor.bitrate,
                                ),
                            };
                        }
                        wake_schedules =
                            engine.wake_schedules(AttachedInterfaces::new(&descriptors));
                    }
                }
            },
        }
        if Store::RETAINS_COUNTS {
            let mut dirty = engine.take_dirty_interfaces();
            let mut changed = false;
            dirty.drain(|interface| {
                if descriptors
                    .iter()
                    .any(|descriptor| descriptor.id == interface)
                {
                    store.set_interface_counts(interface, engine.interface_counts(interface));
                } else {
                    store.forget_interface(interface);
                }
                changed = true;
            });
            if changed {
                store.signal_interface_counts_changed();
            }
        }
    }
}

/// A grant lane whose slots and channel live on the leaked heap — the test stand-in
/// for the `StaticCell`s firmware parks them in. Public under `std` so interface-crate
/// tests can drive a seam by hand; a real host never links it.
#[cfg(any(test, feature = "std"))]
pub fn leaked_grant_lane<const SLOT: usize>(
    depth: usize,
) -> (
    EmbassyGrantProducer<'static, embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, SLOT>,
    EmbassyGrantConsumer<'static, embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, SLOT>,
) {
    let slots: std::vec::Vec<FrameSlot<SLOT>> = (0..depth).map(|_| FrameSlot::empty()).collect();
    let channel = std::boxed::Box::leak(std::boxed::Box::new(zerocopy_channel::Channel::new(
        std::boxed::Box::leak(slots.into_boxed_slice()),
    )));
    embassy_grant_lane(channel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        bytes_from_hex, pin_transport_id, TestStorageLayout, RNS_1_3_5_ANNOUNCE, TEST_TRANSPORT_ID,
    };
    use crate::interfaces::{
        AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability,
        InterfaceCapabilities, InterfaceMode, TransportCapability,
    };
    use crate::reactor::interface_seam::Interface;
    use crate::wire::{PacketType, WirePacketHeader};

    use embassy_futures::block_on;
    use embassy_futures::select::{select, select4, Either, Either4};
    use embassy_futures::yield_now;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use embassy_time::with_timeout;

    use std::cell::RefCell;
    use std::rc::Rc;

    const WATCHDOG: Duration = Duration::from_secs(5);

    fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            bitrate: BitrateBps::guess(1_000_000_000),
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
        }
    }

    #[test]
    fn packet_phy_retention_reuses_the_classified_packet_hash() {
        const PACKET_PHY_CAPACITY: usize = 8;
        const PACKET_PHY_INDEX_BUCKETS: usize =
            crate::routing::dedup::dedup_index_buckets(PACKET_PHY_CAPACITY);

        let store = EmbassyInterfaceStore::<
            CriticalSectionRawMutex,
            8,
            PACKET_PHY_CAPACITY,
            PACKET_PHY_INDEX_BUCKETS,
        >::new();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let expected = crate::routing::dedup::PacketHash::of_wire_packet(&raw)
            .expect("the fixture is a wire packet");
        let packet = ClassifiedInboundPacket::classify(InboundPacket {
            arrived_at: InstantMillis(7),
            source_interface: InterfaceId::new([0xC7; 8]),
            bytes: &mut raw,
        });
        let packet_phy = PacketPhyStats {
            rssi: Some(crate::interfaces::RssiDbm::new(-103)),
            snr: Some(crate::interfaces::SnrQuarterDb::new(-11)),
            quality: crate::interfaces::SignalQualityTenthsPercent::new(731),
        };

        retain_packet_phy(&store, &packet, packet_phy);

        assert_eq!(packet.packet_hash(), Some(expected));
        assert_eq!(store.packet_phy(expected), Some(packet_phy));
    }

    #[test]
    fn packet_phy_crosses_the_embassy_ingress_seam_with_its_frame() {
        const SLOT: usize = 64;

        let interface = InterfaceId::new([0xA1; 8]);
        let (inbound, mut reactor_inbound) = leaked_grant_lane::<SLOT>(1);
        let (_reactor_outbound, outbound) = leaked_grant_lane::<SLOT>(1);
        let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
        let packet_phy = PacketPhyStats {
            rssi: Some(crate::interfaces::RssiDbm::new(-87)),
            snr: Some(crate::interfaces::SnrQuarterDb::new(-9)),
            quality: crate::interfaces::SignalQualityTenthsPercent::new(875),
        };
        let mut seam = EmbassyInterfaceSeam::new(interface, inbound, notify.sender(), outbound);

        block_on(seam.next_inbound_with_phy(b"observed", packet_phy));

        let retained = reactor_inbound
            .try_peek()
            .expect("the committed frame reaches the reactor lane");
        assert_eq!(
            (retained.frame(), retained.packet_phy),
            (b"observed".as_slice(), packet_phy)
        );
        assert_eq!(notify.receiver().try_receive(), Ok(interface));

        reactor_inbound.release();
        block_on(seam.next_inbound(b"plain"));

        let retained = reactor_inbound
            .try_peek()
            .expect("the next committed frame reaches the reactor lane");
        assert_eq!(
            (retained.frame(), retained.packet_phy),
            (b"plain".as_slice(), PacketPhyStats::default())
        );
    }

    /// An interface whose "wire" is two grant lanes: the test fills `wire_in` (the medium
    /// delivering a frame) and drains transmitted frames off `wire_out`. It exercises the
    /// [`InterfaceSeam`] in isolation on the no_std host — no real I/O, just the boundary.
    struct EmbassyLoopbackInterface<'a, M: RawMutex, const SLOT: usize> {
        descriptor: InterfaceDescriptor,
        wire_in: EmbassyGrantConsumer<'a, M, SLOT>,
        wire_out: EmbassyGrantProducer<'a, M, SLOT>,
    }

    impl<M: RawMutex, const SLOT: usize> Interface for EmbassyLoopbackInterface<'_, M, SLOT> {
        const HW_MTU: usize = crate::wire::BROADCAST_MTU;
        const KIND: crate::interfaces::InterfaceKind = crate::interfaces::InterfaceKind::Loopback;

        fn descriptor(&self) -> InterfaceDescriptor {
            self.descriptor
        }

        fn channel_tag(&self) -> &[u8] {
            self.descriptor.id.as_bytes()
        }

        async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
            let id = self.descriptor.id;
            let mut wire_in = self.wire_in;
            let mut wire_out = self.wire_out;
            loop {
                match select(wire_in.peek(), seam.next_outbound()).await {
                    Either::First(slot) => {
                        seam.next_inbound(slot.frame()).await;
                        wire_in.release();
                    }
                    Either::Second(out) => {
                        wire_out.grant().await.fill_for(id, out);
                        wire_out.commit();
                    }
                }
            }
        }
    }

    #[test]
    fn an_ifac_frame_crosses_the_seam_and_leaves_masked_through_the_peer() {
        use crate::interfaces::ifac::{IfacContext, IfacSize};

        let source = InterfaceId::new([0xA1; 8]);
        let peer = InterfaceId::new([0xB2; 8]);
        let interfaces = [descriptor(source), descriptor(peer)];
        let network =
            || IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::NARROW).unwrap();
        let ifacs = [
            InterfaceIfac {
                id: source,
                context: network(),
            },
            InterfaceIfac {
                id: peer,
                context: network(),
            },
        ];

        let mut engine = EngineState::<TestStorageLayout>::default();
        pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

        // One notify funnel shared by both interfaces, plus a command lane.
        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 4> = Channel::new();
        let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 2> = Channel::new();

        // Each interface's "wire" (the medium), grant lanes like everything else.
        let (mut source_wire_in_tx, source_wire_in_rx) =
            leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
        let (source_wire_out_tx, _source_wire_out_rx) =
            leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
        let (_peer_wire_in_tx, peer_wire_in_rx) =
            leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
        let (peer_wire_out_tx, mut peer_wire_out_rx) =
            leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);

        // The grant lanes are deliberately sized apart: erasure carries both
        // through one reactor, each paying only its own slot size.
        const SOURCE_SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
        const PEER_SLOT: usize = 256;
        let (source_in_tx, mut source_in_rx) = leaked_grant_lane::<SOURCE_SLOT>(2);
        let (mut source_out_tx, source_out_rx) = leaked_grant_lane::<SOURCE_SLOT>(2);
        let (peer_in_tx, mut peer_in_rx) = leaked_grant_lane::<PEER_SLOT>(2);
        let (mut peer_out_tx, peer_out_rx) = leaked_grant_lane::<PEER_SLOT>(2);

        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut masked = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
        let masked_len = network().mask_outbound(&raw, &mut masked).unwrap();
        let original_hops = WirePacketHeader::parse(&raw)
            .expect("valid announce wire")
            .0
            .hops;

        let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let heard_sink = heard.clone();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                *heard_sink.borrow_mut() += 1;
            }
            Journaled::Delivered(_)
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::CommandSettled { .. }
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::LinkInterfaceMismatch { .. } => {}
        };

        let outcome = block_on(async {
            let mut egress_lanes: [(InterfaceId, &mut dyn AnyGrantProducer); 2] =
                [(source, &mut source_out_tx), (peer, &mut peer_out_tx)];
            let egress = EmbassyEgress::new(&mut egress_lanes);
            let mut inbound_lanes: [(InterfaceId, &mut dyn AnyGrantConsumer); 2] =
                [(source, &mut source_in_rx), (peer, &mut peer_in_rx)];

            let reactor = run(
                engine,
                EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
                ReactorWiring {
                    interfaces: AttachedInterfaces::new(&interfaces),
                    ifacs: &ifacs,
                    notify: notify.receiver(),
                    inbound_lanes: &mut inbound_lanes,
                    commands: commands.receiver(),
                    egress,
                },
                app,
            );

            // The source interface: the test plays the announce onto its wire; its seam
            // fills the frame into its own lane and announces it on the notify funnel.
            let source_seam =
                EmbassyInterfaceSeam::new(source, source_in_tx, notify.sender(), source_out_rx);
            let source_iface = EmbassyLoopbackInterface {
                descriptor: descriptor(source),
                wire_in: source_wire_in_rx,
                wire_out: source_wire_out_tx,
            };
            let source_run = source_iface.run(source_seam);

            // The peer interface: the rebroadcast must leave through *its* wire.
            let peer_seam =
                EmbassyInterfaceSeam::new(peer, peer_in_tx, notify.sender(), peer_out_rx);
            let peer_iface = EmbassyLoopbackInterface {
                descriptor: descriptor(peer),
                wire_in: peer_wire_in_rx,
                wire_out: peer_wire_out_tx,
            };
            let peer_run = peer_iface.run(peer_seam);

            let driver = async {
                // An idle reactor is silent: nothing heard, nothing transmitted by the peer.
                Timer::after(Duration::from_millis(50)).await;
                assert_eq!(*heard.borrow(), 0, "an idle reactor journals nothing");
                assert!(
                    peer_wire_out_rx.try_peek().is_none(),
                    "an idle interface transmits nothing"
                );

                // Play the announce onto the source interface's wire — it crosses the seam.
                source_wire_in_tx
                    .grant()
                    .await
                    .fill_for(source, &masked[..masked_len]);
                source_wire_in_tx.commit();

                loop {
                    if *heard.borrow() >= 1 {
                        if let Some(slot) = peer_wire_out_rx.try_peek() {
                            let rebroadcast = slot.frame().to_vec();
                            peer_wire_out_rx.release();
                            break rebroadcast;
                        }
                    }
                    yield_now().await;
                }
            };

            match select4(
                reactor,
                source_run,
                peer_run,
                with_timeout(WATCHDOG, driver),
            )
            .await
            {
                Either4::Fourth(result) => {
                    result.expect("the rebroadcast fires before the watchdog")
                }
                _ => unreachable!("the reactor and interface loops never return"),
            }
        });

        assert_eq!(outcome[0] & 0x80, 0x80);
        let mut opened = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
        let opened_len = network().unmask_inbound(&outcome, &mut opened).unwrap();
        let (header, _) =
            WirePacketHeader::parse(&opened[..opened_len]).expect("valid rebroadcast wire");
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.hops,
            original_hops + 1,
            "the rebroadcast bumps the hop count"
        );
    }

    #[test]
    fn a_pooled_ifac_slot_added_at_runtime_opens_inbound_then_frees_on_remove() {
        use crate::interfaces::ifac::{IfacContext, IfacSize};

        let source = InterfaceId::new([0xA1; 8]);
        let network =
            IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::NARROW).unwrap();

        let mut engine = EngineState::<TestStorageLayout>::default();
        pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 4> = Channel::new();
        let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 2> = Channel::new();
        let lifecycle: Channel<CriticalSectionRawMutex, InterfaceLifecycle, 2> = Channel::new();

        const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
        // The reactor side of one standing lane keyed by `source`, plus the interface-side producer
        // the test drives frames in on. The interface's descriptor is registered mid-run.
        let (mut source_in_tx, source_in_rx) = leaked_grant_lane::<SLOT>(2);
        let (source_out_tx, _source_out_rx) = leaked_grant_lane::<SLOT>(2);

        let mut inbound: HeaplessVec<
            (
                InterfaceId,
                EmbassyGrantConsumer<'static, CriticalSectionRawMutex, SLOT>,
            ),
            1,
        > = HeaplessVec::new();
        let _ = inbound.push((source, source_in_rx));
        let mut egress_lanes: HeaplessVec<
            (
                InterfaceId,
                EmbassyGrantProducer<'static, CriticalSectionRawMutex, SLOT>,
            ),
            1,
        > = HeaplessVec::new();
        let _ = egress_lanes.push((source, source_out_tx));

        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut masked = [0u8; SLOT];
        let masked_len = network.mask_outbound(&raw, &mut masked).unwrap();

        let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let heard_sink = heard.clone();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                *heard_sink.borrow_mut() += 1;
            }
            Journaled::Delivered(_)
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::CommandSettled { .. }
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::LinkInterfaceMismatch { .. } => {}
        };

        let mut egress = PooledEgress::new(egress_lanes);
        let mut host = EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0));
        let count = block_on(async {
            let initial: HeaplessVec<InterfaceDescriptor, 1> = HeaplessVec::new();
            let mut ifacs: HeaplessVec<InterfaceIfac, 1> = HeaplessVec::new();
            let _ = ifacs.push(InterfaceIfac {
                id: source,
                context: network,
            });
            let reactor = run_pooled(
                &mut engine,
                &mut host,
                PooledWiring {
                    initial: &initial,
                    inbound: &mut inbound,
                    egress: &mut egress,
                    notify: notify.receiver(),
                    commands: commands.receiver(),
                    lifecycle: lifecycle.receiver(),
                    ifacs: &mut ifacs,
                },
                app,
                crate::reactor::decline_all(),
                &NoInterfaceInspectionStore,
            );

            let driver = async {
                // Register the interface at runtime, then announce on its lane: it crosses and the
                // engine learns a route via it.
                lifecycle
                    .sender()
                    .send(InterfaceLifecycle::Add {
                        descriptor: descriptor(source),
                    })
                    .await;
                Timer::after(Duration::from_millis(30)).await;
                source_in_tx
                    .grant()
                    .await
                    .fill_for(source, &masked[..masked_len]);
                source_in_tx.commit();
                notify.sender().send(source).await;
                loop {
                    if *heard.borrow() >= 1 {
                        break;
                    }
                    yield_now().await;
                }

                // Drop it: the cull runs against the reduced descriptor set (the route via this
                // interface goes) and the reactor keeps looping, its lane endpoints untouched.
                lifecycle
                    .sender()
                    .send(InterfaceLifecycle::Remove { id: source })
                    .await;
                Timer::after(Duration::from_millis(30)).await;
                *heard.borrow()
            };

            match select(reactor, with_timeout(WATCHDOG, driver)).await {
                Either::Second(result) => result.expect("the slot is heard before the watchdog"),
                Either::First(()) => unreachable!("the reactor loop never returns"),
            }
        });

        assert_eq!(
            count, 1,
            "the runtime-added slot carried exactly the one announce"
        );
    }

    #[test]
    fn pooled_egress_retag_relabels_a_lane_and_ignores_a_missing_id() {
        let old_id = InterfaceId::new([0x11; 8]);
        let new_id = InterfaceId::new([0x22; 8]);
        const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
        let (producer, _consumer) = leaked_grant_lane::<SLOT>(2);
        let mut lanes: HeaplessVec<
            (
                InterfaceId,
                EmbassyGrantProducer<'static, CriticalSectionRawMutex, SLOT>,
            ),
            1,
        > = HeaplessVec::new();
        let _ = lanes.push((old_id, producer));
        let mut egress = PooledEgress::new(lanes);

        egress.retag(old_id, new_id);
        assert_eq!(egress.lanes[0].0, new_id, "the lane carries the new id");
        egress.retag(old_id, new_id);
        assert_eq!(egress.lanes[0].0, new_id, "retagging a gone id is a no-op");
    }

    #[test]
    fn a_fleet_lane_masks_direct_and_broadcast_frames_once() {
        use crate::interfaces::ifac::{IfacContext, IfacSize};

        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"private-fleet");
        let child = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"peer");
        const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
        let (producer, mut consumer) = leaked_grant_lane::<SLOT>(2);
        let mut lanes: HeaplessVec<
            (
                InterfaceId,
                EmbassyGrantProducer<'static, CriticalSectionRawMutex, SLOT>,
            ),
            1,
        > = HeaplessVec::new();
        let _ = lanes.push((supervisor, producer));
        let mut egress = PooledEgress::new(lanes);
        let network =
            IfacContext::derive(Some("fleet-net"), Some("secret"), IfacSize::NARROW).unwrap();
        let ifacs = [InterfaceIfac {
            id: supervisor,
            context: network.clone(),
        }];
        let clean = bytes_from_hex(RNS_1_3_5_ANNOUNCE);

        enqueue_for_wire(&mut egress, &ifacs, child, &clean);
        let direct = consumer.try_peek().unwrap();
        assert_eq!(direct.target, FrameTarget::Direct(child));
        let mut opened = [0u8; SLOT];
        let opened_len = network.unmask_inbound(direct.frame(), &mut opened).unwrap();
        assert_eq!(&opened[..opened_len], clean.as_slice());
        consumer.release();

        enqueue_broadcast_for_wire(
            &mut egress,
            &ifacs,
            InterfaceKind::AutoWifi,
            FanTarget::All,
            &clean,
        );
        let broadcast = consumer.try_peek().unwrap();
        assert_eq!(broadcast.target, FrameTarget::Fan(FanTarget::All));
        let opened_len = network
            .unmask_inbound(broadcast.frame(), &mut opened)
            .unwrap();
        assert_eq!(&opened[..opened_len], clean.as_slice());
        consumer.release();
    }

    #[test]
    fn a_pooled_slot_retagged_at_runtime_carries_traffic_under_the_new_id() {
        let old_id = InterfaceId::new([0xA1; 8]);
        let new_id = InterfaceId::new([0xB2; 8]);

        let mut engine = EngineState::<TestStorageLayout>::default();
        pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 4> = Channel::new();
        let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 2> = Channel::new();
        let lifecycle: Channel<CriticalSectionRawMutex, InterfaceLifecycle, 2> = Channel::new();

        const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
        let (mut source_in_tx, source_in_rx) = leaked_grant_lane::<SLOT>(2);
        let (source_out_tx, _source_out_rx) = leaked_grant_lane::<SLOT>(2);

        let mut inbound: HeaplessVec<
            (
                InterfaceId,
                EmbassyGrantConsumer<'static, CriticalSectionRawMutex, SLOT>,
            ),
            1,
        > = HeaplessVec::new();
        let _ = inbound.push((old_id, source_in_rx));
        let mut egress_lanes: HeaplessVec<
            (
                InterfaceId,
                EmbassyGrantProducer<'static, CriticalSectionRawMutex, SLOT>,
            ),
            1,
        > = HeaplessVec::new();
        let _ = egress_lanes.push((old_id, source_out_tx));

        let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);

        let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let heard_sink = heard.clone();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                *heard_sink.borrow_mut() += 1;
            }
            Journaled::Delivered(_)
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::CommandSettled { .. }
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::LinkInterfaceMismatch { .. } => {}
        };

        let mut egress = PooledEgress::new(egress_lanes);
        let mut host = EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0));
        let count = block_on(async {
            let initial: HeaplessVec<InterfaceDescriptor, 1> = HeaplessVec::new();
            let mut ifacs: HeaplessVec<InterfaceIfac, 1> = HeaplessVec::new();
            let reactor = run_pooled(
                &mut engine,
                &mut host,
                PooledWiring {
                    initial: &initial,
                    inbound: &mut inbound,
                    egress: &mut egress,
                    notify: notify.receiver(),
                    commands: commands.receiver(),
                    lifecycle: lifecycle.receiver(),
                    ifacs: &mut ifacs,
                },
                app,
                crate::reactor::decline_all(),
                &NoInterfaceInspectionStore,
            );

            let driver = async {
                lifecycle
                    .sender()
                    .send(InterfaceLifecycle::Add {
                        descriptor: descriptor(old_id),
                    })
                    .await;
                Timer::after(Duration::from_millis(30)).await;
                // Retag the live interface onto a new channel id; its lane endpoints never move.
                lifecycle
                    .sender()
                    .send(InterfaceLifecycle::Retag {
                        old_id,
                        new_id,
                        descriptor: descriptor(new_id),
                    })
                    .await;
                Timer::after(Duration::from_millis(30)).await;
                // The announce crosses only if the inbound lane and the descriptor now carry new_id.
                source_in_tx.grant().await.fill_for(new_id, &raw);
                source_in_tx.commit();
                notify.sender().send(new_id).await;
                loop {
                    if *heard.borrow() >= 1 {
                        break;
                    }
                    yield_now().await;
                }
                *heard.borrow()
            };

            match select(reactor, with_timeout(WATCHDOG, driver)).await {
                Either::Second(result) => {
                    result.expect("the retagged slot is heard before the watchdog")
                }
                Either::First(()) => unreachable!("the reactor loop never returns"),
            }
        });

        assert_eq!(
            count, 1,
            "the retagged slot carried the announce under its new channel id"
        );
    }
}
