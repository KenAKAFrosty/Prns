use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};

use prns_core::interfaces::bluetooth_auto::{
    self as contract, BleAddress, BleIdentity, CloseReason, EstablishedPeer, EstablishedTransport,
    Handshake, HandshakeOutcome, HandshakeRole, LinkCapabilities, LocalPeer, PeerProtocol,
};
use prns_core::interfaces::bluetooth_auto::{
    role_for, ConnectionPolicy, PolicyAction, PolicyInput,
};
use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin, RadioMode,
    ScanningMode,
};
use prns_core::interfaces::bluetooth_auto::{Endpoint, L2capPlan};

const DIAL_TRACK: usize = 16;
const RECENT_MEMBER_GRACE: Duration = Duration::from_secs(3);
use prns_core::interfaces::{
    ConfiguredInterfacePolicy, ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor,
    InterfaceId, InterfaceKind, InterfaceStatus, TransferRates,
};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam, MAX_WIRE_FRAME_LEN};
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

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
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
    closed: Option<ClosedSignal>,
}

impl<Src: BleSource, Snk: BleSink> BluetoothPeer<Src, Snk> {
    pub fn new(identity: BleIdentity, source: Src, sink: Snk) -> Self {
        Self::with_policy(
            identity,
            source,
            sink,
            contract::defaults_for_bitrate(contract::BLE_BITRATE_GUESS_BPS)
                .configured(ConfiguredInterfacePolicy::default()),
        )
    }

