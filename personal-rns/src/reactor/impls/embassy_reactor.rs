//! The embassy driver: the same reactor as `tokio_reactor`, proven
//! against the no_std host. It races the same three inputs and runs the same engine
//! sink-methods through the same [`fire_due_lane`]/[`wait_for_due_lane`] the tokio driver
//! uses — only the channel and select primitives differ (`embassy_sync` + `embassy_futures`
//! for `tokio::sync` + `tokio::select!`). The interface boundary is identical too: the same
//! [`InterfaceSeam`] contract, an [`EmbassyInterfaceSeam`] supplying the no_std channels behind it.
//! That the dispatch *and* the seam are shared is the point: the sync core's shape holds
//! across std and no_std.

use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_time::{Duration, Timer};
use heapless::Vec as HeaplessVec;

use portable_atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled,
};
use crate::interfaces::substrate::EmbassyTimebase;
use crate::interfaces::{
    AirtimeUtilization, ConnectionState, InboundPacket, InterfaceConfig, InterfaceId,
    InterfaceStatus,
};
use crate::reactor::announce_pacer::{AnnouncePacer, FixedPacerQueue};
use crate::reactor::driver::{
    draw_jitter, fire_due_lane, merge_wake_schedules_delta, wait_for_due_lane, wait_for_pacer,
};
use crate::reactor::interface_seam::{InboundFrame, InterfaceSeam, OutboundFrame};
use crate::reactor::Host;
use crate::routing::storage::EngineStorage;

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
}

const AIRTIME_UNPUBLISHED: u32 = u32::MAX;

impl EmbassyInterfaceStatus {
    #[must_use]
    pub const fn new(id: InterfaceId, connection: ConnectionState) -> Self {
        Self {
            id,
            connection: AtomicU8::new(connection.as_u8()),
            rx: AtomicU64::new(0),
            tx: AtomicU64::new(0),
            airtime: AtomicU32::new(AIRTIME_UNPUBLISHED),
        }
    }

    pub fn set_connection(&self, connection: ConnectionState) {
        self.connection.store(connection.as_u8(), Ordering::Relaxed);
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
}

/// The embassy side of one interface's seam: `next_inbound` frames funnel into the reactor's one
/// inbound stream (a bounded channel, so `next_inbound` backpressures the wire when the reactor
/// is behind), and `next_outbound` parks on this interface's own outbound channel until the
/// reactor enqueues a frame for it.
pub struct EmbassyInterfaceSeam<'a, M: RawMutex, const INBOUND: usize, const OUT: usize> {
    id: InterfaceId,
    inbound: Sender<'a, M, InboundFrame, INBOUND>,
    outbound: Receiver<'a, M, OutboundFrame, OUT>,
}

impl<'a, M: RawMutex, const INBOUND: usize, const OUT: usize>
    EmbassyInterfaceSeam<'a, M, INBOUND, OUT>
{
    #[must_use]
    pub fn new(
        id: InterfaceId,
        inbound: Sender<'a, M, InboundFrame, INBOUND>,
        outbound: Receiver<'a, M, OutboundFrame, OUT>,
    ) -> Self {
        Self {
            id,
            inbound,
            outbound,
        }
    }
}

impl<M: RawMutex, const INBOUND: usize, const OUT: usize> InterfaceSeam
    for EmbassyInterfaceSeam<'_, M, INBOUND, OUT>
{
    async fn next_inbound(&mut self, frame: &[u8]) {
        self.inbound.send(InboundFrame::new(self.id, frame)).await;
    }

    async fn next_outbound(&mut self) -> OutboundFrame {
        self.outbound.receive().await
    }
}

/// The reactor's egress: it routes each engine `Directive::Send` to the target interface's
/// outbound channel. The senders are `Copy` borrows of the channels, so `enqueue` is `&self`
/// and the copy-into-frame is synchronous (`try_send`) — exactly what the lent-buffer borrow
/// demands. No alloc: the lanes are a borrowed slice the caller owns.
pub struct EmbassyEgress<'a, M: RawMutex, const OUT: usize> {
    lanes: &'a [(InterfaceId, Sender<'a, M, OutboundFrame, OUT>)],
}

