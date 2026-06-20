use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::interfaces::bluetooth_auto::core::{
    self, arrangement, is_keeper, l2cap_plan, BleAddress, BleIdentity, Established, Handshake,
    HandshakeRole, LinkCapabilities, Local, Outcome,
};
use crate::interfaces::bluetooth_auto::core::{Endpoint, L2capPlan};
use crate::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam, MAX_WIRE_FRAME_LEN};
use crate::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

struct ClosedSignal {
    identity: BleIdentity,
    address: BleAddress,
    sink: mpsc::UnboundedSender<(BleIdentity, BleAddress)>,
}

pub struct BluetoothPeer<Src, Snk> {
    id: InterfaceId,
    identity: BleIdentity,
    source: Src,
    sink: Snk,
    channel_tag: [u8; 16],
    status: TokioInterfaceStatus,
    closed: Option<ClosedSignal>,
}

impl<Src: BleSource, Snk: BleSink> BluetoothPeer<Src, Snk> {
    pub fn new(identity: BleIdentity, source: Src, sink: Snk) -> Self {
        let channel_tag = *identity.as_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &channel_tag);
        Self {
            id,
            identity,
            source,
            sink,
            channel_tag,
            status: TokioInterfaceStatus::new(id, ConnectionState::Connected),
            closed: None,
        }
    }

    fn report_close_to(
        mut self,
        address: BleAddress,
        sink: mpsc::UnboundedSender<(BleIdentity, BleAddress)>,
    ) -> Self {
        self.closed = Some(ClosedSignal {
            identity: self.identity,
            address,
            sink,
        });
        self
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    #[must_use]
    pub fn identity(&self) -> BleIdentity {
        self.identity
    }
}

impl<Src: BleSource, Snk: BleSink> Interface for BluetoothPeer<Src, Snk> {
    const HW_MTU: usize = core::BLE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::BluetoothPeer;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, core::BLE_BITRATE_GUESS_BPS)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let mut buf = [0u8; MAX_WIRE_FRAME_LEN];
        loop {
            tokio::select! {
                received = self.source.recv_frame(&mut buf) => {
                    let Ok(len) = received else { break };
                    if len == 0 {
                        continue;
                    }
                    self.status.add_rx(len as u64);
                    seam.next_inbound(&buf[..len]).await;
                }
                outbound = seam.next_outbound() => {
                    if outbound.is_empty() {
                        continue;
                    }
                    let outbound_len = outbound.len();
                    if self.sink.send_frame(outbound).await.is_err() {
                        break;
                    }
                    self.status.add_tx(outbound_len as u64);
                }
            }
        }
        if let Some(ClosedSignal {
            identity,
            address,
            sink,
        }) = self.closed.take()
        {
            let _ = sink.send((identity, address));
            std::future::pending::<()>().await;
        }
    }
}

impl<Src: BleSource, Snk: BleSink> crate::interfaces::ReportsStatus for BluetoothPeer<Src, Snk> {
    fn status_view(&self) -> Option<crate::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![crate::interfaces::InterfaceSnapshot::of(&status)]
        }))
    }
}

struct Settled {
    member: AttachedInterface,
    keeper: bool,
    address: BleAddress,
}

struct HandshakeDone<L: BleLink> {
    address: BleAddress,
    origin: Origin,
    outcome: Option<(Established, L)>,
}

type HandshakeQueue<L> = FuturesUnordered<Pin<Box<dyn Future<Output = HandshakeDone<L>>>>>;

enum Step<L: BleLink> {
    Event(BleEvent<L>),
    Handshake(HandshakeDone<L>),
    Closed(BleIdentity, BleAddress),
}

pub struct BluetoothAuto<B> {
    backend: B,
    local: Local,
}

impl<B: BleBackend> BluetoothAuto<B> {
    pub fn new(
        backend: B,
        identity: BleIdentity,
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
    ) -> Self {
        Self {
            backend,
            local: Local {
                identity,
                endpoint,
                capabilities,
            },
        }
    }
}

