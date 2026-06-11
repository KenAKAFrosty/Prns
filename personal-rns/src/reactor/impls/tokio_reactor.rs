use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled, ProofRequest,
};
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{
    AirtimeUtilization, ConnectionState, InboundPacket, InterfaceConfig, InterfaceId,
    InterfaceStatus, TransferRates,
};
use crate::reactor::announce_pacer::{AnnouncePacer, HeapPacerQueue};
use crate::reactor::driver::{
    draw_jitter, fire_due_lane, merge_wake_schedules_delta, wait_for_due_lane, wait_for_pacer,
};
use crate::reactor::interface_seam::{
    InboundFrame, InterfaceSeam, OutboundFrame, MAX_WIRE_FRAME_LEN,
};
use crate::reactor::Host;
use crate::routing::storage::EngineStorage;

pub struct TokioHost {
    base: Instant,
}

impl TokioHost {
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
        }
    }
}

impl Default for TokioHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for TokioHost {
    fn now(&self) -> InstantMillis {
        InstantMillis(self.base.elapsed().as_millis() as u64)
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        tokio::time::sleep_until(self.base + Duration::from_millis(deadline.0)).await;
    }

    #[allow(clippy::expect_used)]
    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        getrandom::getrandom(bytes).expect("OS CSPRNG must provide reactor entropy");
    }
}

/// The tokio side of one interface's seam: `next_inbound` frames funnel into the reactor's one
/// inbound stream (tagged with this interface's id), and `next_outbound` parks on this
/// interface's own outbound queue until the reactor enqueues a frame for it.
pub struct TokioInterfaceSeam {
    id: InterfaceId,
    inbound: UnboundedSender<InboundFrame>,
    outbound: UnboundedReceiver<OutboundFrame>,
}

impl TokioInterfaceSeam {
    #[must_use]
    pub fn new(
        id: InterfaceId,
        inbound: UnboundedSender<InboundFrame>,
        outbound: UnboundedReceiver<OutboundFrame>,
    ) -> Self {
        Self {
            id,
            inbound,
            outbound,
        }
    }
}

impl InterfaceSeam for TokioInterfaceSeam {
    async fn next_inbound(&mut self, frame: &[u8]) {
        let _ = self.inbound.send(InboundFrame::new(self.id, frame));
    }

    async fn next_outbound(&mut self) -> OutboundFrame {
        match self.outbound.recv().await {
            Some(frame) => frame,
            // The reactor's egress dropped this interface's lane: there is no more work
            // ever. Park (lifecycle/shutdown is a deferred concern, not the seam's).
            None => core::future::pending().await,
        }
    }
}

/// The reactor's egress: it routes each engine `Directive::Send` to the target
/// interface's outbound queue. The senders are cheap clones, so `enqueue` is `&self`
/// and the copy-into-frame is synchronous — exactly what the lent-buffer borrow demands.
pub struct Egress {
    lanes: std::vec::Vec<(InterfaceId, UnboundedSender<OutboundFrame>)>,
}

impl Egress {
    #[must_use]
    pub fn new(lanes: std::vec::Vec<(InterfaceId, UnboundedSender<OutboundFrame>)>) -> Self {
        Self { lanes }
    }

    fn enqueue(&self, target: InterfaceId, bytes: &[u8]) {
        for (id, sender) in &self.lanes {
            if *id == target {
                let _ = sender.send(OutboundFrame::new(bytes));
                return;
            }
        }
    }
}

/// A cheap-clone handle to one interface's live state: the interface holds a clone and writes
/// it as the wire moves (connection on connect/disconnect, bytes as they cross); the app holds
/// a clone and reads it lock-free via [`InterfaceStatus`] on its own render cadence.
#[derive(Clone)]
pub struct TokioInterfaceStatus {
    inner: Arc<StatusCell>,
}

struct StatusCell {
    id: InterfaceId,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    airtime: AtomicU32,
    transfer_rates: AtomicU64,
}

const AIRTIME_UNPUBLISHED: u32 = u32::MAX;
const RATES_UNPUBLISHED: u64 = u64::MAX;

fn pack_airtime(utilization: AirtimeUtilization) -> u32 {
    (u32::from(utilization.short_per_mille) << 16) | u32::from(utilization.long_per_mille)
}

fn unpack_airtime(packed: u32) -> Option<AirtimeUtilization> {
    if packed == AIRTIME_UNPUBLISHED {
        return None;
    }
    Some(AirtimeUtilization {
        short_per_mille: (packed >> 16) as u16,
        long_per_mille: packed as u16,
    })
}

impl TokioInterfaceStatus {
    #[must_use]
    pub fn new(id: InterfaceId, connection: ConnectionState) -> Self {
        Self {
            inner: Arc::new(StatusCell {
                id,
                connection: AtomicU8::new(connection.as_u8()),
                rx: AtomicU64::new(0),
                tx: AtomicU64::new(0),
                airtime: AtomicU32::new(AIRTIME_UNPUBLISHED),
                transfer_rates: AtomicU64::new(RATES_UNPUBLISHED),
            }),
        }
    }

    pub fn set_connection(&self, connection: ConnectionState) {
        self.inner
            .connection
            .store(connection.as_u8(), Ordering::Relaxed);
    }

