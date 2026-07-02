use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::sync::mpsc;

use personal_rns::interfaces::bluetooth_auto::core::{
    self, BleAddress, BleIdentity, CloseReason, Established, Handshake, HandshakeRole,
    LinkCapabilities, Local, Outcome,
};
use personal_rns::interfaces::bluetooth_auto::core::{Endpoint, L2capPlan};
use personal_rns::interfaces::bluetooth_auto::manager::{
    role_for, AdvertisingMode, ConnectionManager, ManagerAction, ManagerInput, ScanningMode,
};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};

/// The dial/suppress backoff table size for the host brain — a handful more than any host radio's
/// `MAX_PEERS`, since it tracks addresses mid-dial or cooling off, not settled peers.
const DIAL_TRACK: usize = 16;
/// Briefly keep the aggregate BLE card in `Degraded` after the last settled peer drops, so a normal
/// reconnect window does not look like BLE has gone dormant.
const RECENT_MEMBER_GRACE: Duration = Duration::from_secs(3);
use personal_rns::interfaces::{
    ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind, InterfaceStatus, TransferRates,
};
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::reactor::interface_seam::{Interface, InterfaceSeam, MAX_WIRE_FRAME_LEN};
use personal_rns::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

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

    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
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

impl<Src: BleSource, Snk: BleSink> personal_rns::interfaces::ReportsStatus
    for BluetoothPeer<Src, Snk>
{
    fn status_view(&self) -> Option<personal_rns::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![personal_rns::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

/// The driver's physical half of a settled peer — the brain owns the logical slot (keeper/address);
/// this is the fleet attachment + status the driver tears down on evict/close.
struct TokioMember {
    attached: AttachedInterface,
    status: TokioInterfaceStatus,
    address: BleAddress,
}

struct HandshakeDone<L: BleLink> {
    address: BleAddress,
    origin: Origin,
    outcome: Result<(Established, L), HandshakeFailure>,
}

type HandshakeQueue<L> = FuturesUnordered<Pin<Box<dyn Future<Output = HandshakeDone<L>>>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandshakeFailure {
    Timeout,
    InitialSend,
    Recv,
    ReplySend,
    Aborted(CloseReason),
}

enum Step<L: BleLink> {
    Event(BleEvent<L>),
    Handshake(HandshakeDone<L>),
    Closed(BleIdentity, BleAddress),
    PollStatus,
}

pub struct BluetoothAuto<B, const MAX_PEERS: usize> {
    backend: B,
    local: Local,
    status: BluetoothAutoStatus,
}

impl<B: BleBackend, const MAX_PEERS: usize> BluetoothAuto<B, MAX_PEERS> {
    pub fn new(
        backend: B,
        identity: BleIdentity,
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
    ) -> Self {
        let status = BluetoothAutoStatus::new(InterfaceId::from_channel_tag(
            InterfaceKind::BluetoothAuto,
            identity.as_bytes(),
        ));
        Self {
            backend,
            local: Local {
                identity,
                endpoint,
                capabilities,
            },
            status,
        }
    }

    #[must_use]
    pub fn status(&self) -> BluetoothAutoStatus {
        self.status.clone()
    }
}

/// The Bluetooth supervisor's aggregate live status, rendered as one "BLE" card: Dormant while the
/// radio is up and scanning with no peer linked, Live once a peer settles. The per-peer
/// [`BluetoothPeer`] members carry their own traffic on their own cards beside it.
#[derive(Clone)]
pub struct BluetoothAutoStatus {
    shared: Arc<BluetoothAutoShared>,
}

struct BluetoothAutoShared {
    id: InterfaceId,
    enabled: AtomicBool,
    up: AtomicBool,
    failed: AtomicBool,
    failure_reason: Mutex<Option<&'static str>>,
    members: Mutex<std::vec::Vec<TokioInterfaceStatus>>,
    last_member_at: Mutex<Option<Instant>>,
}

impl BluetoothAutoStatus {
    fn new(id: InterfaceId) -> Self {
        Self {
            shared: Arc::new(BluetoothAutoShared {
                id,
                enabled: AtomicBool::new(true),
                up: AtomicBool::new(false),
                failed: AtomicBool::new(false),
                failure_reason: Mutex::new(None),
                members: Mutex::new(std::vec::Vec::new()),
                last_member_at: Mutex::new(None),
            }),
        }
    }

    fn mark_up(&self) {
        self.shared.up.store(true, Ordering::Relaxed);
    }

    fn mark_failed(&self, reason: Option<&'static str>) {
        self.shared.failed.store(true, Ordering::Relaxed);
        if let Ok(mut slot) = self.shared.failure_reason.lock() {
            *slot = reason;
        }
    }

    /// Turn the Bluetooth auto-interface off or back on from the application.
    pub fn set_enabled(&self, enabled: bool) {
        self.shared.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether the supervisor should advertise, scan, and carry peers.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    fn set_members(&self, members: std::vec::Vec<TokioInterfaceStatus>) {
        if !members.is_empty() {
            if let Ok(mut last_member_at) = self.shared.last_member_at.lock() {
                *last_member_at = Some(Instant::now());
            }
        }
        if let Ok(mut slot) = self.shared.members.lock() {
            *slot = members;
        }
    }
}

impl InterfaceStatus for BluetoothAutoStatus {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            ConnectionState::Disabled
        } else if self.shared.failed.load(Ordering::Relaxed) {
            ConnectionState::Failed
        } else if !self.shared.up.load(Ordering::Relaxed) {
            ConnectionState::Initializing
        } else if self
            .shared
            .members
            .lock()
            .is_ok_and(|members| !members.is_empty())
        {
            ConnectionState::Connected
        } else if self
            .shared
            .last_member_at
            .lock()
            .ok()
            .and_then(|slot| *slot)
            .is_some_and(|last| last.elapsed() < RECENT_MEMBER_GRACE)
        {
            ConnectionState::Degraded
        } else {
            ConnectionState::Disconnected
        }
    }

    fn failure_reason(&self) -> Option<&'static str> {
        self.shared
            .failure_reason
            .lock()
            .ok()
            .and_then(|slot| *slot)
    }

    fn rx_bytes(&self) -> u64 {
        self.shared
            .members
            .lock()
            .map(|members| members.iter().map(InterfaceStatus::rx_bytes).sum())
            .unwrap_or(0)
    }

    fn tx_bytes(&self) -> u64 {
        self.shared
            .members
            .lock()
            .map(|members| members.iter().map(InterfaceStatus::tx_bytes).sum())
            .unwrap_or(0)
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        let members = self.shared.members.lock().ok()?;
        members
            .iter()
            .filter_map(InterfaceStatus::transfer_rates)
            .reduce(|acc, rates| TransferRates {
                rx_bps: acc.rx_bps.saturating_add(rates.rx_bps),
                tx_bps: acc.tx_bps.saturating_add(rates.tx_bps),
            })
    }
}

