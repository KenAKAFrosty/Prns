use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled,
};
use crate::interfaces::{
    ConnectionState, InboundPacket, InterfaceConfig, InterfaceId, InterfaceStatus,
};
use crate::reactor::announce_pacer::{AnnouncePacer, HeapPacerQueue};
use crate::reactor::driver::{
    draw_jitter, fire_due_lane, merge_wake_schedules_delta, wait_for_due_lane, wait_for_pacer,
};
use crate::reactor::interface_seam::{InboundFrame, InterfaceSeam, OutboundFrame};
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
}

pub async fn run<S, H, A>(
    mut engine: EngineState<S>,
    interfaces: std::vec::Vec<InterfaceConfig>,
    mut host: H,
    mut inbound: UnboundedReceiver<InboundFrame>,
    mut commands: UnboundedReceiver<IssuedCommand>,
    egress: Egress,
    mut app: A,
) where
    S: EngineStorage,
    H: Host,
    A: FnMut(Journaled<'_>),
{
    let mut wake_schedules = engine.wake_schedules();
    let mut pacers: std::vec::Vec<(InterfaceId, AnnouncePacer<HeapPacerQueue>)> = interfaces
        .iter()
        .map(|config| (config.id, AnnouncePacer::new(config.announce_bandwidth_cap)))
        .collect();
    loop {
        let wake = wake_schedules.soonest(host.now());
        let pacer_wake = soonest_pacer_release(&pacers);
        tokio::select! {
            arrived = inbound.recv() => {
                let Some(mut frame) = arrived else { return };
                let now = host.now();
                let jitter = draw_jitter(&mut host);
                let packet = InboundPacket {
                    arrived_at: now,
                    source_interface: frame.source,
                    bytes: &mut frame.bytes[..frame.len],
                };
                let wake_schedules_delta = engine.ingest_packet_into(
                    packet,
                    jitter,
                    &interfaces,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut |reaction| route_reaction(reaction, &egress, &mut pacers, now, &mut app),
                );
                merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine);
            }
            issued = commands.recv() => {
                let Some(issued) = issued else { return };
                let now = host.now();
                let wake_schedules_delta = engine.ingest_command_into(
                    issued,
                    &interfaces,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut |reaction| route_reaction(reaction, &egress, &mut pacers, now, &mut app),
                );
                merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine);
            }
            lane = wait_for_due_lane(&host, wake) => {
                let now = host.now();
                let wake_schedules_delta = fire_due_lane(
                    &mut engine,
                    lane,
                    now,
                    &interfaces,
                    &mut host,
                    &mut |reaction| route_reaction(reaction, &egress, &mut pacers, now, &mut app),
                );
                merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine);
            }
            _ = wait_for_pacer(&host, pacer_wake) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &egress);
            }
        }
    }
}

fn route_reaction<A: FnMut(Journaled<'_>)>(
    reaction: EngineReaction<'_>,
    egress: &Egress,
    pacers: &mut [(InterfaceId, AnnouncePacer<HeapPacerQueue>)],
    now: InstantMillis,
    app: &mut A,
) {
    match reaction {
        EngineReaction::Directive(Directive::Send { target, bytes }) => {
            egress.enqueue(target, bytes);
        }
        EngineReaction::Directive(Directive::SendAnnounce { target, bytes }) => {
            offer_to_pacer(pacers, target, bytes, now, egress);
        }
        EngineReaction::Journaled(journaled) => app(journaled),
    }
}

fn offer_to_pacer(
    pacers: &mut [(InterfaceId, AnnouncePacer<HeapPacerQueue>)],
    target: InterfaceId,
    bytes: &[u8],
    now: InstantMillis,
    egress: &Egress,
) {
    match pacers.iter_mut().find(|(id, _)| *id == target) {
        Some((_, pacer)) => pacer.offer(bytes, now, |frame| egress.enqueue(target, frame)),
        None => egress.enqueue(target, bytes),
    }
}

fn flush_due_pacers(
    pacers: &mut [(InterfaceId, AnnouncePacer<HeapPacerQueue>)],
    now: InstantMillis,
    egress: &Egress,
) {
    for (id, pacer) in pacers.iter_mut() {
        let target = *id;
        pacer.release_due(now, |frame| egress.enqueue(target, frame));
    }
}

fn soonest_pacer_release(
    pacers: &[(InterfaceId, AnnouncePacer<HeapPacerQueue>)],
) -> Option<InstantMillis> {
    pacers
        .iter()
        .filter_map(|(_, pacer)| pacer.next_release())
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
    use tokio::sync::mpsc;

    fn descriptor(id: InterfaceId) -> InterfaceConfig {
        InterfaceConfig {
            id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        }
    }

    #[test]
    fn the_pacer_wiring_holds_then_releases_a_capped_burst() {
        let id = InterfaceId::new([0x5a; 16]);
        let cap = AnnounceBandwidthCap::Limited {
            bitrate_bps: 5_000,
            cap_per_mille: 20,
        };
        let mut pacers = std::vec![(id, AnnouncePacer::<HeapPacerQueue>::new(cap))];
        let (tx, mut rx) = mpsc::unbounded_channel::<OutboundFrame>();
        let egress = Egress::new(std::vec![(id, tx)]);

        offer_to_pacer(&mut pacers, id, &[1; 10], InstantMillis(1_000), &egress);
        assert_eq!(rx.try_recv().unwrap().bytes(), [1u8; 10].as_slice());

        offer_to_pacer(&mut pacers, id, &[2; 10], InstantMillis(1_200), &egress);
        assert!(rx.try_recv().is_err(), "the second is held, not sent");
        assert_eq!(soonest_pacer_release(&pacers), Some(InstantMillis(1_800)));

        flush_due_pacers(&mut pacers, InstantMillis(1_799), &egress);
        assert!(rx.try_recv().is_err(), "nothing releases before the window");

        flush_due_pacers(&mut pacers, InstantMillis(1_800), &egress);
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
            Journaled::Delivered(_) | Journaled::CommandSettled { .. } => {}
        };

        tokio::spawn(run(
            engine,
            view,
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
            ContextFlag, DestinationType, IfacFlag, PropagationType, WireContext, MTU,
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
        let mut wire = [0u8; MTU];
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
            Journaled::AnnounceHeard { .. } | Journaled::CommandSettled { .. } => {}
        };

        tokio::spawn(run(
            engine,
            view,
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
            Journaled::AnnounceHeard { .. } | Journaled::Delivered(_) => {}
        };

        tokio::spawn(run(
            engine,
            view,
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
}