impl<'a, M: RawMutex, const OUT: usize> EmbassyEgress<'a, M, OUT> {
    #[must_use]
    pub fn new(lanes: &'a [(InterfaceId, Sender<'a, M, OutboundFrame, OUT>)]) -> Self {
        Self { lanes }
    }

    fn enqueue(&self, target: InterfaceId, bytes: &[u8]) {
        for (id, sender) in self.lanes {
            if *id == target {
                let _ = sender.try_send(OutboundFrame::new(bytes));
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
pub async fn run<S, H, M, const INBOUND: usize, const COMMANDS: usize, const OUT: usize>(
    mut engine: EngineState<S>,
    interfaces: &[InterfaceConfig],
    mut host: H,
    inbound: Receiver<'_, M, InboundFrame, INBOUND>,
    commands: Receiver<'_, M, IssuedCommand, COMMANDS>,
    egress: EmbassyEgress<'_, M, OUT>,
    mut app: impl FnMut(Journaled<'_>),
) where
    S: EngineStorage,
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
            inbound.receive(),
            commands.receive(),
            wait_for_due_lane(&host, wake),
            wait_for_pacer(&host, pacer_wake),
        )
        .await
        {
            Either4::First(mut frame) => {
                let now = host.now();
                let jitter = draw_jitter(&mut host);
                let packet = InboundPacket {
                    arrived_at: now,
                    source_interface: frame.source,
                    bytes: &mut frame.bytes[..frame.len],
                };
                let delta = engine.ingest_packet_into(
                    packet,
                    jitter,
                    interfaces,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut |reaction| route_reaction(reaction, &egress, &mut pacers, now, &mut app),
                );
                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
            }
            Either4::Second(issued) => {
                let now = host.now();
                let delta = engine.ingest_command_into(
                    issued,
                    interfaces,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut |reaction| route_reaction(reaction, &egress, &mut pacers, now, &mut app),
                );
                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
            }
            Either4::Third(lane) => {
                let now = host.now();
                let delta = fire_due_lane(&mut engine, lane, now, interfaces, &mut |reaction| {
                    route_reaction(reaction, &egress, &mut pacers, now, &mut app)
                });
                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
            }
            Either4::Fourth(()) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &egress);
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

fn route_reaction<M: RawMutex, const OUT: usize>(
    reaction: EngineReaction<'_>,
    egress: &EmbassyEgress<'_, M, OUT>,
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    app: &mut impl FnMut(Journaled<'_>),
) {
    match reaction {
        EngineReaction::Directive(Directive::Send { target, bytes }) => {
            egress.enqueue(target, bytes);
        }
        EngineReaction::Directive(Directive::SendAnnounce {
            target,
            bytes,
            hops,
        }) => {
            offer_to_pacer(pacers, target, bytes, hops, now, egress);
        }
        EngineReaction::Journaled(journaled) => app(journaled),
    }
}

fn offer_to_pacer<M: RawMutex, const OUT: usize>(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    bytes: &[u8],
    hops: u8,
    now: InstantMillis,
    egress: &EmbassyEgress<'_, M, OUT>,
) {
    match pacers.iter_mut().find(|entry| entry.id == target) {
        Some(entry) => entry
            .pacer
            .offer(bytes, hops, now, |frame| egress.enqueue(target, frame)),
        None => egress.enqueue(target, bytes),
    }
}

fn flush_due_pacers<M: RawMutex, const OUT: usize>(
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    egress: &EmbassyEgress<'_, M, OUT>,
) {
    for entry in pacers.iter_mut() {
        let target = entry.id;
        entry
            .pacer
            .release_due(now, |frame| egress.enqueue(target, frame));
    }
}

fn soonest_pacer_release(pacers: &[InterfacePacer]) -> Option<InstantMillis> {
    pacers
        .iter()
        .filter_map(|entry| entry.pacer.next_release())
        .min_by_key(|deadline| deadline.0)
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
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        }
    }

    /// An interface whose "wire" is two embassy channels: the test plays received frames
    /// onto `wire_in` (the medium delivering one) and reads transmitted frames off
    /// `wire_out`. It exercises the [`InterfaceSeam`] in isolation on the no_std host — no
    /// real I/O, just the boundary.
    struct EmbassyLoopbackInterface<'a, M: RawMutex, const IN: usize, const OUT: usize> {
        descriptor: InterfaceConfig,
        wire_in: Receiver<'a, M, OutboundFrame, IN>,
        wire_out: Sender<'a, M, OutboundFrame, OUT>,
    }

    impl<M: RawMutex, const IN: usize, const OUT: usize> Interface
        for EmbassyLoopbackInterface<'_, M, IN, OUT>
    {
        fn descriptor(&self) -> InterfaceConfig {
            self.descriptor
        }

        async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
            loop {
                match select(self.wire_in.receive(), seam.next_outbound()).await {
                    Either::First(frame) => seam.next_inbound(frame.bytes()).await,
                    Either::Second(out) => self.wire_out.send(out).await,
                }
            }
        }
    }

    #[test]
    fn a_loopback_frame_crosses_the_seam_and_the_rebroadcast_leaves_through_the_peer() {
        let source = InterfaceId::new([0xA1; 16]);
        let peer = InterfaceId::new([0xB2; 16]);
        let view = [descriptor(source), descriptor(peer)];

        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);

        // One inbound funnel shared by both interfaces, plus a command lane.
        let funnel: Channel<CriticalSectionRawMutex, InboundFrame, 2> = Channel::new();
        let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 2> = Channel::new();

        // Each interface's "wire" (the medium) and its reactor-facing outbound queue.
        let source_wire_in: Channel<CriticalSectionRawMutex, OutboundFrame, 2> = Channel::new();
        let source_wire_out: Channel<CriticalSectionRawMutex, OutboundFrame, 2> = Channel::new();
        let source_out: Channel<CriticalSectionRawMutex, OutboundFrame, 2> = Channel::new();
        let peer_wire_in: Channel<CriticalSectionRawMutex, OutboundFrame, 2> = Channel::new();
        let peer_wire_out: Channel<CriticalSectionRawMutex, OutboundFrame, 2> = Channel::new();
        let peer_out: Channel<CriticalSectionRawMutex, OutboundFrame, 2> = Channel::new();

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
            | Journaled::RouteInterfaceGone { .. } => {}
        };

        let outcome = block_on(async {
            let lanes = [(source, source_out.sender()), (peer, peer_out.sender())];
            let egress = EmbassyEgress::new(&lanes);

            let reactor = run(
                engine,
                &view,
                EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
                funnel.receiver(),
                commands.receiver(),
                egress,
                app,
            );

            // The source interface: the test plays the announce onto its wire; its seam
            // funnels the frame into the reactor.
            let source_seam =
                EmbassyInterfaceSeam::new(source, funnel.sender(), source_out.receiver());
            let source_iface = EmbassyLoopbackInterface {
                descriptor: descriptor(source),
                wire_in: source_wire_in.receiver(),
                wire_out: source_wire_out.sender(),
            };
            let source_run = source_iface.run(source_seam);

            // The peer interface: the rebroadcast must leave through *its* wire.
            let peer_seam = EmbassyInterfaceSeam::new(peer, funnel.sender(), peer_out.receiver());
            let peer_iface = EmbassyLoopbackInterface {
                descriptor: descriptor(peer),
                wire_in: peer_wire_in.receiver(),
                wire_out: peer_wire_out.sender(),
            };
            let peer_run = peer_iface.run(peer_seam);

            let driver = async {
                // An idle reactor is silent: nothing heard, nothing transmitted by the peer.
                Timer::after(Duration::from_millis(50)).await;
                assert_eq!(*heard.borrow(), 0, "an idle reactor journals nothing");
                assert!(
                    peer_wire_out.try_receive().is_err(),
                    "an idle interface transmits nothing"
                );

                // Play the announce onto the source interface's wire — it crosses the seam.
                source_wire_in.send(OutboundFrame::new(&raw)).await;

                loop {
                    if *heard.borrow() >= 1 {
                        if let Ok(frame) = peer_wire_out.try_receive() {
                            break frame;
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

        let (header, _) = WirePacketHeader::parse(outcome.bytes()).expect("valid rebroadcast wire");
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.hops,
            original_hops + 1,
            "the rebroadcast bumps the hop count"
        );
    }
}
