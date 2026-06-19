use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::interfaces::bluetooth_auto::core::{
    self, keeps_duplicate, BleAddress, BleIdentity, Established, Handshake, HandshakeRole,
    LinkCapabilities, Local, Outcome, Transport,
};
use crate::interfaces::bluetooth_auto::seam::{BleBackend, BleEvent, BleLink, BleSink, BleSource};
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam, MAX_WIRE_FRAME_LEN};
use crate::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

struct ClosedSignal {
    address: BleAddress,
    sink: mpsc::UnboundedSender<BleAddress>,
}

pub struct BluetoothPeer<Src, Snk> {
    id: InterfaceId,
    identity: BleIdentity,
    transport: Transport,
    source: Src,
    sink: Snk,
    reachability_tag: [u8; 16],
    status: TokioInterfaceStatus,
    closed: Option<ClosedSignal>,
}

impl<Src: BleSource, Snk: BleSink> BluetoothPeer<Src, Snk> {
    pub fn new(identity: BleIdentity, transport: Transport, source: Src, sink: Snk) -> Self {
        let reachability_tag = *identity.as_bytes();
        let id =
            InterfaceId::from_reachability_tag(InterfaceKind::BluetoothPeer, &reachability_tag);
        Self {
            id,
            identity,
            transport,
            source,
            sink,
            reachability_tag,
            status: TokioInterfaceStatus::new(id, ConnectionState::Connected),
            closed: None,
        }
    }