    pub fn with_policy(
        identity: BleIdentity,
        source: Src,
        sink: Snk,
        policy: EffectiveInterfacePolicy,
    ) -> Self {
        let channel_tag = *identity.as_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &channel_tag);
        Self {
            id,
            identity,
            source,
            sink,
            channel_tag,
            policy,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected),
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
    const HW_MTU: usize = contract::BLE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::BluetoothPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.policy.descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let mut buf = [0u8; MAX_WIRE_FRAME_LEN];
        loop {
            tokio::select! {
                received = self.source.recv_frame(&mut buf) => {
                    let len = match received {
                        Ok(len) => len,
                        Err(error) => {
                            crate::diagnostic_log::warn!(
                                "bluetooth: peer {:?} receive closed: {error:?}",
                                self.identity
                            );
                            break;
                        }
                    };
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
                    if let Err(error) = self.sink.send_frame(outbound).await {
                        crate::diagnostic_log::warn!(
                            "bluetooth: peer {:?} send closed: {error:?}",
                            self.identity
                        );
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

impl<Src: BleSource, Snk: BleSink> prns_core::interfaces::ReportsStatus
    for BluetoothPeer<Src, Snk>
{
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(
            self.status.clone(),
        ))
    }
}

struct TokioMember {
    attached: AttachedInterface,
    status: TokioInterfaceStatus,
    address: BleAddress,
}

struct HandshakeDone<L: BleLink> {
    address: BleAddress,
    origin: Origin,
    outcome: Result<(EstablishedPeer, L), HandshakeFailure>,
}

type HandshakeQueue<L> = FuturesUnordered<Pin<Box<dyn Future<Output = HandshakeDone<L>>>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandshakeFailure {
    Timeout,
    MissingColumbaIdentity,
    ColumbaIdentitySend,
    InitialSend,
    Recv,
    ReplySend,
    Aborted(CloseReason),
}

enum Step<L: BleLink> {
    Event(BleEvent<L>),
    Handshake(HandshakeDone<L>),
    Closed(BleIdentity, BleAddress),
    Disabled,
}

pub struct BluetoothAuto<B, const MAX_PEERS: usize> {
    backend: B,
    local: LocalPeer,
    policy: EffectiveInterfacePolicy,
    status: BluetoothAutoStatus,
}

impl<B, const MAX_PEERS: usize> BluetoothAuto<B, MAX_PEERS>
where
    B: BleBackend<MAX_PEERS>,
{
    pub fn new(
        backend: B,
        identity: BleIdentity,
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
    ) -> Self {
        Self::with_status(
            backend,
            identity,
            endpoint,
            capabilities,
            BluetoothAutoStatus::new(),
        )
    }

    #[must_use]
    pub fn with_policy(mut self, policy: EffectiveInterfacePolicy) -> Self {
        self.policy = policy;
        self
    }

    pub(crate) fn with_status(
        backend: B,
        identity: BleIdentity,
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
        status: BluetoothAutoStatus,
    ) -> Self {
        Self {
            backend,
            local: LocalPeer {
                identity,
                endpoint,
                capabilities,
            },
            policy: contract::defaults_for_bitrate(contract::BLE_BITRATE_GUESS_BPS)
                .configured(ConfiguredInterfacePolicy::default()),
            status,
        }
    }

    #[must_use]
    pub fn status(&self) -> BluetoothAutoStatus {
        self.status.clone()
    }
}

#[derive(Clone)]
pub struct BluetoothAutoStatus {
    shared: Arc<BluetoothAutoShared>,
}

struct BluetoothAutoShared {
    id: InterfaceId,
    enabled: watch::Sender<bool>,
    up: AtomicBool,
    failed: AtomicBool,
    failure_reason: Mutex<Option<&'static str>>,
    members: Mutex<std::vec::Vec<TokioInterfaceStatus>>,
    last_member_at: Mutex<Option<Instant>>,
}

impl BluetoothAutoStatus {
    pub(crate) fn new() -> Self {
        let (enabled, _) = watch::channel(true);
        Self {
            shared: Arc::new(BluetoothAutoShared {
                id: InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, contract::CHANNEL_TAG),
                enabled,
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

    pub(crate) fn mark_failed(&self, reason: Option<&'static str>) {
        self.shared.failed.store(true, Ordering::Relaxed);
        if let Ok(mut slot) = self.shared.failure_reason.lock() {
            *slot = reason;
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios", test))]
    pub(crate) fn clear_failure(&self) {
        self.shared.failed.store(false, Ordering::Relaxed);
        if let Ok(mut slot) = self.shared.failure_reason.lock() {
            *slot = None;
        }
    }

    pub fn enable(&self) {
        self.update_enabled(true);
    }

    pub fn disable(&self) {
        self.update_enabled(false);
    }

    pub fn toggle_enabled(&self) {
        self.shared.enabled.send_if_modified(|current| {
            *current = !*current;
            true
        });
    }

    fn update_enabled(&self, enabled: bool) {
        self.shared.enabled.send_if_modified(|current| {
            let changed = *current != enabled;
            *current = enabled;
            changed
        });
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        *self.shared.enabled.borrow()
    }

    async fn wait_until_enabled(&self) {
        self.wait_for_enabled_state(true).await;
    }

    async fn wait_until_disabled(&self) {
        self.wait_for_enabled_state(false).await;
    }

    async fn wait_for_enabled_state(&self, enabled: bool) {
        let mut changed = self.shared.enabled.subscribe();
        let _ = changed.wait_for(|current| *current == enabled).await;
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
    B: BleBackend<MAX_PEERS>,
    B::Link: 'static,
    <B::Link as BleLink>::Source: Send + 'static,
    <B::Link as BleLink>::Sink: Send + 'static,
{
    const KIND: InterfaceKind = InterfaceKind::BluetoothAuto;

    fn channel_tag(&self) -> &[u8] {
        contract::CHANNEL_TAG
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        let Self {
            mut backend,
            local,
            policy,
            status,
        } = self;
        if let Some(reason) = backend.blocked() {
            status.mark_failed(Some(reason));
            std::future::pending::<()>().await;
        }
        let configured_capabilities = local.capabilities;
        let mut local = local;
        prepare_radio::<B, MAX_PEERS>(&mut backend, &mut local, configured_capabilities).await;
        let started = Instant::now();
        let mut manager = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(local);
        let mut members: HashMap<BleIdentity, TokioMember> = HashMap::new();
        let mut handshakes: HandshakeQueue<B::Link> = FuturesUnordered::new();
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<(BleIdentity, BleAddress)>();
        let mut pending: std::vec::Vec<PolicyAction> = std::vec::Vec::new();
        status.mark_up();
        manager.start(&mut |action| pending.push(action));
        apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend).await;
        loop {
            if !status.is_enabled() {
                let _ = backend.set_advertising(AdvertisingMode::Off).await;
                let _ = backend.set_scanning(ScanningMode::Off).await;
                for (_, member) in members.drain() {
                    member.attached.teardown();
                    backend.on_link_closed(member.address).await;
                }
                handshakes = FuturesUnordered::new();
                pending.clear();
                status.set_members(std::vec::Vec::new());
                let _ = backend.set_radio_mode(RadioMode::Off).await;
                status.wait_until_enabled().await;
                prepare_radio::<B, MAX_PEERS>(&mut backend, &mut local, configured_capabilities)
                    .await;
                manager = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(local);
                manager.start(&mut |action| pending.push(action));
                apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend).await;
                continue;
            }
            let step = tokio::select! {
                event = backend.next_event() => Step::Event(event),
                Some(done) = handshakes.next(), if !handshakes.is_empty() => Step::Handshake(done),
                Some((identity, address)) = closed_rx.recv() => Step::Closed(identity, address),
                () = status.wait_until_disabled() => Step::Disabled,
            };
            match step {
                Step::Disabled => {}
                Step::Event(BleEvent::Sighting { address, .. }) => {
                    let now_ms = started.elapsed().as_millis() as u64;
                    manager.handle(PolicyInput::Sighting { address, now_ms }, &mut |action| {
                        pending.push(action);
                    });
                    apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend).await;
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
                    manager.handle(PolicyInput::DialFailed { address, now_ms }, &mut |action| {
                        pending.push(action);
                    });
                    apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend).await;
                }
                Step::Handshake(HandshakeDone {
                    address,
                    origin,
                    outcome,
                }) => {
                    let now_ms = started.elapsed().as_millis() as u64;
                    match outcome {
                        Ok((established, link)) => {
                            manager.handle(
                                PolicyInput::Settled {
                                    address,
                                    origin,
                                    established,
                                    now_ms,
                                },
                                &mut |action| pending.push(action),
                            );
                            apply_settle::<B, MAX_PEERS>(
                                &mut pending,
                                link,
                                &fleet,
                                &closed_tx,
                                &mut members,
                                &mut backend,
                                policy,
                            )
                            .await;
                        }
                        Err(_reason) => {
                            manager.handle(
                                PolicyInput::HandshakeFailed { address, origin },
                                &mut |action| pending.push(action),
                            );
                            apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend)
                                .await;
                        }
                    }
                }
                Step::Closed(identity, address) => {
                    if members
                        .get(&identity)
                        .is_some_and(|member| member.address == address)
                    {
                        if let Some(member) = members.remove(&identity) {
                            member.attached.teardown();
                        }
                    }
                    manager.handle(PolicyInput::Closed { identity, address }, &mut |action| {
                        pending.push(action)
                    });
                    apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend).await;
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

impl<B, const MAX_PEERS: usize> prns_core::interfaces::ReportsStatus for BluetoothAuto<B, MAX_PEERS>
where
    B: BleBackend<MAX_PEERS>,
{
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

pub(super) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

async fn prepare_radio<B, const MAX_PEERS: usize>(
    backend: &mut B,
    local: &mut LocalPeer,
    configured_capabilities: LinkCapabilities,
) where
    B: BleBackend<MAX_PEERS>,
{
    let _ = backend.set_radio_mode(RadioMode::On).await;
    if let Ok(capabilities) = backend.local_capabilities(configured_capabilities).await {
        local.capabilities = capabilities;
    }
}

async fn apply_one<B, const MAX_PEERS: usize>(
    action: PolicyAction,
    members: &mut HashMap<BleIdentity, TokioMember>,
    backend: &mut B,
) where
    B: BleBackend<MAX_PEERS>,
{
    match action {
        PolicyAction::Dial(address) => {
            let _ = backend.dial(address).await;
        }
        PolicyAction::Evict { identity, .. } => {
            if let Some(member) = members.remove(&identity) {
                member.attached.teardown();
            }
        }
        PolicyAction::NotifyClosed(address) => backend.on_link_closed(address).await,
        PolicyAction::SetAdvertising(mode) => {
            let _ = backend.set_advertising(mode).await;
        }
        PolicyAction::SetScanning(mode) => {
            let _ = backend.set_scanning(mode).await;
        }
        PolicyAction::Admit { .. } | PolicyAction::Reject { .. } => {}
    }
}

async fn apply_radio<B, const MAX_PEERS: usize>(
    pending: &mut std::vec::Vec<PolicyAction>,
    members: &mut HashMap<BleIdentity, TokioMember>,
    backend: &mut B,
) where
    B: BleBackend<MAX_PEERS>,
{
    let actions = std::mem::take(pending);
    for action in actions {
        apply_one::<B, MAX_PEERS>(action, members, backend).await;
    }
}

async fn apply_settle<B, const MAX_PEERS: usize>(
    pending: &mut std::vec::Vec<PolicyAction>,
    link: B::Link,
    fleet: &Fleet,
    closed: &mpsc::UnboundedSender<(BleIdentity, BleAddress)>,
    members: &mut HashMap<BleIdentity, TokioMember>,
    backend: &mut B,
    policy: EffectiveInterfacePolicy,
) where
    B: BleBackend<MAX_PEERS>,
    B::Link: 'static,
    <B::Link as BleLink>::Source: Send + 'static,
    <B::Link as BleLink>::Sink: Send + 'static,
{
    let actions = std::mem::take(pending);
    let mut link = Some(link);
    for action in actions {
        match action {
            PolicyAction::Admit {
                identity,
                address,
                lane,
                ..
            } => {
                if let Some(mut held) = link.take() {
                    arm_fast_lane(&mut held, &lane).await;
                    let (source, sink) = held.into_data();
                    let member = BluetoothPeer::with_policy(identity, source, sink, policy)
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
            PolicyAction::Reject { address, .. } => {
                link = None;
                backend.on_link_closed(address).await;
            }
            other => apply_one::<B, MAX_PEERS>(other, members, backend).await,
        }
    }
}

async fn run_handshake_task<L: BleLink>(
    mut link: L,
    role: HandshakeRole,
    local: LocalPeer,
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
    local: LocalPeer,
    measured_rssi: Option<i8>,
) -> Result<EstablishedPeer, HandshakeFailure> {
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        if link.peer_protocol() == PeerProtocol::Columba {
            let identity = link
                .receive_columba_peer_identity()
                .await
                .map_err(|_| HandshakeFailure::MissingColumbaIdentity)?;
            if role == HandshakeRole::Dialer {
                link.send_columba_identity(local.identity)
                    .await
                    .map_err(|_| HandshakeFailure::ColumbaIdentitySend)?;
            }
            return Ok(EstablishedPeer {
                identity,
                transport: EstablishedTransport::ColumbaGatt,
                peer_rssi: measured_rssi,
            });
        }
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
                HandshakeOutcome::Settled(established) => return Ok(established),
                HandshakeOutcome::Aborted(reason) => return Err(HandshakeFailure::Aborted(reason)),
                HandshakeOutcome::Pending => {}
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

    use tokio::sync::{mpsc, oneshot};

    use super::*;
    use prns_core::interfaces::bluetooth_auto::{
        is_keeper, l2cap_arrangement, l2cap_plan, AndroidHost, AppleHost, BleAddress, BlueZHost,
        Control, PeerProtocol, Psm,
    };
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};

    const TEST_FRAME_CAP: usize = 2_048;

    fn mac() -> Endpoint {
        Endpoint::CoreBluetooth(AppleHost::MacOs)
    }

    fn native_transport(established: EstablishedPeer) -> (Endpoint, LinkCapabilities) {
        let EstablishedTransport::Native {
            endpoint,
            capabilities,
        } = established.transport
        else {
            unreachable!("native loopback settles as native")
        };
        (endpoint, capabilities)
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
        let status = BluetoothAutoStatus::new();
        status.mark_up();

        status.set_members(std::vec![TokioInterfaceStatus::new_unaccounted(
            InterfaceId::new([0xB2; 8]),
            ConnectionState::Connected,
        )]);
        assert_eq!(status.connection(), ConnectionState::Connected);

        status.set_members(std::vec::Vec::new());
        assert_eq!(status.connection(), ConnectionState::Degraded);

        tokio::time::sleep(RECENT_MEMBER_GRACE + Duration::from_millis(10)).await;
        assert_eq!(status.connection(), ConnectionState::Disconnected);
    }

    #[test]
    fn asynchronous_readiness_can_clear_a_transient_bluetooth_failure() {
        let status = BluetoothAutoStatus::new();
        status.mark_failed(Some("Bluetooth not granted or radio unavailable"));
        assert_eq!(status.connection(), ConnectionState::Failed);
        assert_eq!(
            status.failure_reason(),
            Some("Bluetooth not granted or radio unavailable")
        );

        status.clear_failure();
        status.mark_up();
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert_eq!(status.failure_reason(), None);
    }

    struct MockSeam {
        inbound: mpsc::UnboundedSender<std::vec::Vec<u8>>,
        sink: std::vec::Vec<u8>,
        outbound: TokioGrantConsumer,
    }

    use prns_core::interfaces::FrameSink;

    impl InterfaceSeam for MockSeam {
        fn fill_entropy(&mut self, bytes: &mut [u8]) {
            bytes.fill(0);
        }

        async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
            &mut self.sink
        }

        async fn commit_inbound(&mut self) {
            if !self.sink.is_empty() {
                let _ = self.inbound.send(std::mem::take(&mut self.sink));
            }
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

    struct BlockingSink {
        started: Option<oneshot::Sender<()>>,
    }

    struct ColumbaLink {
        address: BleAddress,
        peer_identity: Option<BleIdentity>,
        sent_identity: Option<BleIdentity>,
    }

    impl BleLink for ColumbaLink {
        type Error = Closed;
        type Source = LoopbackSource;
        type Sink = LoopbackSink;

        fn peer_protocol(&self) -> PeerProtocol {
            PeerProtocol::Columba
        }

        fn address(&self) -> BleAddress {
            self.address
        }

        async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, Closed> {
            self.peer_identity.ok_or(Closed)
        }

        async fn send_columba_identity(&mut self, identity: BleIdentity) -> Result<(), Closed> {
            self.sent_identity = Some(identity);
            Ok(())
        }

        async fn control_send(&mut self, _msg: &Control) -> Result<(), Closed> {
            Err(Closed)
        }

        async fn control_recv(&mut self) -> Result<Control, Closed> {
            Err(Closed)
        }

        async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), Closed> {
            Ok(())
        }

        fn into_data(self) -> (LoopbackSource, LoopbackSink) {
            let (data_tx, data_rx) = mpsc::channel(1);
            (LoopbackSource { data_rx }, LoopbackSink { data_tx })
        }
    }

    impl BleLink for LoopbackLink {
        type Error = Closed;
        type Source = LoopbackSource;
        type Sink = LoopbackSink;

        fn peer_protocol(&self) -> PeerProtocol {
            PeerProtocol::Native
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

    #[tokio::test]
    async fn a_columba_dialer_sends_its_identity_and_settles_without_native_metadata() {
        let local = LocalPeer {
            identity: BleIdentity::new([1; 16]),
            endpoint: linux(),
            capabilities: caps(0x0080),
        };
        let peer_identity = BleIdentity::new([2; 16]);
        let mut link = ColumbaLink {
            address: BleAddress::new([3; 6]),
            peer_identity: Some(peer_identity),
            sent_identity: None,
        };

        let established = drive_handshake(&mut link, HandshakeRole::Dialer, local, Some(-42))
            .await
            .unwrap();

        assert_eq!(
            established,
            EstablishedPeer {
                identity: peer_identity,
                transport: EstablishedTransport::ColumbaGatt,
                peer_rssi: Some(-42),
            }
        );
        assert_eq!(link.sent_identity, Some(local.identity));
    }

    #[tokio::test]
    async fn a_columba_listener_uses_the_received_identity_without_writing_one() {
        let local = LocalPeer {
            identity: BleIdentity::new([1; 16]),
            endpoint: android(),
            capabilities: caps(0x0080),
        };
        let peer_identity = BleIdentity::new([2; 16]);
        let mut link = ColumbaLink {
            address: BleAddress::new([3; 6]),
            peer_identity: Some(peer_identity),
            sent_identity: None,
        };

        let established = drive_handshake(&mut link, HandshakeRole::Listener, local, None)
            .await
            .unwrap();

        assert_eq!(established.identity, peer_identity);
        assert_eq!(established.transport, EstablishedTransport::ColumbaGatt);
        assert_eq!(link.sent_identity, None);
    }

    impl BleSink for LoopbackSink {
        type Error = Closed;

        async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Closed> {
            self.data_tx.send(frame.to_vec()).await.map_err(|_| Closed)
        }
    }

    impl BleSink for BlockingSink {
        type Error = Closed;

        async fn send_frame(&mut self, _frame: &[u8]) -> Result<(), Closed> {
            if let Some(started) = self.started.take() {
                let _ = started.send(());
            }
            std::future::pending().await
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

    impl BleBackend<8> for LoopbackBleBackend {
        type Error = Closed;
        type Link = LoopbackLink;

        async fn set_advertising(&mut self, _mode: AdvertisingMode) -> Result<(), Closed> {
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

        async fn dial(
            &mut self,
            address: BleAddress,
        ) -> prns_core::interfaces::bluetooth_auto::DialOutcome {
            let (mine, theirs) = link_pair(self.our_address, address);
            if self.peer_inbound_tx.send(theirs).await.is_ok() {
                self.dialed = Some(mine);
                prns_core::interfaces::bluetooth_auto::DialOutcome::Started
            } else {
                prns_core::interfaces::bluetooth_auto::DialOutcome::Busy
            }
        }
    }

    #[tokio::test]
    async fn two_nodes_handshake_over_loopback_and_a_frame_crosses() {
        let local_a = LocalPeer {
            identity: BleIdentity::new([1u8; 16]),
            endpoint: mac(),
            capabilities: caps(0x0081),
        };
        let local_b = LocalPeer {
            identity: BleIdentity::new([2u8; 16]),
            endpoint: linux(),
            capabilities: caps(0x0082),
        };
        let (mut backend_a, mut backend_b) = LoopbackBleBackend::pair();

        let dialer = async move {
            let BleEvent::Sighting { address, .. } = backend_a.next_event().await else {
                unreachable!("the dialer side only sights")
            };
            assert_eq!(
                backend_a.dial(address).await,
                prns_core::interfaces::bluetooth_auto::DialOutcome::Started
            );
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
        let (endpoint_a, _) = native_transport(established_a);
        let (endpoint_b, _) = native_transport(established_b);

        assert_eq!(established_a.identity, local_b.identity);
        assert_eq!(endpoint_a, local_b.endpoint);
        assert_eq!(established_b.identity, local_a.identity);
        assert_eq!(endpoint_b, local_a.endpoint);

        let (source_a, sink_a) = link_a.into_data();
        let (source_b, sink_b) = link_b.into_data();
        let member_a = BluetoothPeer::new(established_a.identity, source_a, sink_a);
        let member_b = BluetoothPeer::new(established_b.identity, source_b, sink_b);

        let (discard_a, _discard_a_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_a_producer, out_a_consumer) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let seam_a = MockSeam {
            inbound: discard_a,
            sink: std::vec::Vec::new(),
            outbound: out_a_consumer,
        };

        let (capture_tx, mut capture_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (_idle_b_producer, idle_b_consumer) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let seam_b = MockSeam {
            inbound: capture_tx,
            sink: std::vec::Vec::new(),
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
    async fn a_blocked_peer_does_not_stall_another_peer() {
        let (slow_data_tx, slow_data_rx) = mpsc::channel(1);
        let _keep_slow_source_alive = slow_data_tx;
        let (slow_started_tx, slow_started_rx) = oneshot::channel();
        let slow_peer = BluetoothPeer::new(
            BleIdentity::new([1; 16]),
            LoopbackSource {
                data_rx: slow_data_rx,
            },
            BlockingSink {
                started: Some(slow_started_tx),
            },
        );
        let (slow_discard_tx, _slow_discard_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut slow_outbound_tx, slow_outbound_rx) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let slow_seam = MockSeam {
            inbound: slow_discard_tx,
            sink: std::vec::Vec::new(),
            outbound: slow_outbound_rx,
        };
        let slow_task = tokio::spawn(slow_peer.run(slow_seam));

        slow_outbound_tx.try_grant().unwrap().fill(&[0xAA]);
        slow_outbound_tx.commit();
        tokio::time::timeout(Duration::from_secs(1), slow_started_rx)
            .await
            .unwrap()
            .unwrap();

        let (fast_data_tx, fast_data_rx) = mpsc::channel(1);
        let (fast_sink_tx, mut fast_sink_rx) = mpsc::channel(1);
        let fast_peer = BluetoothPeer::new(
            BleIdentity::new([2; 16]),
            LoopbackSource {
                data_rx: fast_data_rx,
            },
            LoopbackSink {
                data_tx: fast_sink_tx,
            },
        );
        let (fast_capture_tx, mut fast_capture_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (_fast_outbound_tx, fast_outbound_rx) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let fast_seam = MockSeam {
            inbound: fast_capture_tx,
            sink: std::vec::Vec::new(),
            outbound: fast_outbound_rx,
        };
        let fast_task = tokio::spawn(fast_peer.run(fast_seam));

        fast_data_tx.send(vec![0x10, 0x20]).await.unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), fast_capture_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(received, [0x10, 0x20]);
        assert!(fast_sink_rx.try_recv().is_err());
        assert!(!slow_task.is_finished());

        slow_task.abort();
        fast_task.abort();
    }

    #[test]
    fn configured_policy_reaches_a_bluetooth_auto_peer_descriptor() {
        let identity = BleIdentity::new([0x44; 16]);
        let (link, _peer) = link_pair(BleAddress::new([0x11; 6]), BleAddress::new([0x22; 6]));
        let (source, sink) = link.into_data();
        let policy = contract::defaults_for_bitrate(contract::BLE_BITRATE_GUESS_BPS).configured(
            ConfiguredInterfacePolicy {
                mode: Some(prns_core::interfaces::InterfaceMode::AccessPoint),
                bitrate: Some(prns_core::interfaces::BitrateBps::guess(7_654_321)),
                ..ConfiguredInterfacePolicy::default()
            },
        );
        let member = BluetoothPeer::with_policy(identity, source, sink, policy);

        assert_eq!(member.descriptor(), policy.descriptor(member.id()));
    }

    #[tokio::test]
    async fn a_gatt_only_listener_settles_the_link_on_the_floor() {
        let local_a = LocalPeer {
            identity: BleIdentity::new([1u8; 16]),
            endpoint: mac(),
            capabilities: caps(0x0081),
        };
        let local_b = LocalPeer {
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
            assert_eq!(
                backend_a.dial(address).await,
                prns_core::interfaces::bluetooth_auto::DialOutcome::Started
            );
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
        let (endpoint_a, capabilities_a) = native_transport(established_a);
        let (endpoint_b, capabilities_b) = native_transport(established_b);

        let arr_a = l2cap_arrangement(local_a.endpoint, endpoint_a);
        let arr_b = l2cap_arrangement(local_b.endpoint, endpoint_b);
        assert_eq!(
            l2cap_plan(
                arr_a,
                HandshakeRole::Dialer,
                local_a.endpoint,
                &local_a.capabilities,
                &capabilities_a,
            ),
            L2capPlan::None
        );
        assert_eq!(
            l2cap_plan(
                arr_b,
                HandshakeRole::Listener,
                local_b.endpoint,
                &local_b.capabilities,
                &capabilities_b,
            ),
            L2capPlan::None
        );
    }

    #[tokio::test]
    async fn an_android_peer_is_the_l2cap_opener_and_the_mac_accepts() {
        let mac_local = LocalPeer {
            identity: BleIdentity::new([1u8; 16]),
            endpoint: mac(),
            capabilities: caps(0x00c0),
        };
        let android_local = LocalPeer {
            identity: BleIdentity::new([2u8; 16]),
            endpoint: android(),
            capabilities: caps(0x0080),
        };
        let (mut mac_backend, mut android_backend) = LoopbackBleBackend::pair();

        let mac_side = async move {
            let BleEvent::Sighting { address, .. } = mac_backend.next_event().await else {
                unreachable!("mac sights android")
            };
            assert_eq!(
                mac_backend.dial(address).await,
                prns_core::interfaces::bluetooth_auto::DialOutcome::Started
            );
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
        let (mac_endpoint, mac_capabilities) = native_transport(mac_established);
        let (_, android_capabilities) = native_transport(android_established);

        let arr = l2cap_arrangement(mac_local.endpoint, mac_endpoint);
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
                &android_capabilities,
            ),
            L2capPlan::None
        );
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Dialer,
                mac_local.endpoint,
                &mac_local.capabilities,
                &mac_capabilities,
            ),
            L2capPlan::Accept
        );
    }

    fn idle_seam() -> MockSeam {
        let (discard, _discard_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (_idle_producer, idle_consumer) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        MockSeam {
            inbound: discard,
            sink: std::vec::Vec::new(),
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