impl<B> InterfaceSupervisor for BluetoothAuto<B>
where
    B: BleBackend,
    B::Link: 'static,
    <B::Link as BleLink>::Source: Send + 'static,
    <B::Link as BleLink>::Sink: Send + 'static,
{
    const KIND: InterfaceKind = InterfaceKind::BluetoothAuto;

    fn channel_tag(&self) -> &[u8] {
        self.local.identity.as_bytes()
    }

    async fn run(self, fleet: Fleet) {
        let Self { mut backend, local } = self;
        let mut advertising = backend.set_advertising(true).await.is_ok();
        let mut dialing: HashMap<BleAddress, Instant> = HashMap::new();
        let mut suppressed: HashMap<BleAddress, Instant> = HashMap::new();
        let mut settled: HashMap<BleIdentity, Settled> = HashMap::new();
        let mut settled_by_addr: HashMap<BleAddress, BleIdentity> = HashMap::new();
        let mut handshakes: HandshakeQueue<B::Link> = FuturesUnordered::new();
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<(BleIdentity, BleAddress)>();
        loop {
            let step = tokio::select! {
                event = backend.next_event() => Step::Event(event),
                Some(done) = handshakes.next(), if !handshakes.is_empty() => Step::Handshake(done),
                Some((identity, address)) = closed_rx.recv() => Step::Closed(identity, address),
            };
            match step {
                Step::Event(BleEvent::Sighting { address, .. }) => {
                    let dialable = settled.len() < B::MAX_PEERS
                        && !settled_by_addr.contains_key(&address)
                        && match (dialing.get(&address), suppressed.get(&address)) {
                            (Some(since), _) => since.elapsed() >= DIAL_RETRY_TTL,
                            (None, Some(since)) => since.elapsed() >= SUPPRESS_TTL,
                            (None, None) => true,
                        };
                    if dialable {
                        dialing.insert(address, Instant::now());
                        backend.dial(address).await;
                    }
                }
                Step::Event(BleEvent::LinkReady {
                    link,
                    origin,
                    peer_rssi,
                }) => {
                    let address = link.address();
                    let role = match origin {
                        Origin::Dialed => HandshakeRole::Dialer,
                        Origin::Accepted => HandshakeRole::Listener,
                    };
                    handshakes.push(Box::pin(run_handshake_task(
                        link, role, local, address, origin, peer_rssi,
                    )));
                }
                Step::Event(BleEvent::Inbound(link)) => {
                    let address = link.address();
                    handshakes.push(Box::pin(run_handshake_task(
                        link,
                        HandshakeRole::Listener,
                        local,
                        address,
                        Origin::Accepted,
                        None,
                    )));
                }
                Step::Handshake(done) => {
                    resolve(
                        done,
                        &fleet,
                        &mut dialing,
                        &mut suppressed,
                        &mut settled,
                        &mut settled_by_addr,
                        local,
                        &closed_tx,
                        &mut backend,
                    )
                    .await;
                }
                Step::Closed(identity, address) => {
                    retire(identity, address, &mut settled, &mut settled_by_addr);
                    backend.on_link_closed(address).await;
                }
            }
            reconcile_advertising(&mut backend, &mut advertising, settled.len() < B::MAX_PEERS)
                .await;
        }
    }
}

impl<B: BleBackend> crate::interfaces::ReportsStatus for BluetoothAuto<B> {}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const SUPPRESS_TTL: Duration = Duration::from_secs(8);
const DIAL_RETRY_TTL: Duration = Duration::from_secs(16);

async fn reconcile_advertising<B: BleBackend>(backend: &mut B, advertising: &mut bool, want: bool) {
    if want != *advertising {
        let _ = backend.set_advertising(want).await;
        *advertising = want;
    }
}

async fn run_handshake_task<L: BleLink>(
    mut link: L,
    role: HandshakeRole,
    local: Local,
    address: BleAddress,
    origin: Origin,
    measured_rssi: Option<i8>,
) -> HandshakeDone<L> {
    let outcome = drive_handshake(&mut link, role, local, measured_rssi)
        .await
        .map(|established| (established, link));
    HandshakeDone {
        address,
        origin,
        outcome,
    }
}

async fn drive_handshake<L: BleLink>(
    link: &mut L,
    role: HandshakeRole,
    local: Local,
    measured_rssi: Option<i8>,
) -> Option<Established> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        let (mut handshake, opening) = Handshake::begin(role, local, measured_rssi);
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

