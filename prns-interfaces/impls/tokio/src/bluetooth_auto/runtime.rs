use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch, Mutex as AsyncMutex};
use tokio::time::Instant;

use prns_core::interfaces::bluetooth_auto::{
    self as contract, BleAddress, BleIdentity, CloseReason, DiscoveryGroupSet, EstablishedPeer,
    EstablishedTransport, Handshake, HandshakeFailureKind, HandshakeOutcome, HandshakeRole,
    LinkCapabilities, LocalPeer, PeerProtocol,
};
use prns_core::interfaces::bluetooth_auto::{
    role_for, ConnectionPolicy, PolicyAction, PolicyInput,
};
use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, BluetoothRadioState,
    Origin, RadioMode, ScanningMode,
};
use prns_core::interfaces::bluetooth_auto::{Endpoint, L2capPlan};

const DIAL_TRACK: usize = 16;
const RECENT_MEMBER_GRACE: Duration = Duration::from_secs(3);
use prns_core::interfaces::{
    ConfiguredInterfacePolicy, ConnectionState, DiscoveryGroupApplyOutcome,
    EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceStatus,
    TransferRates, DEFAULT_DISCOVERY_GROUP_HASH,
};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{
    Interface, InterfaceSeam, OutboundDisposition, OutboundDropReason,
};
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

use contract::{send_frame_duplex, BleDuplexOutcome, BleFrameForwarder};

struct PeerInbound<'a, Seam> {
    seam: &'a mut Seam,
    status: &'a TokioInterfaceStatus,
}

impl<Seam: InterfaceSeam> BleFrameForwarder for PeerInbound<'_, Seam> {
    type Error = core::convert::Infallible;

    async fn forward(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        self.status.add_rx(frame.len() as u64);
        self.seam.next_inbound(frame).await;
        Ok(())
    }
}

struct ClosedSignal {
    identity: BleIdentity,
    address: BleAddress,
    sink: mpsc::UnboundedSender<PeerClosed>,
}

struct PeerClosed {
    identity: BleIdentity,
    address: BleAddress,
    status: TokioInterfaceStatus,
}

impl PeerClosed {
    fn matches(&self, address: BleAddress, status: &TokioInterfaceStatus) -> bool {
        self.address == address && self.status.same_instance(status)
    }
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
        sink: mpsc::UnboundedSender<PeerClosed>,
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
        let mut buf = [0u8; contract::BLE_WIRE_FRAME_LEN];
        let mut pending_outbound = [0u8; contract::BLE_WIRE_FRAME_LEN];
        loop {
            tokio::select! {
                received = contract::receive_frame(&mut self.source, &mut buf) => {
                    let frame = match received {
                        Ok(frame) => frame,
                        Err(error) => {
                            crate::diagnostic_log::warn!(
                                "bluetooth: peer {:?} receive closed: {error:?}",
                                self.identity
                            );
                            break;
                        }
                    };
                    if frame.is_empty() {
                        continue;
                    }
                    self.status.add_rx(frame.len() as u64);
                    seam.next_inbound(frame).await;
                }
                outbound = seam.next_outbound() => {
                    if outbound.is_empty() {
                        continue;
                    }
                    let outbound_len = outbound.len();
                    if outbound_len > pending_outbound.len() {
                        seam.complete_outbound(OutboundDisposition::Dropped(OutboundDropReason::Rejected));
                        break;
                    }
                    pending_outbound[..outbound_len].copy_from_slice(outbound);
                    seam.accept_outbound_custody();
                    let result = match send_frame_duplex(
                        &mut self.source, &mut self.sink,
                        &pending_outbound[..outbound_len], &mut buf,
                        PeerInbound { seam: &mut seam, status: &self.status },
                    ).await {
                        BleDuplexOutcome::Finished(result) => result,
                        BleDuplexOutcome::ForwardFailed(never) => match never {},
                        BleDuplexOutcome::ReceiveFailed(error) => {
                            seam.complete_outbound(OutboundDisposition::Dropped(OutboundDropReason::TransportFailure));
                            crate::diagnostic_log::warn!(
                                "bluetooth: peer {:?} receive closed during send: {error:?}",
                                self.identity
                            );
                            break;
                        }
                        BleDuplexOutcome::InvalidReceiveLength(length) => {
                            seam.complete_outbound(OutboundDisposition::Dropped(OutboundDropReason::TransportFailure));
                            crate::diagnostic_log::warn!(
                                "bluetooth: peer {:?} reported invalid receive length {length} during send",
                                self.identity
                            );
                            break;
                        }
                    };
                    if let Err(error) = result {
                        seam.complete_outbound(OutboundDisposition::Dropped(OutboundDropReason::TransportFailure));
                        crate::diagnostic_log::warn!(
                            "bluetooth: peer {:?} send closed: {error:?}",
                            self.identity
                        );
                        break;
                    }
                    self.status.add_tx(outbound_len as u64);
                    seam.complete_outbound(OutboundDisposition::Sent);
                }
            }
        }
        let closed = self
            .closed
            .take()
            .map(|signal| (signal, self.status.clone()));
        drop(self);
        if let Some((
            ClosedSignal {
                identity,
                address,
                sink,
            },
            status,
        )) = closed
        {
            let _ = sink.send(PeerClosed {
                identity,
                address,
                status,
            });
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
    Closed(PeerClosed),
    Disabled,
    DiscoveryGroups(DiscoveryGroupSet),
}

pub struct BluetoothAuto<B, const MAX_PEERS: usize, S = super::TokioFairBleEvents> {
    backend: B,
    local: LocalPeer,
    policy: EffectiveInterfacePolicy,
    status: BluetoothAutoStatus,
    event_selector: S,
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