    pub fn add_rx(&self, bytes: u64) {
        self.inner.rx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_tx(&self, bytes: u64) {
        self.inner.tx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn set_airtime(&self, utilization: AirtimeUtilization) {
        self.inner
            .airtime
            .store(pack_airtime(utilization), Ordering::Relaxed);
    }

    pub fn set_transfer_rates(&self, rates: TransferRates) {
        let packed = (u64::from(rates.rx_bps) << 32) | u64::from(rates.tx_bps);
        self.inner.transfer_rates.store(packed, Ordering::Relaxed);
    }
}

impl InterfaceStatus for TokioInterfaceStatus {
    fn id(&self) -> InterfaceId {
        self.inner.id
    }

    fn connection(&self) -> ConnectionState {
        ConnectionState::from_u8(self.inner.connection.load(Ordering::Relaxed))
    }

    fn rx_bytes(&self) -> u64 {
        self.inner.rx.load(Ordering::Relaxed)
    }

    fn tx_bytes(&self) -> u64 {
        self.inner.tx.load(Ordering::Relaxed)
    }

    fn airtime(&self) -> Option<AirtimeUtilization> {
        unpack_airtime(self.inner.airtime.load(Ordering::Relaxed))
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        let packed = self.inner.transfer_rates.load(Ordering::Relaxed);
        if packed == RATES_UNPUBLISHED {
            return None;
        }
        Some(TransferRates {
            rx_bps: (packed >> 32) as u32,
            tx_bps: packed as u32,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run<S, H, J>(
    engine: EngineState<S>,
    interfaces: std::vec::Vec<InterfaceConfig>,
    ifacs: std::vec::Vec<InterfaceIfac>,
    host: H,
    inbound: UnboundedReceiver<InboundFrame>,
    commands: UnboundedReceiver<IssuedCommand>,
    egress: Egress,
    on_journaled: J,
) where
    S: EngineStorage,
    H: Host,
    J: FnMut(Journaled<'_>),
{
    run_with_proof_decider(
        engine,
        interfaces,
        ifacs,
        host,
        inbound,
        commands,
        egress,
        on_journaled,
        |_: &ProofRequest| false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_with_proof_decider<S, H, J, P>(
    mut engine: EngineState<S>,
    interfaces: std::vec::Vec<InterfaceConfig>,
    ifacs: std::vec::Vec<InterfaceIfac>,
    mut host: H,
    mut inbound: UnboundedReceiver<InboundFrame>,
    mut commands: UnboundedReceiver<IssuedCommand>,
    egress: Egress,
    mut on_journaled: J,
    mut should_prove: P,
) where
    S: EngineStorage,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
{
    let mut wake_schedules = engine.wake_schedules(&interfaces);
    let mut pacers: std::vec::Vec<InterfacePacer> = interfaces
        .iter()
        .map(|config| InterfacePacer {
            id: config.id,
            pacer: AnnouncePacer::new(config.announce_bandwidth_cap, config.bitrate_bps),
        })
        .collect();
    loop {
        let wake = wake_schedules.soonest(host.now());
        let pacer_wake = soonest_pacer_release(&pacers);
        tokio::select! {
            arrived = inbound.recv() => {
                let Some(mut frame) = arrived else { return };
                let mut unmasked = [0u8; MAX_WIRE_FRAME_LEN];
                let bytes = match ifac_for(&ifacs, frame.source) {
                    Some(entry) => {
                        let Some(clean_len) =
                            entry.context.unmask_inbound(&frame.bytes[..frame.len], &mut unmasked)
                        else {
                            continue;
                        };
                        &mut unmasked[..clean_len]
                    }
                    None => &mut frame.bytes[..frame.len],
                };
                let now = host.now();
                let jitter = draw_jitter(&mut host);
                let packet = InboundPacket {
                    arrived_at: now,
                    source_interface: frame.source,
                    bytes,
                };
                let wake_schedules_delta = engine.ingest_packet_into(
                    packet,
                    jitter,
                    &interfaces,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut should_prove,
                    &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, now, &mut on_journaled),
                );
                merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine, &interfaces);
            }
            issued = commands.recv() => {
                let Some(issued) = issued else { return };
                let now = host.now();
                let wake_schedules_delta = engine.ingest_command_into(
                    issued,
                    &interfaces,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, now, &mut on_journaled),
                );
                merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine, &interfaces);
            }
            lane = wait_for_due_lane(&host, wake) => {
                let now = host.now();
                let wake_schedules_delta = fire_due_lane(
                    &mut engine,
                    lane,
                    now,
                    &interfaces,
                    &mut |bytes| host.fill_entropy(bytes),
                    &mut |reaction| route_reaction(reaction, &egress, &ifacs, &mut pacers, now, &mut on_journaled),
                );
                merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine, &interfaces);
            }
            _ = wait_for_pacer(&host, pacer_wake) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &egress, &ifacs);
            }
        }
    }
}

struct InterfacePacer {
    id: InterfaceId,
    pacer: AnnouncePacer<HeapPacerQueue>,
}

fn route_reaction<A: FnMut(Journaled<'_>)>(
    reaction: EngineReaction<'_>,
    egress: &Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    app: &mut A,
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
        EngineReaction::Journaled(journaled) => app(journaled),
    }
}

fn ifac_for(ifacs: &[InterfaceIfac], id: InterfaceId) -> Option<&InterfaceIfac> {
    if ifacs.is_empty() {
        return None;
    }
    ifacs.iter().find(|entry| entry.id == id)
}

/// The one egress choke: a target with an access code never sees clean bytes on its
/// wire, and a frame the mask refuses (oversize) is dropped rather than leaked open.
fn enqueue_for_wire(egress: &Egress, ifacs: &[InterfaceIfac], target: InterfaceId, bytes: &[u8]) {
    match ifac_for(ifacs, target) {
        Some(entry) => {
            let mut wire = [0u8; MAX_WIRE_FRAME_LEN];
            if let Some(masked_len) = entry.context.mask_outbound(bytes, &mut wire) {
                egress.enqueue(target, &wire[..masked_len]);
            }
        }
        None => egress.enqueue(target, bytes),
    }
}

fn offer_to_pacer(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    bytes: &[u8],
    hops: u8,
    now: InstantMillis,
    egress: &Egress,
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
    egress: &Egress,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        hx, Cap, RATCHETED_ANNOUNCE_RNS_WIRE, RAW_ANNOUNCE, TEST_TRANSPORT_ID,
    };
    use crate::interfaces::{
        AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
        InterfaceMode, TransportCapability,
    };
    use crate::reactor::interface_seam::Interface;
    use crate::wire::{PacketType, WirePacketHeader};
    use tokio::sync::mpsc;

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
            airtime_duty_cycle: None,
        }
    }

    #[test]
    fn airtime_reads_none_until_published_then_round_trips() {
        let status =
            TokioInterfaceStatus::new(InterfaceId::new([0x5A; 16]), ConnectionState::Initializing);
        assert_eq!(status.airtime(), None);

        status.set_airtime(AirtimeUtilization {
            short_per_mille: 137,
            long_per_mille: 4,
        });
        assert_eq!(
            status.airtime(),
            Some(AirtimeUtilization {
                short_per_mille: 137,
                long_per_mille: 4,
            }),
        );
    }

    #[test]
    fn the_pacer_wiring_holds_then_releases_a_capped_burst() {
        let id = InterfaceId::new([0x5a; 16]);
        let mut pacers = std::vec![InterfacePacer {
            id,
            pacer: AnnouncePacer::<HeapPacerQueue>::new(
                AnnounceBandwidthCap::RNS_DEFAULT,
                Some(5_000),
            ),
        }];
        let (tx, mut rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let egress = Egress::new(std::vec![(id, tx)]);

        offer_to_pacer(
            &mut pacers,
            id,
            &[1; 10],
            1,
            InstantMillis(1_000),
            &egress,
            &[],
        );
        assert_eq!(rx.try_recv().unwrap().bytes(), [1u8; 10].as_slice());

        offer_to_pacer(
            &mut pacers,
            id,
            &[2; 10],
            1,
            InstantMillis(1_200),
            &egress,
            &[],
        );
        assert!(rx.try_recv().is_err(), "the second is held, not sent");
        assert_eq!(soonest_pacer_release(&pacers), Some(InstantMillis(1_800)));

        flush_due_pacers(&mut pacers, InstantMillis(1_799), &egress, &[]);
        assert!(rx.try_recv().is_err(), "nothing releases before the window");

        flush_due_pacers(&mut pacers, InstantMillis(1_800), &egress, &[]);
        assert_eq!(rx.try_recv().unwrap().bytes(), [2u8; 10].as_slice());
        assert_eq!(soonest_pacer_release(&pacers), None);
    }

    /// An interface whose "wire" is two in-memory channels: the test plays received
    /// bytes onto `wire_in` (the medium delivering a frame) and reads transmitted bytes
    /// off `wire_out` (the medium sending one). It exercises the [`InterfaceSeam`] in
    /// isolation — no real I/O, just the boundary.
    struct LoopbackInterface {
        descriptor: InterfaceConfig,
        wire_in: UnboundedReceiver<std::vec::Vec<u8>>,
        wire_out: UnboundedSender<std::vec::Vec<u8>>,
    }

    impl Interface for LoopbackInterface {
        fn descriptor(&self) -> InterfaceConfig {
            self.descriptor
        }

        async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
            loop {
                tokio::select! {
                    received = self.wire_in.recv() => {
                        match received {
                            Some(bytes) => seam.next_inbound(&bytes).await,
                            None => return,
                        }
                    }
                    outbound = seam.next_outbound() => {
                        let _ = self.wire_out.send(outbound.bytes().to_vec());
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn a_loopback_frame_crosses_the_seam_and_the_rebroadcast_leaves_through_the_peer() {
        let source = InterfaceId::new([0xA1; 16]);
        let peer = InterfaceId::new([0xB2; 16]);
        let view = std::vec![descriptor(source), descriptor(peer)];

        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);

        // One inbound funnel shared by both interfaces.
        let (funnel_tx, funnel_rx) = mpsc::unbounded_channel::<InboundFrame>();

        // The source interface: the test plays an announce onto its wire; its seam
        // funnels the frame to the reactor.
        let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_wire_out_tx, _source_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_out_tx, source_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let source_iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_seam = TokioInterfaceSeam::new(source, funnel_tx.clone(), source_out_rx);

        // The peer interface: the rebroadcast must leave through *its* wire.
        let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_wire_out_tx, mut peer_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_out_tx, peer_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let peer_iface = LoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_seam = TokioInterfaceSeam::new(peer, funnel_tx.clone(), peer_out_rx);

        drop(funnel_tx);

        let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);

        let (_command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<()>();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                let _ = heard_tx.send(());
            }
            Journaled::Delivered(_)
            | Journaled::CommandSettled { .. }
            | Journaled::RouteExpired { .. }
            | Journaled::RouteEvicted { .. }
            | Journaled::RouteInterfaceGone { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::LinkClosed { .. } => {}
        };

        tokio::spawn(run(
            engine,
            view,
            std::vec![],
            TokioHost::new(),
            funnel_rx,
            command_rx,
            egress,
            app,
        ));
        tokio::spawn(source_iface.run(source_seam));
        tokio::spawn(peer_iface.run(peer_seam));

        // Idle: the loopback wires are quiet, the reactor journals and emits nothing.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            heard_rx.try_recv().is_err(),
            "an idle reactor journals nothing"
        );
        assert!(
            peer_wire_out_rx.try_recv().is_err(),
            "an idle interface transmits nothing"
        );

        let raw = hx(RAW_ANNOUNCE);
        let original_hops = WirePacketHeader::parse(&raw)
            .expect("valid announce wire")
            .0
            .hops;
        // Play the announce onto the source interface's wire — it crosses the seam itself.
        source_wire_in_tx
            .send(raw)
            .expect("the source interface holds its wire");

        tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
            .await
            .expect("the deposited frame journals within the window")
            .expect("the reactor task is alive");

        // The rebroadcast leaves through the peer interface's wire: egress -> peer seam ->
        // next_outbound -> the peer's write.
        let bytes = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
            .await
            .expect("the rebroadcast reaches the peer's wire within the window")
            .expect("the peer interface task is alive");
        let (header, _) = WirePacketHeader::parse(&bytes).expect("valid rebroadcast wire");
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.hops,
            original_hops + 1,
            "the rebroadcast bumps the hop count"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_capped_link_holds_a_rebroadcast_burst_then_drains_it_over_time() {
        let source = InterfaceId::new([0xA1; 16]);
        let peer = InterfaceId::new([0xB2; 16]);
        let slow_peer = InterfaceConfig {
            bitrate_bps: Some(1_000),
            announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
            ..descriptor(peer)
        };
        let view = std::vec![descriptor(source), slow_peer];

        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);

        let (funnel_tx, funnel_rx) = mpsc::unbounded_channel::<InboundFrame>();

        let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_wire_out_tx, _source_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_out_tx, source_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let source_iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_seam = TokioInterfaceSeam::new(source, funnel_tx.clone(), source_out_rx);

        let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_wire_out_tx, mut peer_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_out_tx, peer_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let peer_iface = LoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_seam = TokioInterfaceSeam::new(peer, funnel_tx.clone(), peer_out_rx);

        drop(funnel_tx);

        let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();

        tokio::spawn(run(
            engine,
            view,
            std::vec![],
            TokioHost::new(),
            funnel_rx,
            command_rx,
            egress,
            |_journaled: Journaled<'_>| {},
        ));
        tokio::spawn(source_iface.run(source_seam));
        tokio::spawn(peer_iface.run(peer_seam));

        source_wire_in_tx
            .send(hx(RAW_ANNOUNCE))
            .expect("the source interface holds its wire");
        source_wire_in_tx
            .send(hx(RATCHETED_ANNOUNCE_RNS_WIRE))
            .expect("the source interface holds its wire");

        let first = tokio::time::timeout(Duration::from_secs(5), peer_wire_out_rx.recv())
            .await
            .expect("the first rebroadcast leaves the idle link within the window")
            .expect("the peer task is alive");
        assert_eq!(
            WirePacketHeader::parse(&first).unwrap().0.packet_type,
            PacketType::Announce
        );

        assert!(
            tokio::time::timeout(Duration::from_secs(5), peer_wire_out_rx.recv())
                .await
                .is_err(),
            "the cap holds the second rebroadcast far short of its spacing window",
        );

        let second = tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
            .await
            .expect("the held rebroadcast drains once the spacing window passes")
            .expect("the peer task is alive");
        assert_eq!(
            WirePacketHeader::parse(&second).unwrap().0.packet_type,
            PacketType::Announce
        );
        assert_ne!(first, second, "the two rebroadcasts are distinct announces");
    }

    #[tokio::test(start_paused = true)]
    async fn the_reactor_re_emits_a_rebroadcast_once_more_then_retires_it() {
        let source = InterfaceId::new([0xA1; 16]);
        let peer = InterfaceId::new([0xB2; 16]);
        let view = std::vec![descriptor(source), descriptor(peer)];

        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);

        let (funnel_tx, funnel_rx) = mpsc::unbounded_channel::<InboundFrame>();

        let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_wire_out_tx, _source_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_out_tx, source_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let source_iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_seam = TokioInterfaceSeam::new(source, funnel_tx.clone(), source_out_rx);

        let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_wire_out_tx, mut peer_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_out_tx, peer_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let peer_iface = LoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_seam = TokioInterfaceSeam::new(peer, funnel_tx.clone(), peer_out_rx);

        drop(funnel_tx);

        let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();

        tokio::spawn(run(
            engine,
            view,
            std::vec![],
            TokioHost::new(),
            funnel_rx,
            command_rx,
            egress,
            |_journaled: Journaled<'_>| {},
        ));
        tokio::spawn(source_iface.run(source_seam));
        tokio::spawn(peer_iface.run(peer_seam));

        source_wire_in_tx
            .send(hx(RAW_ANNOUNCE))
            .expect("the source interface holds its wire");

        let first = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
            .await
            .expect("the first emission leaves within the jitter window")
            .expect("the peer task is alive");

        assert!(
            tokio::time::timeout(Duration::from_secs(4), peer_wire_out_rx.recv())
                .await
                .is_err(),
            "the second emission waits the full retransmit interval",
        );

        let second = tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
            .await
            .expect("the reactor re-emits once the retransmit interval passes")
            .expect("the peer task is alive");
        assert_eq!(
            first, second,
            "the retransmit re-emits the same pinned announce, byte for byte",
        );

        assert!(
            tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
                .await
                .is_err(),
            "after two emissions the reactor retires the entry",
        );
    }

    #[tokio::test]
    async fn a_delivery_answers_with_a_proof_directive_on_the_arrival_lane() {
        use crate::crypto::X25519SecretKey;
        use crate::engine::RatchetPolicy;
        use crate::identity::in_memory::InMemoryNodeIdentity;
        use crate::identity::{IdentitySigner, RemoteIdentity, Zeroizing};
        use crate::routing::dedup::PacketHash;
        use crate::routing::proof::IMPLICIT_PROOF_WIRE_LEN;
        use crate::routing::upstream_app_destinations::ProofStrategy;
        use crate::wire::{
            ContextFlag, DestinationType, IfacFlag, PropagationType, WireContext, BROADCAST_MTU,
        };

        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        let secret = Zeroizing::new(secret);

        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
        let mut engine = EngineState::<Cap>::new(secret);
        let destination = engine
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the single destination");

        let remote = RemoteIdentity::from_public_keys(
            identity.encryption_public_key(),
            identity.signing_public_key(),
        );
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination,
            context: WireContext::None,
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut wire).expect("writes the header");
        let sealed = remote
            .encrypt(
                &X25519SecretKey::new([0x77; 32]),
                &[0x88; 16],
                b"prove-through-the-stack",
                &mut wire[header_len..],
            )
            .expect("seals the payload");
        let raw = wire[..header_len + sealed].to_vec();
        let packet_hash = PacketHash::of_wire_packet(&raw).expect("hashes the wire packet");

        let mut expected_proof = std::vec::Vec::new();
        expected_proof.push(0x03);
        expected_proof.push(0x00);
        expected_proof.extend_from_slice(packet_hash.proof_destination().as_bytes());
        expected_proof.push(0x00);
        expected_proof.extend_from_slice(&identity.sign(packet_hash.as_bytes()).0);
        assert_eq!(expected_proof.len(), IMPLICIT_PROOF_WIRE_LEN);

        let source = InterfaceId::new([0xA1; 16]);
        let view = std::vec![descriptor(source)];

        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<InboundFrame>();
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
        let (source_out_tx, mut source_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let egress = Egress::new(std::vec![(source, source_out_tx)]);

        let (delivered_tx, mut delivered_rx) = mpsc::unbounded_channel::<()>();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::Delivered(_) => {
                let _ = delivered_tx.send(());
            }
            Journaled::AnnounceHeard { .. }
            | Journaled::CommandSettled { .. }
            | Journaled::RouteExpired { .. }
            | Journaled::RouteEvicted { .. }
            | Journaled::RouteInterfaceGone { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::LinkClosed { .. } => {}
        };

        tokio::spawn(run(
            engine,
            view,
            std::vec![],
            TokioHost::new(),
            inbound_rx,
            command_rx,
            egress,
            app,
        ));

        inbound_tx
            .send(InboundFrame::new(source, &raw))
            .expect("the reactor task holds the receiver");

        tokio::time::timeout(Duration::from_secs(2), delivered_rx.recv())
            .await
            .expect("the delivery journals within the window")
            .expect("the reactor task is alive");

        let frame = tokio::time::timeout(Duration::from_secs(2), source_out_rx.recv())
            .await
            .expect("the owed proof is emitted within the window")
            .expect("the reactor task is alive");
        assert_eq!(
            frame.bytes(),
            expected_proof,
            "the proof is byte-identical to the RNS 1.3.1 implicit proof, on the arrival lane"
        );
    }

    #[tokio::test]
    async fn ifac_members_hear_each_other_and_strangers_stay_outside() {
        use crate::interfaces::ifac::IfacContext;
        use crate::wire::DestinationHash;

        let source = InterfaceId::new([0xA1; 16]);
        let peer = InterfaceId::new([0xB2; 16]);
        let view = std::vec![descriptor(source), descriptor(peer)];
        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);

        let network = || IfacContext::derive(Some("testnet"), Some("s3cret"), 8).unwrap();
        let ifacs = std::vec![
            InterfaceIfac {
                id: source,
                context: network(),
            },
            InterfaceIfac {
                id: peer,
                context: network(),
            },
        ];

        let (funnel_tx, funnel_rx) = mpsc::unbounded_channel::<InboundFrame>();
        let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_wire_out_tx, _source_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (source_out_tx, source_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let source_iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_seam = TokioInterfaceSeam::new(source, funnel_tx.clone(), source_out_rx);

        let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_wire_out_tx, mut peer_wire_out_rx) =
            mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (peer_out_tx, peer_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let peer_iface = LoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_seam = TokioInterfaceSeam::new(peer, funnel_tx.clone(), peer_out_rx);
        drop(funnel_tx);

        let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let app = move |journaled: Journaled<'_>| {
            if let Journaled::AnnounceHeard { destination, .. } = journaled {
                let _ = heard_tx.send(destination);
            }
        };

        tokio::spawn(run(
            engine,
            view,
            ifacs,
            TokioHost::new(),
            funnel_rx,
            command_rx,
            egress,
            app,
        ));
        tokio::spawn(source_iface.run(source_seam));
        tokio::spawn(peer_iface.run(peer_seam));

        let clean = hx(RAW_ANNOUNCE);
        let mut member_wire = [0u8; MAX_WIRE_FRAME_LEN];
        let masked_len = network().mask_outbound(&clean, &mut member_wire).unwrap();
        source_wire_in_tx
            .send(member_wire[..masked_len].to_vec())
            .expect("the source interface holds its wire");

        let heard = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
            .await
            .expect("a member's masked announce is heard")
            .expect("the reactor task is alive");
        assert_eq!(
            heard.as_bytes(),
            hx("16f8a6d3f7d7c5b6f106d293804d7314").as_slice(),
        );

        let rebroadcast = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
            .await
            .expect("the rebroadcast leaves through the peer")
            .expect("the peer task is alive");
        assert_eq!(
            rebroadcast[0] & 0x80,
            0x80,
            "the peer's wire only ever carries flagged, masked frames",
        );
        let mut recovered = [0u8; MAX_WIRE_FRAME_LEN];
        let clean_len = network()
            .unmask_inbound(&rebroadcast, &mut recovered)
            .expect("a member can open the rebroadcast");
        let (header, _) = WirePacketHeader::parse(&recovered[..clean_len]).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.hops, 1,
            "the relay bumped the hop count under the mask"
        );

        let stranger = IfacContext::derive(Some("testnet"), Some("wrong"), 8).unwrap();
        let mut stranger_wire = [0u8; MAX_WIRE_FRAME_LEN];
        let stranger_len = stranger
            .mask_outbound(&hx(RATCHETED_ANNOUNCE_RNS_WIRE), &mut stranger_wire)
            .unwrap();
        source_wire_in_tx
            .send(stranger_wire[..stranger_len].to_vec())
            .expect("the source interface holds its wire");
        assert!(
            tokio::time::timeout(Duration::from_secs(1), heard_rx.recv())
                .await
                .is_err(),
            "a stranger's code opens nothing",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_reactor_culls_an_expired_route_at_its_deadline() {
        use crate::engine::{
            CommandId, EngineCommand, SendSingle, SendSingleError, SendSingleFailure,
            SendSinglePayload, Settlement,
        };
        use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
        use crate::wire::DestinationHash;

        let source = InterfaceId::new([0xA1; 16]);
        let view = std::vec![descriptor(source)];
        let engine = EngineState::<Cap>::default();

        let (funnel_tx, funnel_rx) = mpsc::unbounded_channel::<InboundFrame>();
        let (wire_in_tx, wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (wire_out_tx, _wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (out_tx, out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let iface = LoopbackInterface {
            descriptor: descriptor(source),
            wire_in: wire_in_rx,
            wire_out: wire_out_tx,
        };
        let seam = TokioInterfaceSeam::new(source, funnel_tx.clone(), out_rx);
        drop(funnel_tx);
        let egress = Egress::new(std::vec![(source, out_tx)]);
        let (command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();

        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let (expired_tx, mut expired_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { destination, .. } => {
                let _ = heard_tx.send(destination);
            }
            Journaled::RouteExpired { destination } => {
                let _ = expired_tx.send(destination);
            }
            Journaled::CommandSettled { id, settlement } => {
                let _ = settled_tx.send((id, settlement));
            }
            Journaled::Delivered(_)
            | Journaled::RouteEvicted { .. }
            | Journaled::RouteInterfaceGone { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::LinkClosed { .. } => {}
        };

        tokio::spawn(run(
            engine,
            view,
            std::vec![],
            TokioHost::new(),
            funnel_rx,
            command_rx,
            egress,
            app,
        ));
        tokio::spawn(iface.run(seam));

        wire_in_tx
            .send(hx(RAW_ANNOUNCE))
            .expect("the interface holds its wire");
        let destination = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
            .await
            .expect("the announce is heard, so the route exists before the deadline")
            .expect("the reactor task is alive");

        tokio::time::sleep(Duration::from_millis(DEFAULT_ROUTE_EXPIRY_MILLIS + 10_000)).await;

        let expired = tokio::time::timeout(Duration::from_secs(2), expired_rx.recv())
            .await
            .expect("the cull journals the removal at the expiry deadline")
            .expect("the reactor task is alive");
        assert_eq!(
            expired, destination,
            "the expired route names its destination"
        );

        command_tx
            .send(IssuedCommand {
                id: CommandId(3),
                command: EngineCommand::SendSingle(SendSingle {
                    destination,
                    payload: SendSinglePayload::from_slice(b"late").expect("fits the MDU"),
                }),
            })
            .expect("the reactor task holds the receiver");

        let (settled_id, settlement) =
            tokio::time::timeout(Duration::from_secs(2), settled_rx.recv())
                .await
                .expect("the late send settles")
                .expect("the reactor task is alive");
        assert_eq!(settled_id, CommandId(3));
        assert_eq!(
            settlement,
            Settlement::SendSingle(Err(SendSingleFailure::Rejected(
                SendSingleError::NoRouteToDestination
            ))),
            "the reactor woke at the route's expiry and culled it",
        );
    }

    #[tokio::test]
    async fn a_commanded_announce_fans_to_every_interface_and_settles() {
        use crate::engine::{
            AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, RatchetPolicy,
            Settlement,
        };
        use crate::identity::Zeroizing;
        use crate::routing::upstream_app_destinations::ProofStrategy;

        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        let mut engine = EngineState::<Cap>::new(Zeroizing::new(secret));
        let node = engine.held_identity_hashes()[0];
        let destination = engine
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the single destination");

        let first = InterfaceId::new([0xA1; 16]);
        let second = InterfaceId::new([0xB2; 16]);
        let view = std::vec![descriptor(first), descriptor(second)];

        let (_inbound_tx, inbound_rx) = mpsc::unbounded_channel::<InboundFrame>();
        let (command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
        let (first_out_tx, mut first_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let (second_out_tx, mut second_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let egress = Egress::new(std::vec![(first, first_out_tx), (second, second_out_tx)]);

        let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let app = move |journaled: Journaled<'_>| match journaled {
            Journaled::CommandSettled { id, settlement } => {
                let _ = settled_tx.send((id, settlement));
            }
            Journaled::AnnounceHeard { .. }
            | Journaled::Delivered(_)
            | Journaled::RouteExpired { .. }
            | Journaled::RouteEvicted { .. }
            | Journaled::RouteInterfaceGone { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::LinkClosed { .. } => {}
        };

        tokio::spawn(run(
            engine,
            view,
            std::vec![],
            TokioHost::new(),
            inbound_rx,
            command_rx,
            egress,
            app,
        ));

        command_tx
            .send(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            })
            .expect("the reactor task holds the receiver");

        let (settled_id, settlement) =
            tokio::time::timeout(Duration::from_secs(2), settled_rx.recv())
                .await
                .expect("the command settles within the window")
                .expect("the reactor task is alive");
        assert_eq!(settled_id, CommandId(7));
        assert_eq!(settlement, Settlement::AnnounceNow(Ok(())));

        for out_rx in [&mut first_out_rx, &mut second_out_rx] {
            let frame = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
                .await
                .expect("an announce fires on each interface")
                .expect("the reactor task is alive");
            let (header, _) = WirePacketHeader::parse(frame.bytes()).expect("valid announce wire");
            assert_eq!(header.packet_type, PacketType::Announce);
            assert_eq!(header.destination, destination);
        }
    }

    #[tokio::test]
    async fn a_link_establishes_and_carries_data_across_two_live_reactors() {
        use crate::engine::test_support::{
            personal_node_announcer, personal_node_destination, second_secret_key,
        };
        use crate::engine::{
            AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EstablishLink,
            LinkEstablished, SendLink, SendLinkPayload, Settlement,
        };
        use crate::routing::delivery::Delivery;
        use crate::routing::links::LinkId;

        let initiator_iface = InterfaceId::new([0xA1; 16]);
        let responder_iface = InterfaceId::new([0xB2; 16]);

        let (a_to_b_tx, a_to_b_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (b_to_a_tx, b_to_a_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();

        let initiator_engine = EngineState::<Cap>::new(second_secret_key());
        let (a_funnel_tx, a_funnel_rx) = mpsc::unbounded_channel::<InboundFrame>();
        let (a_out_tx, a_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let a_iface = LoopbackInterface {
            descriptor: descriptor(initiator_iface),
            wire_in: b_to_a_rx,
            wire_out: a_to_b_tx,
        };
        let a_seam = TokioInterfaceSeam::new(initiator_iface, a_funnel_tx.clone(), a_out_rx);
        drop(a_funnel_tx);
        let a_egress = Egress::new(std::vec![(initiator_iface, a_out_tx)]);
        let (a_command_tx, a_command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
        let (a_heard_tx, mut a_heard_rx) = mpsc::unbounded_channel::<()>();
        let (a_settled_tx, mut a_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let (a_delivered_tx, mut a_delivered_rx) =
            mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
        let a_app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                let _ = a_heard_tx.send(());
            }
            Journaled::CommandSettled { id, settlement } => {
                let _ = a_settled_tx.send((id, settlement));
            }
            Journaled::Delivered(Delivery::Link(link)) => {
                let _ = a_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
            }
            _ => {}
        };

        let responder_engine = personal_node_announcer();
        let (b_funnel_tx, b_funnel_rx) = mpsc::unbounded_channel::<InboundFrame>();
        let (b_out_tx, b_out_rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let b_iface = LoopbackInterface {
            descriptor: descriptor(responder_iface),
            wire_in: a_to_b_rx,
            wire_out: b_to_a_tx,
        };
        let b_seam = TokioInterfaceSeam::new(responder_iface, b_funnel_tx.clone(), b_out_rx);
        drop(b_funnel_tx);
        let b_egress = Egress::new(std::vec![(responder_iface, b_out_tx)]);
        let (b_command_tx, b_command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
        let (b_established_tx, mut b_established_rx) = mpsc::unbounded_channel::<LinkEstablished>();
        let (b_settled_tx, mut b_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let (b_delivered_tx, mut b_delivered_rx) =
            mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
        let b_app = move |journaled: Journaled<'_>| match journaled {
            Journaled::LinkEstablished(established) => {
                let _ = b_established_tx.send(established);
            }
            Journaled::CommandSettled { id, settlement } => {
                let _ = b_settled_tx.send((id, settlement));
            }
            Journaled::Delivered(Delivery::Link(link)) => {
                let _ = b_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
            }
            _ => {}
        };

        tokio::spawn(run(
            initiator_engine,
            std::vec![descriptor(initiator_iface)],
            std::vec![],
            TokioHost::new(),
            a_funnel_rx,
            a_command_rx,
            a_egress,
            a_app,
        ));
        tokio::spawn(run(
            responder_engine,
            std::vec![descriptor(responder_iface)],
            std::vec![],
            TokioHost::new(),
            b_funnel_rx,
            b_command_rx,
            b_egress,
            b_app,
        ));
        tokio::spawn(a_iface.run(a_seam));
        tokio::spawn(b_iface.run(b_seam));

        b_command_tx
            .send(IssuedCommand {
                id: CommandId(1),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), a_heard_rx.recv())
            .await
            .expect("the announce crosses the wire")
            .expect("the initiator reactor is alive");

        a_command_tx
            .send(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::EstablishLink(EstablishLink {
                    destination: personal_node_destination(),
                }),
            })
            .unwrap();

        let (settled_id, settlement) =
            tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
                .await
                .expect("the link settles within the window")
                .expect("the initiator reactor is alive");
        assert_eq!(settled_id, CommandId(7));
        let Settlement::EstablishLink(Ok(established)) = settlement else {
            panic!("the command must settle established, got {settlement:?}");
        };

        let responder_side = tokio::time::timeout(Duration::from_secs(5), b_established_rx.recv())
            .await
            .expect("the responder journals the link up")
            .expect("the responder reactor is alive");
        assert_eq!(
            responder_side.link_id, established.link_id,
            "one link, two ends",
        );
        assert!(
            responder_side.rtt_ms >= established.rtt_ms,
            "the responder takes max(measured, reported)",
        );

        a_command_tx
            .send(IssuedCommand {
                id: CommandId(8),
                command: EngineCommand::SendLink(SendLink {
                    link_id: established.link_id,
                    payload: SendLinkPayload::from_slice(b"ping over the live link").unwrap(),
                }),
            })
            .unwrap();
        let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
            .await
            .expect("the initiator's send settles")
            .expect("the initiator reactor is alive");
        assert_eq!(
            (sent_id, sent),
            (CommandId(8), Settlement::SendLink(Ok(())))
        );
        let delivered = tokio::time::timeout(Duration::from_secs(5), b_delivered_rx.recv())
            .await
            .expect("the responder journals the delivery")
            .expect("the responder reactor is alive");
        assert_eq!(
            delivered,
            (established.link_id, b"ping over the live link".to_vec()),
        );

        b_command_tx
            .send(IssuedCommand {
                id: CommandId(2),
                command: EngineCommand::SendLink(SendLink {
                    link_id: established.link_id,
                    payload: SendLinkPayload::from_slice(b"pong right back").unwrap(),
                }),
            })
            .unwrap();
        let sent = loop {
            let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), b_settled_rx.recv())
                .await
                .expect("the responder's send settles")
                .expect("the responder reactor is alive");
            if sent_id == CommandId(2) {
                break sent;
            }
        };
        assert_eq!(sent, Settlement::SendLink(Ok(())));
        let delivered = tokio::time::timeout(Duration::from_secs(5), a_delivered_rx.recv())
            .await
            .expect("the initiator journals the delivery")
            .expect("the initiator reactor is alive");
        assert_eq!(
            delivered,
            (established.link_id, b"pong right back".to_vec()),
        );
    }
}