#[allow(clippy::too_many_arguments)]
async fn resolve<B>(
    done: HandshakeDone<B::Link>,
    fleet: &Fleet,
    dialing: &mut HashMap<BleAddress, Instant>,
    suppressed: &mut HashMap<BleAddress, Instant>,
    settled: &mut HashMap<BleIdentity, Settled>,
    settled_by_addr: &mut HashMap<BleAddress, BleIdentity>,
    local: Local,
    closed: &mpsc::UnboundedSender<(BleIdentity, BleAddress)>,
    backend: &mut B,
) where
    B: BleBackend,
    B::Link: 'static,
    <B::Link as BleLink>::Source: Send + 'static,
    <B::Link as BleLink>::Sink: Send + 'static,
{
    let HandshakeDone {
        address,
        origin,
        outcome,
    } = done;
    let dialed = matches!(origin, Origin::Dialed);
    if dialed {
        dialing.remove(&address);
    }
    let Some((established, mut link)) = outcome else {
        if dialed {
            backend.on_link_closed(address).await;
        }
        return;
    };
    let identity = established.identity;
    let role = if dialed {
        HandshakeRole::Dialer
    } else {
        HandshakeRole::Listener
    };
    let plan = arrangement(local.endpoint, established.endpoint);
    let keeper = is_keeper(plan, role, local.identity, local.endpoint, identity);

    if let Some(incumbent) = settled.get(&identity) {
        let challenger_wins = keeper && !incumbent.keeper;
        if !challenger_wins {
            suppressed.insert(address, Instant::now());
            if dialed {
                backend.on_link_closed(address).await;
            }
            return;
        }
        if let Some(old) = settled.remove(&identity) {
            settled_by_addr.remove(&old.address);
            old.member.teardown();
        }
    } else if settled.len() >= B::MAX_PEERS {
        suppressed.insert(address, Instant::now());
        if dialed {
            backend.on_link_closed(address).await;
        }
        return;
    }

    let lane = l2cap_plan(
        plan,
        role,
        local.endpoint,
        &local.capabilities,
        &established.capabilities,
    );
    arm_fast_lane(&mut link, &lane).await;
    let (source, sink) = link.into_data();
    let member =
        BluetoothPeer::new(identity, source, sink).report_close_to(address, closed.clone());
    let attached = fleet.add(member);
    settled.insert(
        identity,
        Settled {
            member: attached,
            keeper,
            address,
        },
    );
    settled_by_addr.insert(address, identity);
}

async fn arm_fast_lane<L: BleLink>(link: &mut L, lane: &L2capPlan) {
    if matches!(lane, L2capPlan::None) {
        return;
    }
    let _ = link.upgrade(lane).await;
}

