//! The embassy driver: the same reactor as `tokio_reactor`, proven
//! against the no_std host. It races the same three inputs and runs the same engine
//! sink-methods through the same [`fire_due_lane`]/[`wait_for_due_lane`] the tokio driver
//! uses — only the channel and select primitives differ (`embassy_sync` + `embassy_futures`
//! for `tokio::sync` + `tokio::select!`). The interface boundary is identical too: the same
//! [`InterfaceSeam`] contract, an [`EmbassyInterfaceSeam`] supplying the no_std channels behind it.
//! That the dispatch *and* the seam are shared is the point: the sync core's shape holds
//! across std and no_std.

use embassy_futures::select::{select4, select5, Either4, Either5};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::zerocopy_channel;
use embassy_time::{Duration, Timer};
use heapless::Vec as HeaplessVec;

use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled, ProofRequest,
};
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::substrate::EmbassyTimebase;
use crate::interfaces::{
    AirtimeUtilization, ConnectionState, InboundPacket, InterfaceConfig, InterfaceId,
    InterfaceStatus, TransferRates,
};
use crate::reactor::announce_pacer::{AnnouncePacer, FixedPacerQueue};
use crate::reactor::driver::{
    draw_jitter, fire_due_lane, merge_wake_schedules_delta, wait_for_due_lane, wait_for_pacer,
};
use crate::reactor::grant::{
    AnyGrantConsumer, AnyGrantProducer, FrameSlot, GrantConsumer, GrantProducer,
};
use crate::reactor::interface_seam::{
    InterfaceSeam, EMBEDDED_MAX_LINK_MTU, EMBEDDED_MAX_WIRE_FRAME_LEN,
};
use crate::reactor::Host;
use crate::storage::StorageLayout;

/// A [`Host`] backed by embassy's clock and a caller-supplied entropy source. Mirrors the
/// legacy `EmbassyContractHost`: an [`EmbassyTimebase`] owns the clock and `draw_entropy`
/// is whatever the board hands it (an esp-hal `Rng`, a seeded test fill). The engine never
/// reads either — it asks the host.
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

/// One interface's live state on the no_std host, shared by reference (a `&'static` the firmware
/// parks in a `StaticCell`) rather than an `Arc`: the interface writes it as the wire moves, the
/// display task reads it lock-free through [`InterfaceStatus`] on its own render cadence — the
/// embassy twin of `TokioInterfaceStatus`. The byte counters are true `u64` (a board left running
/// must never wrap a card's numbers); the 32-bit esp32s3 has no native 64-bit atomic, so
/// `portable-atomic` carries them through critical-section.
pub struct EmbassyInterfaceStatus {
    id: InterfaceId,
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
            id,
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
        self.id
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
    fn try_peek_frame(&mut self) -> Option<(InterfaceId, &mut [u8])> {
        let slot = GrantConsumer::try_peek(self)?;
        let id = slot.interface_id;
        Some((id, slot.frame_mut()))
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
    async fn next_inbound(&mut self, frame: &[u8]) {
        self.inbound.grant().await.fill_for(self.id, frame);
        self.inbound.commit();
        self.notify.send(self.id).await;
    }

    async fn next_outbound(&mut self) -> &[u8] {
        self.outbound.release();
        self.outbound.peek().await.frame()
    }
}

/// Whether the lane keyed by `lane_key` carries traffic for `target`. Two ways it can: an exact id
/// match (a dedicated 1:1 lane owns exactly its interface), or `target` being a child of the fleet
/// supervisor `lane_key` names (the supervisor's [`member_kind`](InterfaceKind::member_kind) equals
/// `target`'s kind). The second is how one shared lane serves a whole fleet with no per-child
/// routing entry: a `WifiPeer` frame finds the lane keyed by the `AutoWifi` supervisor by the kind
/// byte alone. With no supervisor lane registered, only the exact match fires, so routing is
/// unchanged.
fn lane_serves(lane_key: InterfaceId, target: InterfaceId) -> bool {
    if lane_key == target {
        return true;
    }
    match (lane_key.kind(), target.kind()) {
        (Some(supervisor), Some(child)) => supervisor.member_kind() == Some(child),
        _ => false,
    }
}

/// The reactor's egress: it routes one engine `Directive::Send` into the target
/// interface's outbound grant lane — granted, filled in place, committed. A full lane drops
/// the frame rather than stalling the reactor. The fixed [`EmbassyEgress`] and the dynamic
/// [`PooledEgress`] both satisfy it, so the reaction-routing helpers stay shared across the
/// fixed-slice `run` and the runtime's [`run_pooled`].
pub trait ReactorEgress {
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]);
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
}