    fn report_close_to(
        mut self,
        address: BleAddress,
        sink: mpsc::UnboundedSender<BleAddress>,
    ) -> Self {
        self.closed = Some(ClosedSignal { address, sink });
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

    #[must_use]
    pub fn transport(&self) -> Transport {
        self.transport
    }
}

impl<Src: BleSource, Snk: BleSink> Interface for BluetoothPeer<Src, Snk> {
    const HW_MTU: usize = core::BLE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::BluetoothPeer;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, core::BLE_BITRATE_GUESS_BPS)
    }

    fn reachability_tag(&self) -> &[u8] {
        &self.reachability_tag
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
        if let Some(ClosedSignal { address, sink }) = self.closed.take() {
            let _ = sink.send(address);
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

enum SupervisorStep<L> {
    Event(BleEvent<L>),
    LinkClosed(BleAddress),
}

pub struct BluetoothAuto<B> {
    backend: B,
    local: Local,
}

impl<B: BleBackend> BluetoothAuto<B> {
    pub fn new(backend: B, identity: BleIdentity, capabilities: LinkCapabilities) -> Self {
        Self {
            backend,
            local: Local {
                identity,
                capabilities,
            },
        }
    }
}

impl<B> InterfaceSupervisor for BluetoothAuto<B>
where
    B: BleBackend,
    <B::Link as BleLink>::Source: Send + 'static,
    <B::Link as BleLink>::Sink: Send + 'static,
{
    const KIND: InterfaceKind = InterfaceKind::BluetoothAuto;

    fn reachability_tag(&self) -> &[u8] {
        self.local.identity.as_bytes()
    }

    async fn run(self, fleet: Fleet) {
        let Self { mut backend, local } = self;
        let _ = backend.advertise().await;
        let mut members: HashMap<BleIdentity, AttachedInterface> = HashMap::new();
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<BleAddress>();
        loop {
            let step = tokio::select! {
                event = backend.next_event() => SupervisorStep::Event(event),
                Some(address) = closed_rx.recv() => SupervisorStep::LinkClosed(address),
            };
            match step {
                SupervisorStep::Event(BleEvent::Sighting(address)) => {
                    let Ok(mut link) = backend.dial(address).await else {
                        continue;
                    };
                    if let Some(established) =
                        drive_handshake(&mut link, HandshakeRole::Dialer, local).await
                    {
                        admit(
                            &fleet,
                            &mut members,
                            local.identity,
                            HandshakeRole::Dialer,
                            established,
                            link,
                            &closed_tx,
                        )
                        .await;
                    }
                }
                SupervisorStep::Event(BleEvent::Inbound(mut link)) => {
                    if let Some(established) =
                        drive_handshake(&mut link, HandshakeRole::Listener, local).await
                    {
                        admit(
                            &fleet,
                            &mut members,
                            local.identity,
                            HandshakeRole::Listener,
                            established,
                            link,
                            &closed_tx,
                        )
                        .await;
                    }
                }
                SupervisorStep::LinkClosed(address) => {
                    backend.on_link_closed(address).await;
                }
            }
        }
    }
}

impl<B: BleBackend> crate::interfaces::ReportsStatus for BluetoothAuto<B> {}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

async fn drive_handshake<L: BleLink>(
    link: &mut L,
    role: HandshakeRole,
    local: Local,
) -> Option<Established> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        let (mut handshake, opening) = Handshake::begin(role, local);
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

async fn admit<L>(
    fleet: &Fleet,
    members: &mut HashMap<BleIdentity, AttachedInterface>,
    ours: BleIdentity,
    role: HandshakeRole,
    established: Established,
    mut link: L,
    closed: &mpsc::UnboundedSender<BleAddress>,
) where
    L: BleLink,
    L::Source: Send + 'static,
    L::Sink: Send + 'static,
{
    if members.contains_key(&established.identity) {
        if keeps_duplicate(&ours, &established.identity, role) {
            if let Some(superseded) = members.remove(&established.identity) {
                superseded.teardown();
            }
        } else {
            return;
        }
    }
    let address = link.address();
    let _ = link.upgrade(&established.transport).await;
    let (source, sink) = link.into_data();
    let member = BluetoothPeer::new(established.identity, established.transport, source, sink)
        .report_close_to(address, closed.clone());
    let attached = fleet.add(member);
    members.insert(established.identity, attached);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::mpsc;

    use super::*;
    use crate::interfaces::bluetooth_auto::core::{BleAddress, Control, Dialect, Psm};
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};

    const TEST_FRAME_CAP: usize = 2_048;

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

        async fn upgrade(&mut self, _transport: &Transport) -> Result<(), Closed> {
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
            };
            let b = Self {
                inbound_rx: b_inbound_rx,
                peer_inbound_tx: a_inbound_tx,
                our_address: addr_b,
                peer_address: addr_a,
                sighting_pending: false,
            };
            (a, b)
        }
    }

    impl BleBackend for LoopbackBleBackend {
        const MAX_PEERS: usize = 8;
        type Error = Closed;
        type Link = LoopbackLink;

        async fn advertise(&mut self) -> Result<(), Closed> {
            Ok(())
        }

        async fn next_event(&mut self) -> BleEvent<LoopbackLink> {
            if self.sighting_pending {
                self.sighting_pending = false;
                return BleEvent::Sighting(self.peer_address);
            }
            match self.inbound_rx.recv().await {
                Some(link) => BleEvent::Inbound(link),
                None => std::future::pending().await,
            }
        }

        async fn dial(&mut self, address: BleAddress) -> Result<LoopbackLink, Closed> {
            let (mine, theirs) = link_pair(self.our_address, address);
            self.peer_inbound_tx
                .send(theirs)
                .await
                .map_err(|_| Closed)?;
            Ok(mine)
        }
    }

    #[tokio::test]
    async fn two_nodes_handshake_over_loopback_and_a_frame_crosses() {
        let local_a = Local {
            identity: BleIdentity::new([1u8; 16]),
            capabilities: caps(0x0081),
        };
        let local_b = Local {
            identity: BleIdentity::new([2u8; 16]),
            capabilities: caps(0x0082),
        };
        let (mut backend_a, mut backend_b) = LoopbackBleBackend::pair();

        let dialer = async move {
            let BleEvent::Sighting(address) = backend_a.next_event().await else {
                unreachable!("the dialer side only sights")
            };
            let mut link = backend_a.dial(address).await.unwrap();
            let established = drive_handshake(&mut link, HandshakeRole::Dialer, local_a)
                .await
                .unwrap();
            (established, link)
        };
        let listener = async move {
            let BleEvent::Inbound(mut link) = backend_b.next_event().await else {
                unreachable!("the listener side only accepts")
            };
            let established = drive_handshake(&mut link, HandshakeRole::Listener, local_b)
                .await
                .unwrap();
            (established, link)
        };
        let ((established_a, link_a), (established_b, link_b)) = tokio::join!(dialer, listener);

        assert_eq!(established_a.identity, local_b.identity);
        assert_eq!(established_b.identity, local_a.identity);
        assert_eq!(established_a.transport, established_b.transport);

        let (source_a, sink_a) = link_a.into_data();
        let (source_b, sink_b) = link_b.into_data();
        let member_a = BluetoothPeer::new(
            established_a.identity,
            established_a.transport,
            source_a,
            sink_a,
        );
        let member_b = BluetoothPeer::new(
            established_b.identity,
            established_b.transport,
            source_b,
            sink_b,
        );

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
    async fn a_gatt_only_listener_settles_the_link_on_gatt() {
        let local_a = Local {
            identity: BleIdentity::new([1u8; 16]),
            capabilities: caps(0x0081),
        };
        let local_b = Local {
            identity: BleIdentity::new([2u8; 16]),
            capabilities: LinkCapabilities {
                l2cap: None,
                link_mtu: 247,
            },
        };
        let (mut backend_a, mut backend_b) = LoopbackBleBackend::pair();

        let dialer = async move {
            let BleEvent::Sighting(address) = backend_a.next_event().await else {
                unreachable!("the dialer side only sights")
            };
            let mut link = backend_a.dial(address).await.unwrap();
            drive_handshake(&mut link, HandshakeRole::Dialer, local_a)
                .await
                .unwrap()
        };
        let listener = async move {
            let BleEvent::Inbound(mut link) = backend_b.next_event().await else {
                unreachable!("the listener side only accepts")
            };
            drive_handshake(&mut link, HandshakeRole::Listener, local_b)
                .await
                .unwrap()
        };
        let (established_a, established_b) = tokio::join!(dialer, listener);

        assert_eq!(established_a.transport, Transport::Gatt);
        assert_eq!(established_b.transport, Transport::Gatt);
    }
}