    pub(crate) fn with_status(
        backend: B,
        identity: BleIdentity,
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
        status: BluetoothAutoStatus,
    ) -> Self {
        let discovery_groups = status.discovery_groups();
        Self {
            backend,
            local: LocalPeer {
                identity,
                endpoint,
                capabilities,
                discovery_groups: discovery_groups.hashes(),
            },
            policy: contract::defaults_for_bitrate(contract::BLE_BITRATE_GUESS_BPS)
                .configured(ConfiguredInterfacePolicy::default()),
            status,
            event_selector: super::TokioFairBleEvents,
        }
    }
}

impl<B, const MAX_PEERS: usize, S> BluetoothAuto<B, MAX_PEERS, S> {
    #[must_use]
    pub fn with_policy(mut self, policy: EffectiveInterfacePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Replaces only supervisor arbitration; backend, identity and status stay owned
    /// by this instance. Ordinary constructors retain Tokio's fair selection.
    #[must_use]
    pub fn with_event_selector<N: super::BleEventSelector>(
        self,
        event_selector: N,
    ) -> BluetoothAuto<B, MAX_PEERS, N> {
        BluetoothAuto {
            backend: self.backend,
            local: self.local,
            policy: self.policy,
            status: self.status,
            event_selector,
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

#[derive(Clone, Copy)]
struct BluetoothEnableState {
    enabled: bool,
    generation: u64,
    disabled: bool,
}

impl BluetoothEnableState {
    fn replace(&mut self, enabled: bool) -> bool {
        if self.enabled == enabled {
            return false;
        }
        self.enabled = enabled;
        self.generation = self.generation.wrapping_add(1);
        self.disabled = false;
        true
    }
}

struct BluetoothAutoShared {
    id: InterfaceId,
    enabled: watch::Sender<BluetoothEnableState>,
    discovery_groups: watch::Sender<DiscoveryGroupSet>,
    applied_discovery_groups: watch::Sender<DiscoveryGroupSet>,
    discovery_group_replacement: AsyncMutex<()>,
    up: AtomicBool,
    failed: AtomicBool,
    failure_reason: Mutex<Option<&'static str>>,
    radio_state_source: Mutex<Option<Arc<dyn Fn() -> BluetoothRadioState + Send + Sync>>>,
    members: Mutex<std::vec::Vec<TokioInterfaceStatus>>,
    last_member_at: Mutex<Option<Instant>>,
}

impl BluetoothAutoStatus {
    pub(crate) fn new() -> Self {
        Self::new_with_discovery_groups(DiscoveryGroupSet::reticulum())
    }

    pub(crate) fn new_with_discovery_groups(discovery_groups: DiscoveryGroupSet) -> Self {
        let (enabled, _) = watch::channel(BluetoothEnableState {
            enabled: true,
            generation: 0,
            disabled: false,
        });
        let (groups, _) = watch::channel(discovery_groups);
        let (applied_groups, _) = watch::channel(discovery_groups);
        Self {
            shared: Arc::new(BluetoothAutoShared {
                id: InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, contract::GROUP_ID),
                enabled,
                discovery_groups: groups,
                applied_discovery_groups: applied_groups,
                discovery_group_replacement: AsyncMutex::new(()),
                up: AtomicBool::new(false),
                failed: AtomicBool::new(false),
                failure_reason: Mutex::new(None),
                radio_state_source: Mutex::new(None),
                members: Mutex::new(std::vec::Vec::new()),
                last_member_at: Mutex::new(None),
            }),
        }
    }

    fn mark_up(&self) {
        self.shared.up.store(true, Ordering::Relaxed);
    }

    /// Returns the platform's current observation, independently of the requested enabled state.
    /// A backend without a live radio observation reports `Unknown`; an idle interface alone is
    /// not evidence that the radio is powered on.
    #[must_use]
    pub fn radio_state(&self) -> BluetoothRadioState {
        self.shared
            .radio_state_source
            .lock()
            .ok()
            .and_then(|source| source.as_ref().map(|read| read()))
            .unwrap_or_default()
    }

    #[cfg(any(
        test,
        all(
            feature = "bluetooth-auto",
            any(target_os = "macos", target_os = "ios")
        )
    ))]
    pub(crate) fn observe_radio_state(
        &self,
        source: impl Fn() -> BluetoothRadioState + Send + Sync + 'static,
    ) {
        if let Ok(mut slot) = self.shared.radio_state_source.lock() {
            *slot = Some(Arc::new(source));
        }
    }