impl<B, const MAX_PEERS: usize> InterfaceSupervisor for BluetoothAuto<B, MAX_PEERS>
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
        let Self {
            mut backend,
            local,
            status,
        } = self;
        if let Some(reason) = backend.blocked() {
            status.mark_failed(Some(reason));
            std::future::pending::<()>().await;
        }
        let configured_capabilities = local.capabilities;
        let mut local = local;
        prepare_radio(&mut backend, &mut local, configured_capabilities).await;
        let started = Instant::now();
        let mut manager = ConnectionManager::<MAX_PEERS, DIAL_TRACK>::new(local);
        let mut members: HashMap<BleIdentity, TokioMember> = HashMap::new();
        let mut sighting_log: HashMap<BleAddress, Instant> = HashMap::new();
        let mut handshakes: HandshakeQueue<B::Link> = FuturesUnordered::new();
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<(BleIdentity, BleAddress)>();
        let mut pending: std::vec::Vec<ManagerAction> = std::vec::Vec::new();
        let mut status_poll = tokio::time::interval(Duration::from_millis(100));
        status.mark_up();
        manager.start(&mut |action| pending.push(action));
        apply_radio(&mut pending, &mut members, &mut backend).await;
        loop {
            if !status.is_enabled() {
                let _ = backend.set_advertising(false).await;
                let _ = backend.set_scanning(false).await;
                for (_, member) in members.drain() {
                    member.attached.teardown();
                    backend.on_link_closed(member.address).await;
                }
                handshakes = FuturesUnordered::new();
                pending.clear();
                status.set_members(std::vec::Vec::new());
                let _ = backend.set_radio_enabled(false).await;
                while !status.is_enabled() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                prepare_radio(&mut backend, &mut local, configured_capabilities).await;
                manager = ConnectionManager::<MAX_PEERS, DIAL_TRACK>::new(local);
                manager.start(&mut |action| pending.push(action));
                apply_radio(&mut pending, &mut members, &mut backend).await;
                continue;
            }
            let step = tokio::select! {
                event = backend.next_event() => Step::Event(event),
                Some(done) = handshakes.next(), if !handshakes.is_empty() => Step::Handshake(done),
                Some((identity, address)) = closed_rx.recv() => Step::Closed(identity, address),
                _ = status_poll.tick() => Step::PollStatus,
            };
            match step {
                Step::PollStatus => {}
                Step::Event(BleEvent::Sighting { address, .. }) => {
                    let now_ms = started.elapsed().as_millis() as u64;
                    let now = Instant::now();
                    if sighting_log
                        .get(&address)
                        .is_none_or(|last| now.duration_since(*last) >= Duration::from_secs(5))
                    {
                        println!("HOPSPOT_BLE_SIGHTING address={:02x?}", address.octets());
                        sighting_log.insert(address, now);
                    }
                    manager.handle(ManagerInput::Sighting { address, now_ms }, &mut |action| {
                        pending.push(action);
                    });
                    apply_radio(&mut pending, &mut members, &mut backend).await;
                }
                Step::Event(BleEvent::LinkReady {
                    link,
                    origin,
                    peer_rssi,
                }) => {
                    let address = link.address();
                    if manager.begin_handshake(origin) {
                        handshakes.push(Box::pin(run_handshake_task(
                            link,
                            role_for(origin),
                            local,
                            address,
                            origin,
                            peer_rssi,
                        )));
                    } else {
                        drop(link);
                        backend.on_link_closed(address).await;
                    }
                }
                Step::Event(BleEvent::Inbound(link)) => {
                    let address = link.address();
                    if manager.begin_handshake(Origin::Accepted) {
                        handshakes.push(Box::pin(run_handshake_task(
                            link,
                            HandshakeRole::Listener,
                            local,
                            address,
                            Origin::Accepted,
                            None,
                        )));
                    } else {
                        drop(link);
                        backend.on_link_closed(address).await;
                    }
                }
                Step::Event(BleEvent::DialFailed { address }) => {
                    let now_ms = started.elapsed().as_millis() as u64;
                    println!("HOPSPOT_BLE_DIAL_FAILED address={:02x?}", address.octets());
                    manager.handle(
                        ManagerInput::DialFailed { address, now_ms },
                        &mut |action| {
                            pending.push(action);
                        },
                    );
                    apply_radio(&mut pending, &mut members, &mut backend).await;
                }
                Step::Handshake(HandshakeDone {
                    address,
                    origin,
                    outcome,
                }) => {
                    let now_ms = started.elapsed().as_millis() as u64;
                    match outcome {
                        Ok((established, link)) => {
                            println!(
                                "HOPSPOT_BLE_SETTLED address={:02x?} origin={origin:?} endpoint={:?} identity={:02x?}",
                                address.octets(),
                                established.endpoint,
                                established.identity.as_bytes(),
                            );
                            manager.handle(
                                ManagerInput::Settled {
                                    address,
                                    origin,
                                    established,
                                    now_ms,
                                },
                                &mut |action| pending.push(action),
                            );
                            apply_settle(
                                &mut pending,
                                link,
                                &fleet,
                                &closed_tx,
                                &mut members,
                                &mut backend,
                            )
                            .await;
                        }
                        Err(reason) => {
                            println!(
                                "HOPSPOT_BLE_HANDSHAKE_FAILED address={:02x?} origin={origin:?} reason={reason:?}",
                                address.octets(),
                            );
                            manager.handle(
                                ManagerInput::HandshakeFailed { address, origin },
                                &mut |action| pending.push(action),
                            );
                            apply_radio(&mut pending, &mut members, &mut backend).await;
                        }
                    }
                }
                Step::Closed(identity, address) => {
                    println!(
                        "HOPSPOT_BLE_CLOSED address={:02x?} identity={:02x?}",
                        address.octets(),
                        identity.as_bytes(),
                    );
                    if members
                        .get(&identity)
                        .is_some_and(|member| member.address == address)
                    {
                        if let Some(member) = members.remove(&identity) {
                            member.attached.teardown();
                        }
                    }
                    manager.handle(ManagerInput::Closed { identity, address }, &mut |action| {
                        pending.push(action)
                    });
                    apply_radio(&mut pending, &mut members, &mut backend).await;
                }
            }
            status.set_members(
                members
                    .values()
                    .map(|member| member.status.clone())
                    .collect(),
            );
        }
    }
}