/// Run the reactor loop on an embassy executor until the task is dropped. Each turn parks
/// on the three inputs and runs the one sync engine method the winner names, carrying out
/// every `Directive` through `egress` and pushing every `Journaled` to `app`. `Idle` arms no
/// timer, so the select rests on the two channels and the core truly sleeps — the dormancy
/// an MCU is built for.
#[allow(clippy::too_many_arguments)]
pub async fn run<S, H, M, const NOTIFY: usize, const COMMANDS: usize>(
    engine: EngineState<S>,
    interfaces: &[InterfaceConfig],
    ifacs: &[InterfaceIfac],
    host: H,
    notify: Receiver<'_, M, InterfaceId, NOTIFY>,
    inbound_lanes: &mut [(InterfaceId, &mut dyn AnyGrantConsumer)],
    commands: Receiver<'_, M, IssuedCommand, COMMANDS>,
    egress: EmbassyEgress<'_>,
    on_journaled: impl FnMut(Journaled<'_>),
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex,
{
    run_with_proof_decider(
        engine,
        interfaces,
        ifacs,
        host,
        notify,
        inbound_lanes,
        commands,
        egress,
        on_journaled,
        |_: &ProofRequest| false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_with_proof_decider<S, H, M, const NOTIFY: usize, const COMMANDS: usize>(
    mut engine: EngineState<S>,
    interfaces: &[InterfaceConfig],
    ifacs: &[InterfaceIfac],
    mut host: H,
    notify: Receiver<'_, M, InterfaceId, NOTIFY>,
    inbound_lanes: &mut [(InterfaceId, &mut dyn AnyGrantConsumer)],
    commands: Receiver<'_, M, IssuedCommand, COMMANDS>,
    mut egress: EmbassyEgress<'_>,
    mut on_journaled: impl FnMut(Journaled<'_>),
    mut should_prove: impl FnMut(&ProofRequest) -> bool,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex,
{
    let mut wake_schedules = engine.wake_schedules(interfaces);
    let mut pacers: HeaplessVec<InterfacePacer, MAX_PACED_INTERFACES> = HeaplessVec::new();
    for config in interfaces {
        let _ = pacers.push(InterfacePacer {
            id: config.id,
            pacer: AnnouncePacer::new(config.announce_bandwidth_cap, config.bitrate_bps),
        });
    }
    loop {
        let wake = wake_schedules.soonest(host.now());
        let pacer_wake = soonest_pacer_release(&pacers);

        match select4(
            notify.receive(),
            commands.receive(),
            wait_for_due_lane(&host, wake),
            wait_for_pacer(&host, pacer_wake),
        )
        .await
        {
            Either4::First(source) => {
                let Some((_, lane)) = inbound_lanes
                    .iter_mut()
                    .find(|(id, _)| lane_serves(*id, source))
                else {
                    continue;
                };
                let Some((source, frame)) = lane.try_peek_frame() else {
                    continue;
                };
                let mut unmasked = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
                let bytes = match ifac_for(ifacs, source) {
                    Some(entry) => {
                        let Some(clean_len) = entry.context.unmask_inbound(frame, &mut unmasked)
                        else {
                            lane.release_frame();
                            continue;
                        };
                        &mut unmasked[..clean_len]
                    }
                    None => frame,
                };
                let now = host.now();
                let jitter = draw_jitter(&mut host);
                let packet = InboundPacket {
                    arrived_at: now,
                    source_interface: source,
                    bytes,
                };
                let delta = engine.ingest_packet_into(
                    packet,
                    jitter,
                    interfaces,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut should_prove,
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
                lane.release_frame();
                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
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
            Either4::Third(lane) => {
                let now = host.now();
                let delta = fire_due_lane(
                    &mut engine,
                    lane,
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
    match reaction {
        EngineReaction::Directive(Directive::Send { target, bytes }) => {
            enqueue_for_wire(egress, ifacs, target, bytes);
        }
        EngineReaction::Directive(Directive::SendAnnounce {
            target,
            bytes,
            hops,
        }) => {
            offer_to_pacer(pacers, target, bytes, hops, now, egress, ifacs);
        }
        EngineReaction::Directive(Directive::EmitFrame { target, fill }) => {
            emit_for_wire(egress, ifacs, target, fill);
        }
        EngineReaction::Journaled(journaled) => app(journaled),
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
    match ifac_for(ifacs, target) {
        Some(entry) => {
            let mut wire = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
            if let Some(masked_len) = entry.context.mask_outbound(bytes, &mut wire) {
                egress.enqueue(target, &wire[..masked_len]);
            }
        }
        None => egress.enqueue(target, bytes),
    }
}

/// Whether `id` owns one of the reactor's standing lanes outright — an exact-id match, not the
/// kind match a fleet member rides. Only a dedicated-lane owner (USB, TCP, a top-level supervisor's
/// own announces) earns an announce pacer; a fleet member shares its supervisor's lane and so its
/// medium's pacing, never its own ~1 KiB queue. This keeps the pacer pool bounded by lane count, not
/// by member count.
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
        Some(entry) => entry.pacer.offer(bytes, hops, now, |frame| {
            enqueue_for_wire(egress, ifacs, target, frame)
        }),
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

/// The runtime's lever to bring one engine interface up or down between cycles. A supervisor sends
/// `Add` when a peer is confirmed and `Remove` when it drops; the reactor adds or drops the
/// interface's descriptor (and an announce pacer only when the interface owns a dedicated lane —
/// a fleet member shares its supervisor's medium and pacing). No lane is allocated: the interface's
/// frames route to its
/// medium's standing lane by [`lane_serves`] (a fleet member finds its supervisor's lane by the kind
/// byte), so a fleet of members shares one lane and `Add`/`Remove` only touch the cheap descriptor
/// set.
pub enum InterfaceLifecycle {
    Add { config: InterfaceConfig },
    Remove { id: InterfaceId },
}

/// The dynamic egress: a fixed pool of `N` permanently-owned outbound lane endpoints, each tagged
/// with the id of the interface — or fleet supervisor — it serves (set once at boot by
/// [`Prns::activate`](crate::runtime::Prns)). The uniform `SLOT` size (a board's one wire ceiling)
/// lets the pool own concrete endpoints rather than the fixed path's erased `&mut dyn`, and the tag
/// lets [`lane_serves`] route a whole fleet over one lane.
pub struct PooledEgress<M: RawMutex + 'static, const SLOT: usize, const N: usize> {
    lanes: HeaplessVec<(InterfaceId, EmbassyGrantProducer<'static, M, SLOT>), N>,
}

impl<M: RawMutex + 'static, const SLOT: usize, const N: usize> PooledEgress<M, SLOT, N> {
    /// Build the egress over the reactor-side outbound endpoints, each at the slot it shares with
    /// the interface-side half. Each lane is tagged at boot with the id of the interface or fleet
    /// supervisor it serves.
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
}

/// Cap an interface's advertised link MTU to what this reactor's lanes can carry. The reactor owns
/// the frame buffers ([`EMBEDDED_MAX_WIRE_FRAME_LEN`]), so it is the authority on the largest wire
/// any interface on this board can move; an interface that declares a wider medium (the USB device's
/// 8 KiB, a future fat-link tier) has its ceiling clamped here, so link negotiation can never settle
/// above the slot the frame must land in. An interface that declares no MTU keeps its broadcast-floor
/// fallback untouched.
fn clamp_to_embedded_ceiling(mut config: InterfaceConfig) -> InterfaceConfig {
    if let Some(mtu) = config.hardware_mtu {
        config.hardware_mtu = Some(mtu.min(EMBEDDED_MAX_LINK_MTU));
    }
    config
}

/// The runtime's reactor: the same loop as [`run`], over a fixed pool of `N` interface slots whose
/// the live descriptor set changes at runtime through the `lifecycle` lane. `initial` names the
/// interfaces active at boot (the recipe's top-level set); `inbound`/`egress` carry the reactor-side
/// endpoints for the `LANES` standing lanes, each tagged with the interface or fleet supervisor it
/// serves. A supervisor registers a peer by sending [`InterfaceLifecycle::Add`] (its frames route to
/// the supervisor's lane by kind) and drops it with [`InterfaceLifecycle::Remove`]; on remove the
/// reactor culls the gone interface's routes exactly as the host runtime does. Lanes bounded by
/// `LANES`, the descriptor set by `MAX_IFACES`, alloc-free: the endpoints never move, only the
/// active config set changes. Pacers are bounded by `LANES`, not `MAX_IFACES`: one announce queue
/// per standing medium, never one per fleet member (see [`owns_dedicated_lane`]).
#[allow(clippy::too_many_arguments)]
pub async fn run_pooled<
    S,
    H,
    M,
    const SLOT: usize,
    const LANES: usize,
    const MAX_IFACES: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
>(
    engine: &mut EngineState<S>,
    initial: &HeaplessVec<InterfaceConfig, MAX_IFACES>,
    inbound: &mut HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, SLOT>), LANES>,
    egress: &mut PooledEgress<M, SLOT, LANES>,
    host: &mut H,
    notify: Receiver<'_, M, InterfaceId, NOTIFY>,
    commands: Receiver<'_, M, IssuedCommand, COMMANDS>,
    lifecycle: Receiver<'_, M, InterfaceLifecycle, LIFECYCLE>,
    mut on_journaled: impl FnMut(Journaled<'_>),
    mut should_prove: impl FnMut(&ProofRequest) -> bool,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex + 'static,
{
    let ifacs: &[InterfaceIfac] = &[];
    let mut configs: HeaplessVec<InterfaceConfig, MAX_IFACES> = HeaplessVec::new();
    let mut pacers: HeaplessVec<InterfacePacer, LANES> = HeaplessVec::new();
    for config in initial {
        let config = clamp_to_embedded_ceiling(*config);
        let _ = configs.push(config);
        if owns_dedicated_lane(inbound, config.id) {
            let _ = pacers.push(InterfacePacer {
                id: config.id,
                pacer: AnnouncePacer::new(config.announce_bandwidth_cap, config.bitrate_bps),
            });
        }
    }
    let mut wake_schedules = engine.wake_schedules(&configs);
    loop {
        let wake = wake_schedules.soonest(host.now());
        let pacer_wake = soonest_pacer_release(&pacers);

        match select5(
            notify.receive(),
            commands.receive(),
            wait_for_due_lane(&*host, wake),
            wait_for_pacer(&*host, pacer_wake),
            lifecycle.receive(),
        )
        .await
        {
            Either5::First(source) => {
                let Some((_, lane)) = inbound.iter_mut().find(|(id, _)| lane_serves(*id, source))
                else {
                    continue;
                };
                let Some((source, frame)) = lane.try_peek_frame() else {
                    continue;
                };
                let now = host.now();
                let jitter = draw_jitter(&mut *host);
                let packet = InboundPacket {
                    arrived_at: now,
                    source_interface: source,
                    bytes: frame,
                };
                let delta = engine.ingest_packet_into(
                    packet,
                    jitter,
                    &configs,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut should_prove,
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
                lane.release_frame();
                merge_wake_schedules_delta(&mut wake_schedules, delta, &*engine, &configs);
            }
            Either5::Second(issued) => {
                let now = host.now();
                let delta = engine.ingest_command_into(
                    issued,
                    &configs,
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
                merge_wake_schedules_delta(&mut wake_schedules, delta, &*engine, &configs);
            }
            Either5::Third(lane) => {
                let now = host.now();
                let delta = fire_due_lane(
                    &mut *engine,
                    lane,
                    now,
                    &configs,
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
                merge_wake_schedules_delta(&mut wake_schedules, delta, &*engine, &configs);
            }
            Either5::Fourth(()) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &mut *egress, ifacs);
            }
            Either5::Fifth(message) => match message {
                InterfaceLifecycle::Add { config } => {
                    let config = clamp_to_embedded_ceiling(config);
                    let id = config.id;
                    if configs.iter().all(|existing| existing.id != id) {
                        let _ = configs.push(config);
                        if owns_dedicated_lane(inbound, id) {
                            let _ = pacers.push(InterfacePacer {
                                id,
                                pacer: AnnouncePacer::new(
                                    config.announce_bandwidth_cap,
                                    config.bitrate_bps,
                                ),
                            });
                        }
                        wake_schedules = engine.wake_schedules(&configs);
                    }
                }
                InterfaceLifecycle::Remove { id } => {
                    if let Some(pos) = configs.iter().position(|config| config.id == id) {
                        let _ = configs.swap_remove(pos);
                    }
                    if let Some(pos) = pacers.iter().position(|pacer| pacer.id == id) {
                        let _ = pacers.swap_remove(pos);
                    }
                    let now = host.now();
                    engine.cull_expired_routes(now, &configs, &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut *egress,
                            ifacs,
                            &mut pacers,
                            now,
                            &mut on_journaled,
                        )
                    });
                    wake_schedules = engine.wake_schedules(&configs);
                }
            },
        }
    }
}

/// A grant lane whose slots and channel live on the leaked heap — the test stand-in
/// for the `StaticCell`s firmware parks them in.
#[cfg(test)]
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
    use crate::engine::test_support::{hx, Cap, RAW_ANNOUNCE, TEST_TRANSPORT_ID};
    use crate::interfaces::{
        AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
        InterfaceMode, TransportCapability,
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

    fn descriptor(id: InterfaceId) -> InterfaceConfig {
        InterfaceConfig {
            id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            bitrate_bps: None,
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
        }
    }

    /// An interface whose "wire" is two grant lanes: the test fills `wire_in` (the medium
    /// delivering a frame) and drains transmitted frames off `wire_out`. It exercises the
    /// [`InterfaceSeam`] in isolation on the no_std host — no real I/O, just the boundary.
    struct EmbassyLoopbackInterface<'a, M: RawMutex, const SLOT: usize> {
        descriptor: InterfaceConfig,
        wire_in: EmbassyGrantConsumer<'a, M, SLOT>,
        wire_out: EmbassyGrantProducer<'a, M, SLOT>,
    }

    impl<M: RawMutex, const SLOT: usize> Interface for EmbassyLoopbackInterface<'_, M, SLOT> {
        const HW_MTU: usize = crate::wire::BROADCAST_MTU;
        const KIND: crate::interfaces::InterfaceKind = crate::interfaces::InterfaceKind::Loopback;

        fn descriptor(&self) -> InterfaceConfig {
            self.descriptor
        }

        fn reachability_tag(&self) -> &[u8] {
            self.descriptor.id.as_bytes()
        }

        async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
            let mut wire_in = self.wire_in;
            let mut wire_out = self.wire_out;
            loop {
                match select(wire_in.peek(), seam.next_outbound()).await {
                    Either::First(slot) => {
                        seam.next_inbound(slot.frame()).await;
                        wire_in.release();
                    }
                    Either::Second(out) => {
                        wire_out.grant().await.fill(out);
                        wire_out.commit();
                    }
                }
            }
        }
    }

    #[test]
    fn a_loopback_frame_crosses_the_seam_and_the_rebroadcast_leaves_through_the_peer() {
        let source = InterfaceId::new([0xA1; 8]);
        let peer = InterfaceId::new([0xB2; 8]);
        let view = [descriptor(source), descriptor(peer)];

        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);

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

        let raw = hx(RAW_ANNOUNCE);
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
            | Journaled::CommandSettled { .. }
            | Journaled::RouteExpired { .. }
            | Journaled::RouteEvicted { .. }
            | Journaled::RouteInterfaceGone { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. } => {}
        };

        let outcome = block_on(async {
            let mut egress_lanes: [(InterfaceId, &mut dyn AnyGrantProducer); 2] =
                [(source, &mut source_out_tx), (peer, &mut peer_out_tx)];
            let egress = EmbassyEgress::new(&mut egress_lanes);
            let mut inbound_lanes: [(InterfaceId, &mut dyn AnyGrantConsumer); 2] =
                [(source, &mut source_in_rx), (peer, &mut peer_in_rx)];

            let reactor = run(
                engine,
                &view,
                &[],
                EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
                notify.receiver(),
                &mut inbound_lanes,
                commands.receiver(),
                egress,
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
                source_wire_in_tx.grant().await.fill(&raw);
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

        let (header, _) = WirePacketHeader::parse(&outcome).expect("valid rebroadcast wire");
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.hops,
            original_hops + 1,
            "the rebroadcast bumps the hop count"
        );
    }

    #[test]
    fn a_pooled_slot_added_at_runtime_carries_inbound_then_frees_on_remove() {
        let source = InterfaceId::new([0xA1; 8]);

        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);

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

        let raw = hx(RAW_ANNOUNCE);

        let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let heard_sink = heard.clone();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                *heard_sink.borrow_mut() += 1;
            }
            Journaled::Delivered(_)
            | Journaled::CommandSettled { .. }
            | Journaled::RouteExpired { .. }
            | Journaled::RouteEvicted { .. }
            | Journaled::RouteInterfaceGone { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceNeedsDecompression { .. } => {}
        };

        let mut egress = PooledEgress::new(egress_lanes);
        let mut host = EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0));
        let count = block_on(async {
            let initial: HeaplessVec<InterfaceConfig, 1> = HeaplessVec::new();
            let reactor = run_pooled(
                &mut engine,
                &initial,
                &mut inbound,
                &mut egress,
                &mut host,
                notify.receiver(),
                commands.receiver(),
                lifecycle.receiver(),
                app,
                |_: &ProofRequest| false,
            );

            let driver = async {
                // Register the interface at runtime, then announce on its lane: it crosses and the
                // engine learns a route via it.
                lifecycle
                    .sender()
                    .send(InterfaceLifecycle::Add {
                        config: descriptor(source),
                    })
                    .await;
                Timer::after(Duration::from_millis(30)).await;
                source_in_tx.grant().await.fill_for(source, &raw);
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
}