    pub(crate) fn mark_failed(&self, reason: Option<&'static str>) {
        self.shared.failed.store(true, Ordering::Relaxed);
        if let Ok(mut slot) = self.shared.failure_reason.lock() {
            *slot = reason;
        }
    }

    #[cfg(any(
        test,
        all(
            feature = "bluetooth-auto",
            any(target_os = "macos", target_os = "ios")
        )
    ))]
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
        self.shared
            .enabled
            .send_if_modified(|current| current.replace(!current.enabled));
    }

    #[must_use]
    pub fn discovery_groups(&self) -> DiscoveryGroupSet {
        *self.shared.discovery_groups.borrow()
    }

    /// Requests an atomic replacement. The running supervisor acknowledges it only after all old
    /// peers and handshakes have been torn down and discovery has restarted with the new set.
    pub async fn replace_discovery_groups(
        &self,
        groups: DiscoveryGroupSet,
    ) -> DiscoveryGroupApplyOutcome {
        let _replacement = self.shared.discovery_group_replacement.lock().await;
        if self.discovery_groups() == groups {
            return DiscoveryGroupApplyOutcome::Unchanged;
        }
        let mut applied = self.shared.applied_discovery_groups.subscribe();
        self.shared.discovery_groups.send_replace(groups);
        if !self.is_enabled() || !self.shared.up.load(Ordering::Acquire) {
            self.shared.applied_discovery_groups.send_replace(groups);
            return DiscoveryGroupApplyOutcome::Applied;
        }
        loop {
            if *applied.borrow_and_update() == groups {
                return DiscoveryGroupApplyOutcome::Applied;
            }
            if applied.changed().await.is_err() {
                return DiscoveryGroupApplyOutcome::Failed;
            }
        }
    }

    async fn wait_for_discovery_groups_change(
        &self,
        current: &DiscoveryGroupSet,
    ) -> DiscoveryGroupSet {
        let mut changed = self.shared.discovery_groups.subscribe();
        loop {
            let candidate = *changed.borrow_and_update();
            if &candidate != current {
                return candidate;
            }
            let _ = changed.changed().await;
        }
    }

    fn acknowledge_discovery_groups(&self, groups: DiscoveryGroupSet) {
        self.shared.applied_discovery_groups.send_replace(groups);
    }

    fn update_enabled(&self, enabled: bool) {
        self.shared
            .enabled
            .send_if_modified(|current| current.replace(enabled));
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.borrow().enabled
    }

    /// Whether the supervisor completed shutdown for the current disabled request. This does not
    /// imply that the operating system's Bluetooth radio is powered off.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.shared.enabled.borrow().disabled
    }

    pub(super) fn disabled_request(&self) -> Option<u64> {
        let current = self.shared.enabled.borrow();
        (!current.enabled).then_some(current.generation)
    }

    pub(super) fn acknowledge_disabled(&self, generation: u64) {
        self.shared.enabled.send_if_modified(|current| {
            if current.enabled || current.generation != generation || current.disabled {
                return false;
            }
            current.disabled = true;
            true
        });
    }

    pub(super) async fn wait_for_enabled_change(&self, generation: u64) {
        let mut changed = self.shared.enabled.subscribe();
        let _ = changed
            .wait_for(|current| current.generation != generation)
            .await;
    }

    pub(super) async fn wait_until_disabled(&self) {
        self.wait_for_enabled_state(false).await;
    }

    async fn wait_for_enabled_state(&self, enabled: bool) {
        let mut changed = self.shared.enabled.subscribe();
        let _ = changed.wait_for(|current| current.enabled == enabled).await;
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

impl<B, const MAX_PEERS: usize, S: super::BleEventSelector + 'static> InterfaceSupervisor
    for BluetoothAuto<B, MAX_PEERS, S>
where
    B: BleBackend<MAX_PEERS>,
    B::Link: 'static,
    <B::Link as BleLink>::Source: Send + 'static,
    <B::Link as BleLink>::Sink: Send + 'static,
{
    const KIND: InterfaceKind = InterfaceKind::BluetoothAuto;

    fn channel_tag(&self) -> &[u8] {
        contract::GROUP_ID
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
            mut event_selector,
        } = self;
        if let Some(reason) = backend.blocked() {
            status.mark_failed(Some(reason));
            std::future::pending::<()>().await;
        }
        let configured_capabilities = local.capabilities;
        let mut local = local;
        let mut discovery_groups = status.discovery_groups();
        while let Some(generation) = status.disabled_request() {
            let advertising_stopped = backend.set_advertising(AdvertisingMode::Off).await.is_ok();
            let scanning_stopped = backend.set_scanning(ScanningMode::Off).await.is_ok();
            let radio_stopped = backend.set_radio_mode(RadioMode::Off).await.is_ok();
            status.mark_up();
            if advertising_stopped && scanning_stopped && radio_stopped {
                status.acknowledge_disabled(generation);
            }
            status.wait_for_enabled_change(generation).await;
            discovery_groups = status.discovery_groups();
            local.discovery_groups = discovery_groups.hashes();
        }
        prepare_radio::<B, MAX_PEERS>(&mut backend, &mut local, configured_capabilities).await;
        let started = Instant::now();
        let mut manager = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(local);
        let mut members: HashMap<BleIdentity, TokioMember> = HashMap::new();
        let mut handshakes: HandshakeQueue<B::Link> = FuturesUnordered::new();
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<PeerClosed>();
        let mut pending: std::vec::Vec<PolicyAction> = std::vec::Vec::new();
        status.mark_up();
        manager.start(&mut |action| pending.push(action));
        apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend).await;
        loop {
            if let Some(generation) = status.disabled_request() {
                let advertising_stopped =
                    backend.set_advertising(AdvertisingMode::Off).await.is_ok();
                let scanning_stopped = backend.set_scanning(ScanningMode::Off).await.is_ok();
                for (_, member) in members.drain() {
                    member.attached.teardown();
                    backend.on_link_closed(member.address).await;
                }
                handshakes = FuturesUnordered::new();
                pending.clear();
                status.set_members(std::vec::Vec::new());
                let radio_stopped = backend.set_radio_mode(RadioMode::Off).await.is_ok();
                if advertising_stopped && scanning_stopped && radio_stopped {
                    status.acknowledge_disabled(generation);
                }
                status.wait_for_enabled_change(generation).await;
                if !status.is_enabled() {
                    continue;
                }
                prepare_radio::<B, MAX_PEERS>(&mut backend, &mut local, configured_capabilities)
                    .await;
                manager = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(local);
                manager.start(&mut |action| pending.push(action));
                apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend).await;
                continue;
            }
            let step = event_selector
                .select(super::BleEventSources {
                    backend: async { Step::Event(backend.next_event().await) },
                    handshake: async {
                        if let Some(done) = handshakes.next().await {
                            return Step::Handshake(done);
                        }
                        std::future::pending().await
                    },
                    closed: async {
                        if let Some(closed) = closed_rx.recv().await {
                            return Step::Closed(closed);
                        }
                        std::future::pending().await
                    },
                    disabled: async {
                        status.wait_until_disabled().await;
                        Step::Disabled
                    },
                    groups: async {
                        Step::DiscoveryGroups(
                            status
                                .wait_for_discovery_groups_change(&discovery_groups)
                                .await,
                        )
                    },
                })
                .await;
            match step {
                Step::Disabled => {}
                Step::DiscoveryGroups(groups) => {
                    let _ = backend.set_advertising(AdvertisingMode::Off).await;
                    let _ = backend.set_scanning(ScanningMode::Off).await;
                    for (_, member) in members.drain() {
                        member.attached.teardown();
                        backend.on_link_closed(member.address).await;
                    }
                    handshakes = FuturesUnordered::new();
                    pending.clear();
                    status.set_members(std::vec::Vec::new());
                    discovery_groups = groups;
                    local.discovery_groups = groups.hashes();
                    manager = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(local);
                    manager.start(&mut |action| pending.push(action));
                    apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend).await;
                    status.acknowledge_discovery_groups(groups);
                }
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
                        Err(reason) => {
                            let failure = match reason {
                                HandshakeFailure::Aborted(CloseReason::Incompatible) => {
                                    HandshakeFailureKind::Incompatible
                                }
                                HandshakeFailure::Timeout
                                | HandshakeFailure::MissingColumbaIdentity
                                | HandshakeFailure::ColumbaIdentitySend
                                | HandshakeFailure::InitialSend
                                | HandshakeFailure::Recv
                                | HandshakeFailure::ReplySend
                                | HandshakeFailure::Aborted(
                                    CloseReason::SelfConnection | CloseReason::DuplicateLink,
                                ) => HandshakeFailureKind::Other,
                            };
                            manager.handle(
                                PolicyInput::HandshakeFailed {
                                    address,
                                    origin,
                                    failure,
                                    now_ms,
                                },
                                &mut |action| pending.push(action),
                            );
                            apply_radio::<B, MAX_PEERS>(&mut pending, &mut members, &mut backend)
                                .await;
                        }
                    }
                }
                Step::Closed(closed) => {
                    if !close_member(closed, &mut members, &mut manager, &mut pending) {
                        continue;
                    }
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

impl<B, const MAX_PEERS: usize, S> prns_core::interfaces::ReportsStatus
    for BluetoothAuto<B, MAX_PEERS, S>
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

fn close_member<const MAX_PEERS: usize>(
    closed: PeerClosed,
    members: &mut HashMap<BleIdentity, TokioMember>,
    manager: &mut ConnectionPolicy<MAX_PEERS, DIAL_TRACK>,
    pending: &mut Vec<PolicyAction>,
) -> bool {
    let Some(member) = members.get(&closed.identity) else {
        return false;
    };
    if !closed.matches(member.address, &member.status) {
        return false;
    }
    let PeerClosed {
        identity, address, ..
    } = closed;
    if let Some(member) = members.remove(&identity) {
        member.attached.teardown();
    }
    manager.handle(PolicyInput::Closed { identity, address }, &mut |action| {
        pending.push(action)
    });
    true
}

pub(super) const HANDSHAKE_TIMEOUT: Duration =
    Duration::from_millis(contract::HANDSHAKE_TIMEOUT_MS);

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
    closed: &mpsc::UnboundedSender<PeerClosed>,
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
            if !local
                .discovery_groups
                .contains(&DEFAULT_DISCOVERY_GROUP_HASH)
            {
                return Err(HandshakeFailure::Aborted(CloseReason::Incompatible));
            }
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
            let reaction = handshake.absorb(local, msg);
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

    #[tokio::test(start_paused = true)]
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

        tokio::time::advance(RECENT_MEMBER_GRACE - Duration::from_millis(1)).await;
        assert_eq!(status.connection(), ConnectionState::Degraded);

        tokio::time::advance(Duration::from_millis(1)).await;
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

    #[test]
    fn radio_observation_is_live_and_independent_of_preference_or_peer_count() {
        let status = BluetoothAutoStatus::new();
        status.mark_up();
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert_eq!(status.radio_state(), BluetoothRadioState::Unknown);

        let (observed, current) = watch::channel(BluetoothRadioState::PoweredOn);
        status.observe_radio_state(move || *current.borrow());
        let cloned = status.clone();
        status.disable();
        assert_eq!(cloned.radio_state(), BluetoothRadioState::PoweredOn);
        assert_eq!(cloned.connection(), ConnectionState::Disabled);

        observed.send_replace(BluetoothRadioState::PoweredOff);
        status.enable();
        assert_eq!(cloned.radio_state(), BluetoothRadioState::PoweredOff);
        assert_eq!(cloned.connection(), ConnectionState::Disconnected);
    }

    #[test]
    fn disabled_acknowledgment_cannot_settle_a_newer_enable_request() {
        let status = BluetoothAutoStatus::new();
        assert!(!status.is_disabled());
        status.disable();
        let old_request = status.disabled_request().unwrap();
        assert!(!status.is_disabled());
        status.enable();
        status.acknowledge_disabled(old_request);
        assert!(!status.is_disabled());
        status.disable();
        status.acknowledge_disabled(old_request);
        assert!(!status.is_disabled());
        status.acknowledge_disabled(status.disabled_request().unwrap());
        assert!(status.is_disabled());
        status.enable();
        assert!(!status.is_disabled());
        status.toggle_enabled();
        assert!(!status.is_disabled());
    }

    struct MockSeam {
        inbound: mpsc::UnboundedSender<std::vec::Vec<u8>>,
        sink: std::vec::Vec<u8>,
        outbound: TokioGrantConsumer,
    }

    use prns_core::interfaces::FrameSink;

    impl InterfaceSeam for MockSeam {
        fn fill_random(&mut self, bytes: &mut [u8]) {
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
            contract::copy_received_frame(&frame, out).map_err(|_| Closed)
        }
    }

    #[tokio::test]
    async fn a_columba_dialer_sends_its_identity_and_settles_without_native_metadata() {
        let local = LocalPeer {
            identity: BleIdentity::new([1; 16]),
            endpoint: linux(),
            capabilities: caps(0x0080),
            discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
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
            discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
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

    #[derive(Debug, PartialEq, Eq)]
    enum StartupCall {
        Radio(RadioMode),
        Capabilities(LinkCapabilities),
        Advertising(AdvertisingMode),
        Scanning(ScanningMode),
        NextEvent,
    }

    struct RecordingStartupBackend {
        calls: Vec<StartupCall>,
        first_event: Option<oneshot::Sender<Vec<StartupCall>>>,
        radio_off: Option<oneshot::Sender<()>>,
        finish_radio_off: Option<oneshot::Receiver<Result<(), Closed>>>,
    }

    impl BleBackend<7> for RecordingStartupBackend {
        type Error = Closed;
        type Link = LoopbackLink;

        async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), Closed> {
            self.calls.push(StartupCall::Radio(mode));
            if mode == RadioMode::Off {
                if let Some(radio_off) = self.radio_off.take() {
                    let _ = radio_off.send(());
                }
                if let Some(finish) = self.finish_radio_off.take() {
                    return finish.await.map_err(|_| Closed)?;
                }
            }
            Ok(())
        }

        async fn local_capabilities(
            &mut self,
            configured: LinkCapabilities,
        ) -> Result<LinkCapabilities, Closed> {
            self.calls.push(StartupCall::Capabilities(configured));
            Ok(configured)
        }

        async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), Closed> {
            self.calls.push(StartupCall::Advertising(mode));
            Ok(())
        }

        async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), Closed> {
            self.calls.push(StartupCall::Scanning(mode));
            Ok(())
        }

        async fn next_event(&mut self) -> BleEvent<LoopbackLink> {
            if let Some(first_event) = self.first_event.take() {
                self.calls.push(StartupCall::NextEvent);
                let _ = first_event.send(std::mem::take(&mut self.calls));
            }
            std::future::pending().await
        }

        async fn dial(
            &mut self,
            _address: BleAddress,
        ) -> prns_core::interfaces::bluetooth_auto::DialOutcome {
            prns_core::interfaces::bluetooth_auto::DialOutcome::UnknownPeer
        }
    }

    #[tokio::test]
    async fn ios_gatt_only_startup_enables_scanning_before_waiting_for_peers() {
        let (first_event_tx, first_event_rx) = oneshot::channel();
        let capabilities = LinkCapabilities {
            l2cap: None,
            link_mtu: contract::BLE_HW_MTU as u16,
        };
        let bluetooth = BluetoothAuto::<_, 7>::new(
            RecordingStartupBackend {
                calls: Vec::new(),
                first_event: Some(first_event_tx),
                radio_off: None,
                finish_radio_off: None,
            },
            BleIdentity::new([1; 16]),
            Endpoint::CoreBluetooth(AppleHost::Ios),
            capabilities,
        );
        let status = bluetooth.status();
        assert!(status.is_enabled());
        let (fleet, _detached_fleet) = Fleet::detached(status.id());
        let calls = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(1), first_event_rx) => {
                result.unwrap().unwrap()
            }
            () = bluetooth.run(fleet) => unreachable!("the BLE supervisor runs until cancelled"),
        };
        assert_eq!(
            calls,
            vec![
                StartupCall::Radio(RadioMode::On),
                StartupCall::Capabilities(capabilities),
                StartupCall::Advertising(AdvertisingMode::On),
                StartupCall::Scanning(ScanningMode::On),
                StartupCall::NextEvent,
            ]
        );
    }

    #[tokio::test]
    async fn initially_disabled_bluetooth_never_starts_scanning_before_enable() {
        let (first_event_tx, first_event_rx) = oneshot::channel();
        let (radio_off_tx, radio_off_rx) = oneshot::channel();
        let capabilities = LinkCapabilities {
            l2cap: None,
            link_mtu: contract::BLE_HW_MTU as u16,
        };
        let bluetooth = BluetoothAuto::<_, 7>::new(
            RecordingStartupBackend {
                calls: Vec::new(),
                first_event: Some(first_event_tx),
                radio_off: Some(radio_off_tx),
                finish_radio_off: None,
            },
            BleIdentity::new([1; 16]),
            Endpoint::CoreBluetooth(AppleHost::Ios),
            capabilities,
        );
        let status = bluetooth.status();
        status.disable();
        assert!(!status.is_disabled());
        let (fleet, _detached_fleet) = Fleet::detached(status.id());
        let observe = async {
            radio_off_rx.await.unwrap();
            assert_eq!(status.connection(), ConnectionState::Disabled);
            assert!(status.is_disabled());
            status.enable();
            first_event_rx.await.unwrap()
        };
        let calls = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(1), observe) => result.unwrap(),
            () = bluetooth.run(fleet) => unreachable!("the BLE supervisor runs until cancelled"),
        };
        assert_eq!(
            calls,
            vec![
                StartupCall::Advertising(AdvertisingMode::Off),
                StartupCall::Scanning(ScanningMode::Off),
                StartupCall::Radio(RadioMode::Off),
                StartupCall::Radio(RadioMode::On),
                StartupCall::Capabilities(capabilities),
                StartupCall::Advertising(AdvertisingMode::On),
                StartupCall::Scanning(ScanningMode::On),
                StartupCall::NextEvent,
            ]
        );
    }

    #[tokio::test]
    async fn zero_peer_shutdown_waits_for_backend_completion_before_acknowledging() {
        for initially_enabled in [true, false] {
            let (first_event_tx, first_event_rx) = oneshot::channel();
            let (radio_off_tx, radio_off_rx) = oneshot::channel();
            let (finish_off_tx, finish_off_rx) = oneshot::channel();
            let bluetooth = BluetoothAuto::<_, 7>::new(
                RecordingStartupBackend {
                    calls: Vec::new(),
                    first_event: Some(first_event_tx),
                    radio_off: Some(radio_off_tx),
                    finish_radio_off: Some(finish_off_rx),
                },
                BleIdentity::new([1; 16]),
                Endpoint::CoreBluetooth(AppleHost::Ios),
                caps(0x0081),
            );
            let status = bluetooth.status();
            if !initially_enabled {
                status.disable();
            }
            let (fleet, _detached_fleet) = Fleet::detached(status.id());
            let observe = async {
                if initially_enabled {
                    first_event_rx.await.unwrap();
                    status.disable();
                }
                radio_off_rx.await.unwrap();
                assert!(!status.is_disabled());
                assert!(status.shared.members.lock().unwrap().is_empty());
                finish_off_tx.send(Ok(())).unwrap();
                let mut changed = status.shared.enabled.subscribe();
                changed.wait_for(|current| current.disabled).await.unwrap();
                assert!(status.is_disabled());
            };
            tokio::select! {
                result = tokio::time::timeout(Duration::from_secs(1), observe) => result.unwrap(),
                () = bluetooth.run(fleet) => unreachable!("the BLE supervisor runs until cancelled"),
            }
        }
    }

    #[tokio::test]
    async fn two_nodes_handshake_over_loopback_and_a_frame_crosses() {
        let local_a = LocalPeer {
            identity: BleIdentity::new([1u8; 16]),
            endpoint: mac(),
            capabilities: caps(0x0081),
            discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
        };
        let local_b = LocalPeer {
            identity: BleIdentity::new([2u8; 16]),
            endpoint: linux(),
            capabilities: caps(0x0082),
            discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
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
    async fn concurrent_group_replacements_settle_in_request_order() {
        fn groups(name: &str) -> DiscoveryGroupSet {
            let id = prns_core::interfaces::DiscoveryGroupId::parse(name).expect("valid group");
            DiscoveryGroupSet::singleton(id).expect("valid set")
        }

        let status = BluetoothAutoStatus::new();
        status.mark_up();
        let initial = status.discovery_groups();
        let first = groups("alpha");
        let second = groups("bravo");
        let first_task = {
            let status = status.clone();
            tokio::spawn(async move { status.replace_discovery_groups(first).await })
        };
        tokio::task::yield_now().await;
        let second_task = {
            let status = status.clone();
            tokio::spawn(async move { status.replace_discovery_groups(second).await })
        };

        let first_requested = status.wait_for_discovery_groups_change(&initial).await;
        assert_eq!(first_requested, first);
        status.acknowledge_discovery_groups(first_requested);
        let second_requested = status
            .wait_for_discovery_groups_change(&first_requested)
            .await;
        assert_eq!(second_requested, second);
        status.acknowledge_discovery_groups(second_requested);

        assert_eq!(
            first_task.await.expect("first task"),
            DiscoveryGroupApplyOutcome::Applied
        );
        assert_eq!(
            second_task.await.expect("second task"),
            DiscoveryGroupApplyOutcome::Applied
        );
    }

    #[tokio::test]
    async fn a_gatt_only_listener_settles_the_link_on_the_floor() {
        let local_a = LocalPeer {
            identity: BleIdentity::new([1u8; 16]),
            endpoint: mac(),
            capabilities: caps(0x0081),
            discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
        };
        let local_b = LocalPeer {
            identity: BleIdentity::new([2u8; 16]),
            endpoint: linux(),
            capabilities: LinkCapabilities {
                l2cap: None,
                link_mtu: 247,
            },
            discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
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
            discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
        };
        let android_local = LocalPeer {
            identity: BleIdentity::new([2u8; 16]),
            endpoint: android(),
            capabilities: caps(0x0080),
            discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
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
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<PeerClosed>();
        let member =
            BluetoothPeer::new(identity, source, sink).report_close_to(addr, closed_tx.clone());
        let status = member.status();
        tokio::spawn(member.run(idle_seam()));

        let reported = tokio::time::timeout(Duration::from_secs(2), closed_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!((reported.identity, reported.address), (identity, addr));
        assert!(reported.matches(addr, &status));
    }

    #[test]
    fn queued_close_belongs_to_one_incarnation_even_at_the_same_address() {
        let identity = BleIdentity::new([1; 16]);
        let address = BleAddress::new([2; 6]);
        let id = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, identity.as_bytes());
        let old = TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected);
        let replacement = TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected);
        let delayed = PeerClosed {
            identity,
            address,
            status: old.clone(),
        };
        let current = PeerClosed {
            identity,
            address,
            status: replacement.clone(),
        };
        assert_eq!(
            [
                delayed.matches(address, &old),
                delayed.matches(address, &replacement),
                current.matches(address, &replacement),
                current.matches(BleAddress::new([3; 6]), &replacement),
            ],
            [true, false, true, false]
        );
    }

    #[tokio::test]
    async fn a_torn_down_member_does_not_fire_its_close_signal() {
        let addr = BleAddress::new([0x11; 6]);
        let (link_a, link_b) = link_pair(addr, BleAddress::new([0x22; 6]));
        let (source, sink) = link_a.into_data();
        let _keep_peer_alive = link_b;

        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<PeerClosed>();
        let member = BluetoothPeer::new(BleIdentity::new([1u8; 16]), source, sink)
            .report_close_to(addr, closed_tx.clone());
        let handle = tokio::spawn(member.run(idle_seam()));

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(closed_rx.try_recv().is_err());
    }
    async fn assert_delayed_close_preserves_replacement(replacement_address: BleAddress) {
        let old_address = BleAddress::new([0x11; 6]);
        let identity = BleIdentity::new([2; 16]);
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel();
        let (old_link, old_remote) = link_pair(BleAddress::new([0xAA; 6]), old_address);
        let (source, sink) = old_link.into_data();
        let old_peer = BluetoothPeer::new(identity, source, sink)
            .report_close_to(old_address, closed_tx.clone());
        let old_status = old_peer.status();
        let old_task = tokio::spawn(old_peer.run(idle_seam()));
        drop(old_remote);
        // Hold the real peer's notification until after a fresh admission, as can happen
        // when replacement or a supervisor reset wins the race with the close queue.
        let delayed_close = tokio::time::timeout(Duration::from_secs(1), closed_rx.recv())
            .await
            .unwrap()
            .unwrap();
        old_task.abort();

        let capabilities = LinkCapabilities {
            l2cap: None,
            link_mtu: 247,
        };
        let local = LocalPeer {
            identity: BleIdentity::new([1; 16]),
            endpoint: linux(),
            capabilities,
            discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
        };
        let established = EstablishedPeer {
            identity,
            transport: EstablishedTransport::Native {
                endpoint: linux(),
                capabilities,
            },
            peer_rssi: None,
        };
        let mut manager = ConnectionPolicy::<8, DIAL_TRACK>::new(local);
        manager.start(&mut |_| {});
        let mut pending = Vec::new();
        manager.handle(
            PolicyInput::Settled {
                address: replacement_address,
                origin: Origin::Dialed,
                established,
                now_ms: 0,
            },
            &mut |action| pending.push(action),
        );
        assert!(matches!(pending[0], PolicyAction::Admit { .. }));
        let (replacement_link, _replacement_remote) =
            link_pair(BleAddress::new([0xAA; 6]), replacement_address);
        let (fleet, _detached_fleet) = Fleet::detached(BluetoothAutoStatus::new().id());
        let (mut backend, _remote_backend) = LoopbackBleBackend::pair();
        let mut members = HashMap::new();
        apply_settle(
            &mut pending,
            replacement_link,
            &fleet,
            &closed_tx,
            &mut members,
            &mut backend,
            contract::defaults_for_bitrate(contract::BLE_BITRATE_GUESS_BPS)
                .configured(ConfiguredInterfacePolicy::default()),
        )
        .await;
        let replacement_status = members[&identity].status.clone();
        assert!(!replacement_status.same_instance(&old_status));

        close_member(delayed_close, &mut members, &mut manager, &mut pending);
        assert_eq!(members.len(), 1);
        assert!(members[&identity].status.same_instance(&replacement_status));
        assert_eq!(manager.settled_count(), 1);
        assert!(
            pending.is_empty(),
            "stale close must not emit backend cleanup"
        );

        // The keeper still owns the policy slot, so an ordinary duplicate loses.
        let challenger_address = BleAddress::new([0x33; 6]);
        manager.handle(
            PolicyInput::Settled {
                address: challenger_address,
                origin: Origin::Accepted,
                established,
                now_ms: 1,
            },
            &mut |action| pending.push(action),
        );
        assert_eq!(
            pending,
            vec![PolicyAction::Reject {
                address: challenger_address,
                dialed: false,
            }]
        );
        pending.clear();

        close_member(
            PeerClosed {
                identity,
                address: replacement_address,
                status: replacement_status.clone(),
            },
            &mut members,
            &mut manager,
            &mut pending,
        );
        assert!(members.is_empty());
        assert_eq!(manager.settled_count(), 0);
        assert_eq!(
            pending,
            vec![PolicyAction::NotifyClosed(replacement_address)]
        );
        pending.clear();

        // Repeated notifications after retirement cannot trigger duplicate cleanup.
        close_member(
            PeerClosed {
                identity,
                address: replacement_address,
                status: replacement_status,
            },
            &mut members,
            &mut manager,
            &mut pending,
        );
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn delayed_close_does_not_retire_same_identity_at_same_address() {
        assert_delayed_close_preserves_replacement(BleAddress::new([0x11; 6])).await;
    }

    #[tokio::test]
    async fn delayed_close_does_not_retire_same_identity_at_new_address() {
        assert_delayed_close_preserves_replacement(BleAddress::new([0x22; 6])).await;
    }
}