impl<B: BleBackend, const MAX_PEERS: usize> personal_rns::interfaces::ReportsStatus
    for BluetoothAuto<B, MAX_PEERS>
{
    fn status_view(&self) -> Option<personal_rns::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![personal_rns::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

async fn prepare_radio<B: BleBackend>(
    backend: &mut B,
    local: &mut Local,
    configured_capabilities: LinkCapabilities,
) {
    let _ = backend.set_radio_enabled(true).await;
    if let Ok(capabilities) = backend.local_capabilities(configured_capabilities).await {
        local.capabilities = capabilities;
    }
}

/// Apply the radio/fleet actions that need no link in hand — every action except `Admit`/`Reject`,
/// which are handled where the freshly-settled link is still in scope (see [`apply_settle`]).
async fn apply_one<B: BleBackend>(
    action: ManagerAction,
    members: &mut HashMap<BleIdentity, TokioMember>,
    backend: &mut B,
) {
    match action {
        ManagerAction::Dial(address) => backend.dial(address).await,
        ManagerAction::Evict { identity, .. } => {
            if let Some(member) = members.remove(&identity) {
                member.attached.teardown();
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
async fn apply_radio<B: BleBackend>(
    pending: &mut std::vec::Vec<ManagerAction>,
    members: &mut HashMap<BleIdentity, TokioMember>,
    backend: &mut B,
) {
    let actions = std::mem::take(pending);
    for action in actions {
        apply_one(action, members, backend).await;
    }
}

/// Apply the actions from a freshly-settled link: `Admit` stands it up as a fleet member over the
/// negotiated lane, `Reject` drops it (notifying the backend if we dialed); everything else routes
/// through [`apply_one`].
async fn apply_settle<B>(
    pending: &mut std::vec::Vec<ManagerAction>,
    link: B::Link,
    fleet: &Fleet,
    closed: &mpsc::UnboundedSender<(BleIdentity, BleAddress)>,
    members: &mut HashMap<BleIdentity, TokioMember>,
    backend: &mut B,
) where
    B: BleBackend,
    B::Link: 'static,
    <B::Link as BleLink>::Source: Send + 'static,
    <B::Link as BleLink>::Sink: Send + 'static,
{
    let actions = std::mem::take(pending);
    let mut link = Some(link);
    for action in actions {
        match action {
            ManagerAction::Admit {
                identity,
                address,
                lane,
                ..
            } => {
                if let Some(mut held) = link.take() {
                    arm_fast_lane(&mut held, &lane).await;
                    let (source, sink) = held.into_data();
                    let member = BluetoothPeer::new(identity, source, sink)
                        .report_close_to(address, closed.clone());
                    let status = member.status();
                    let attached = fleet.add(member);
                    members.insert(
                        identity,
                        TokioMember {
                            attached,
                            status,
                            address,
                        },
                    );
                }
            }
            ManagerAction::Reject { address, .. } => {
                link = None;
                backend.on_link_closed(address).await;
            }
            other => apply_one(other, members, backend).await,
        }
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
) -> Result<Established, HandshakeFailure> {
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        let (mut handshake, opening) = Handshake::begin(role, local, measured_rssi);
        if let Some(msg) = opening {
            link.control_send(&msg)
                .await
                .map_err(|_| HandshakeFailure::InitialSend)?;
        }
        loop {
            let msg = link
                .control_recv()
                .await
                .map_err(|_| HandshakeFailure::Recv)?;
            let reaction = handshake.absorb(msg);
            if let Some(reply) = reaction.reply {
                link.control_send(&reply)
                    .await
                    .map_err(|_| HandshakeFailure::ReplySend)?;
            }
            match reaction.outcome {
                Outcome::Settled(established) => return Ok(established),
                Outcome::Aborted(reason) => return Err(HandshakeFailure::Aborted(reason)),
                Outcome::Pending => {}
            }
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(HandshakeFailure::Timeout),
    }
}

async fn arm_fast_lane<L: BleLink>(link: &mut L, lane: &L2capPlan) {
    if matches!(lane, L2capPlan::None) {
        return;
    }
    let _ = link.upgrade(lane).await;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::mpsc;

    use super::*;
    use personal_rns::interfaces::bluetooth_auto::core::{
        arrangement, is_keeper, l2cap_plan, AndroidHost, AppleHost, BleAddress, BlueZHost, Control,
        Dialect, Psm,
    };
    use personal_rns::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};

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

    #[tokio::test]
    async fn aggregate_status_lingers_degraded_after_last_member_drops() {
        let status = BluetoothAutoStatus::new(InterfaceId::new([0xB1; 8]));
        status.mark_up();

        status.set_members(std::vec![TokioInterfaceStatus::new(
            InterfaceId::new([0xB2; 8]),
            ConnectionState::Connected,
        )]);
        assert_eq!(status.connection(), ConnectionState::Connected);

        status.set_members(std::vec::Vec::new());
        assert_eq!(status.connection(), ConnectionState::Degraded);

        tokio::time::sleep(RECENT_MEMBER_GRACE + Duration::from_millis(10)).await;
        assert_eq!(status.connection(), ConnectionState::Disconnected);
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