fn retire(
    identity: BleIdentity,
    address: BleAddress,
    settled: &mut HashMap<BleIdentity, Settled>,
    settled_by_addr: &mut HashMap<BleAddress, BleIdentity>,
) {
    if settled
        .get(&identity)
        .is_some_and(|entry| entry.address == address)
    {
        if let Some(entry) = settled.remove(&identity) {
            settled_by_addr.remove(&entry.address);
            entry.member.teardown();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::mpsc;

    use super::*;
    use crate::interfaces::bluetooth_auto::core::{
        AndroidHost, AppleHost, BleAddress, BlueZHost, Control, Dialect, Psm,
    };
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};

    const TEST_FRAME_CAP: usize = 2_048;

    fn mac() -> Endpoint {
        Endpoint::CoreBluetooth(AppleHost::MacOs)
    }

    fn linux() -> Endpoint {
        Endpoint::BlueZ(BlueZHost::Linux)
    }

    fn android() -> Endpoint {
        Endpoint::Android(AndroidHost::Android)
    }

    fn caps(psm: u16) -> LinkCapabilities {
        LinkCapabilities {
            l2cap: Psm::new(psm),
            link_mtu: 247,
        }
    }

    struct MockSeam {
        inbound: mpsc::UnboundedSender<std::vec::Vec<u8>>,
        outbound: TokioGrantConsumer,
    }

    impl InterfaceSeam for MockSeam {
        async fn next_inbound(&mut self, frame: &[u8]) {
            let _ = self.inbound.send(frame.to_vec());
        }

        async fn next_outbound(&mut self) -> &[u8] {
            self.outbound.release();
            self.outbound.peek().await.frame()
        }
    }

    #[derive(Debug)]
    struct Closed;

    struct LoopbackLink {
        address: BleAddress,
        control_tx: mpsc::Sender<Control>,
        control_rx: mpsc::Receiver<Control>,
        data_tx: mpsc::Sender<std::vec::Vec<u8>>,
        data_rx: mpsc::Receiver<std::vec::Vec<u8>>,
    }

    struct LoopbackSource {
        data_rx: mpsc::Receiver<std::vec::Vec<u8>>,
    }

    struct LoopbackSink {
        data_tx: mpsc::Sender<std::vec::Vec<u8>>,
    }

    impl BleLink for LoopbackLink {
        type Error = Closed;
        type Source = LoopbackSource;
        type Sink = LoopbackSink;

        fn dialect(&self) -> Dialect {
            Dialect::Native
        }

        fn address(&self) -> BleAddress {
            self.address
        }

        async fn control_send(&mut self, msg: &Control) -> Result<(), Closed> {
            self.control_tx.send(*msg).await.map_err(|_| Closed)
        }

        async fn control_recv(&mut self) -> Result<Control, Closed> {
            self.control_rx.recv().await.ok_or(Closed)
        }

        async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), Closed> {
            Ok(())
        }

        fn into_data(self) -> (LoopbackSource, LoopbackSink) {
            (
                LoopbackSource {
                    data_rx: self.data_rx,
                },
                LoopbackSink {
                    data_tx: self.data_tx,
                },
            )
        }
    }

    impl BleSource for LoopbackSource {
        type Error = Closed;

        async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Closed> {
            let frame = self.data_rx.recv().await.ok_or(Closed)?;
            let len = frame.len().min(out.len());
            out[..len].copy_from_slice(&frame[..len]);
            Ok(len)
        }
    }

    impl BleSink for LoopbackSink {
        type Error = Closed;

        async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Closed> {
            self.data_tx.send(frame.to_vec()).await.map_err(|_| Closed)
        }
    }

    fn link_pair(addr_a: BleAddress, addr_b: BleAddress) -> (LoopbackLink, LoopbackLink) {
        let (ctl_a_tx, ctl_a_rx) = mpsc::channel(8);
        let (ctl_b_tx, ctl_b_rx) = mpsc::channel(8);
        let (dat_a_tx, dat_a_rx) = mpsc::channel(8);
        let (dat_b_tx, dat_b_rx) = mpsc::channel(8);
        let a = LoopbackLink {
            address: addr_b,
            control_tx: ctl_a_tx,
            control_rx: ctl_b_rx,
            data_tx: dat_a_tx,
            data_rx: dat_b_rx,
        };
        let b = LoopbackLink {
            address: addr_a,
            control_tx: ctl_b_tx,
            control_rx: ctl_a_rx,
            data_tx: dat_b_tx,
            data_rx: dat_a_rx,
        };
        (a, b)
    }

    struct LoopbackBleBackend {
        inbound_rx: mpsc::Receiver<LoopbackLink>,
        peer_inbound_tx: mpsc::Sender<LoopbackLink>,
        our_address: BleAddress,
        peer_address: BleAddress,
        sighting_pending: bool,
        dialed: Option<LoopbackLink>,
    }

    impl LoopbackBleBackend {
        fn pair() -> (Self, Self) {
            let addr_a = BleAddress::new([0xAA; 6]);
            let addr_b = BleAddress::new([0xBB; 6]);
            let (a_inbound_tx, a_inbound_rx) = mpsc::channel(8);
            let (b_inbound_tx, b_inbound_rx) = mpsc::channel(8);
            let a = Self {
                inbound_rx: a_inbound_rx,
                peer_inbound_tx: b_inbound_tx,
                our_address: addr_a,
                peer_address: addr_b,
                sighting_pending: true,
                dialed: None,
            };
            let b = Self {
                inbound_rx: b_inbound_rx,
                peer_inbound_tx: a_inbound_tx,
                our_address: addr_b,
                peer_address: addr_a,
                sighting_pending: false,
                dialed: None,
            };
            (a, b)
        }
    }

    impl BleBackend for LoopbackBleBackend {
        const MAX_PEERS: usize = 8;
        type Error = Closed;
        type Link = LoopbackLink;

        async fn set_advertising(&mut self, _enabled: bool) -> Result<(), Closed> {
            Ok(())
        }

        async fn next_event(&mut self) -> BleEvent<LoopbackLink> {
            if self.sighting_pending {
                self.sighting_pending = false;
                return BleEvent::Sighting {
                    address: self.peer_address,
                    rssi: None,
                };
            }
            if let Some(link) = self.dialed.take() {
                return BleEvent::LinkReady {
                    link,
                    origin: Origin::Dialed,
                    peer_rssi: None,
                };
            }
            match self.inbound_rx.recv().await {
                Some(link) => BleEvent::LinkReady {
                    link,
                    origin: Origin::Accepted,
                    peer_rssi: None,
                },
                None => std::future::pending().await,
            }
        }

        async fn dial(&mut self, address: BleAddress) {
            let (mine, theirs) = link_pair(self.our_address, address);
            if self.peer_inbound_tx.send(theirs).await.is_ok() {
                self.dialed = Some(mine);
            }
        }
    }

    #[tokio::test]
    async fn two_nodes_handshake_over_loopback_and_a_frame_crosses() {
        let local_a = Local {
            identity: BleIdentity::new([1u8; 16]),
            endpoint: mac(),
            capabilities: caps(0x0081),
        };
        let local_b = Local {
            identity: BleIdentity::new([2u8; 16]),
            endpoint: linux(),
            capabilities: caps(0x0082),
        };
        let (mut backend_a, mut backend_b) = LoopbackBleBackend::pair();

        let dialer = async move {
            let BleEvent::Sighting { address, .. } = backend_a.next_event().await else {
                unreachable!("the dialer side only sights")
            };
            backend_a.dial(address).await;
            let BleEvent::LinkReady { mut link, .. } = backend_a.next_event().await else {
                unreachable!("the dial completes into a link")
            };
            let established = drive_handshake(&mut link, HandshakeRole::Dialer, local_a, None)
                .await
                .unwrap();
            (established, link)
        };
        let listener = async move {
            let BleEvent::LinkReady { mut link, .. } = backend_b.next_event().await else {
                unreachable!("the listener side accepts an inbound link")
            };
            let established = drive_handshake(&mut link, HandshakeRole::Listener, local_b, None)
                .await
                .unwrap();
            (established, link)
        };
        let ((established_a, link_a), (established_b, link_b)) = tokio::join!(dialer, listener);

        assert_eq!(established_a.identity, local_b.identity);
        assert_eq!(established_a.endpoint, local_b.endpoint);
        assert_eq!(established_b.identity, local_a.identity);
        assert_eq!(established_b.endpoint, local_a.endpoint);

        let (source_a, sink_a) = link_a.into_data();
        let (source_b, sink_b) = link_b.into_data();
        let member_a = BluetoothPeer::new(established_a.identity, source_a, sink_a);
        let member_b = BluetoothPeer::new(established_b.identity, source_b, sink_b);

        let (discard_a, _discard_a_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_a_producer, out_a_consumer) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let seam_a = MockSeam {
            inbound: discard_a,
            outbound: out_a_consumer,
        };

        let (capture_tx, mut capture_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (_idle_b_producer, idle_b_consumer) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let seam_b = MockSeam {
            inbound: capture_tx,
            outbound: idle_b_consumer,
        };

        tokio::spawn(member_a.run(seam_a));
        tokio::spawn(member_b.run(seam_b));

        let frame = [0x10u8, 0x20, 0x30, 0x40];
        out_a_producer.try_grant().unwrap().fill(&frame);
        out_a_producer.commit();

        let received = tokio::time::timeout(Duration::from_secs(2), capture_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, frame);
    }

    #[tokio::test]
    async fn a_gatt_only_listener_settles_the_link_on_the_floor() {
        let local_a = Local {
            identity: BleIdentity::new([1u8; 16]),
            endpoint: mac(),
            capabilities: caps(0x0081),
        };
        let local_b = Local {
            identity: BleIdentity::new([2u8; 16]),
            endpoint: linux(),
            capabilities: LinkCapabilities {
                l2cap: None,
                link_mtu: 247,
            },
        };
        let (mut backend_a, mut backend_b) = LoopbackBleBackend::pair();

        let dialer = async move {
            let BleEvent::Sighting { address, .. } = backend_a.next_event().await else {
                unreachable!("the dialer side only sights")
            };
            backend_a.dial(address).await;
            let BleEvent::LinkReady { mut link, .. } = backend_a.next_event().await else {
                unreachable!("the dial completes into a link")
            };
            drive_handshake(&mut link, HandshakeRole::Dialer, local_a, None)
                .await
                .unwrap()
        };
        let listener = async move {
            let BleEvent::LinkReady { mut link, .. } = backend_b.next_event().await else {
                unreachable!("the listener side accepts an inbound link")
            };
            drive_handshake(&mut link, HandshakeRole::Listener, local_b, None)
                .await
                .unwrap()
        };
        let (established_a, established_b) = tokio::join!(dialer, listener);

        let arr_a = arrangement(local_a.endpoint, established_a.endpoint);
        let arr_b = arrangement(local_b.endpoint, established_b.endpoint);
        assert_eq!(
            l2cap_plan(
                arr_a,
                HandshakeRole::Dialer,
                local_a.endpoint,
                &local_a.capabilities,
                &established_a.capabilities,
            ),
            L2capPlan::None
        );
        assert_eq!(
            l2cap_plan(
                arr_b,
                HandshakeRole::Listener,
                local_b.endpoint,
                &local_b.capabilities,
                &established_b.capabilities,
            ),
            L2capPlan::None
        );
    }

    #[tokio::test]
    async fn an_android_peer_is_the_l2cap_opener_and_the_mac_accepts() {
        let mac_local = Local {
            identity: BleIdentity::new([1u8; 16]),
            endpoint: mac(),
            capabilities: caps(0x00c0),
        };
        let android_local = Local {
            identity: BleIdentity::new([2u8; 16]),
            endpoint: android(),
            capabilities: caps(0x0080),
        };
        let (mut mac_backend, mut android_backend) = LoopbackBleBackend::pair();

        let mac_side = async move {
            let BleEvent::Sighting { address, .. } = mac_backend.next_event().await else {
                unreachable!("mac sights android")
            };
            mac_backend.dial(address).await;
            let BleEvent::LinkReady { mut link, .. } = mac_backend.next_event().await else {
                unreachable!("the dial completes into a link")
            };
            drive_handshake(&mut link, HandshakeRole::Dialer, mac_local, None)
                .await
                .unwrap()
        };
        let android_side = async move {
            let BleEvent::LinkReady { mut link, .. } = android_backend.next_event().await else {
                unreachable!("android accepts an inbound link")
            };
            drive_handshake(&mut link, HandshakeRole::Listener, android_local, None)
                .await
                .unwrap()
        };
        let (mac_established, android_established) = tokio::join!(mac_side, android_side);

        let arr = arrangement(mac_local.endpoint, mac_established.endpoint);
        let mac_keeper = is_keeper(
            arr,
            HandshakeRole::Dialer,
            mac_local.identity,
            mac_local.endpoint,
            mac_established.identity,
        );
        let android_keeper = is_keeper(
            arr,
            HandshakeRole::Listener,
            android_local.identity,
            android_local.endpoint,
            android_established.identity,
        );
        assert!(!mac_keeper);
        assert!(!android_keeper);
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Listener,
                android_local.endpoint,
                &android_local.capabilities,
                &android_established.capabilities,
            ),
            L2capPlan::None
        );
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Dialer,
                mac_local.endpoint,
                &mac_local.capabilities,
                &mac_established.capabilities,
            ),
            L2capPlan::Accept
        );
    }

    fn idle_seam() -> MockSeam {
        let (discard, _discard_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (_idle_producer, idle_consumer) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        MockSeam {
            inbound: discard,
            outbound: idle_consumer,
        }
    }

    #[tokio::test]
    async fn a_dying_member_fires_its_close_signal() {
        let addr = BleAddress::new([0x11; 6]);
        let (link_a, link_b) = link_pair(addr, BleAddress::new([0x22; 6]));
        let (source, sink) = link_a.into_data();
        drop(link_b);

        let identity = BleIdentity::new([1u8; 16]);
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<(BleIdentity, BleAddress)>();
        let member =
            BluetoothPeer::new(identity, source, sink).report_close_to(addr, closed_tx.clone());
        tokio::spawn(member.run(idle_seam()));

        let reported = tokio::time::timeout(Duration::from_secs(2), closed_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reported, (identity, addr));
    }

    #[tokio::test]
    async fn a_torn_down_member_does_not_fire_its_close_signal() {
        let addr = BleAddress::new([0x11; 6]);
        let (link_a, link_b) = link_pair(addr, BleAddress::new([0x22; 6]));
        let (source, sink) = link_a.into_data();
        let _keep_peer_alive = link_b;

        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<(BleIdentity, BleAddress)>();
        let member = BluetoothPeer::new(BleIdentity::new([1u8; 16]), source, sink)
            .report_close_to(addr, closed_tx.clone());
        let handle = tokio::spawn(member.run(idle_seam()));

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(closed_rx.try_recv().is_err());
    }
}
