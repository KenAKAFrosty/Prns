use ::core::cell::Cell;

use embassy_futures::join::join_array;
use embassy_futures::select::{select, select5, select_array, Either, Either5};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, RawMutex};
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_deadline, with_timeout, Duration, Instant};
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use prns_core::engine::FanTarget;
use prns_core::interfaces::bluetooth_auto::{
    self as contract, BleAddress, BleIdentity, Control, Endpoint, EstablishedPeer,
    EstablishedTransport, Handshake, HandshakeOutcome, L2capPlan, LinkCapabilities, LocalPeer,
    PeerProtocol,
};
use prns_core::interfaces::bluetooth_auto::{
    role_for, ConnectionPolicy, PolicyAction, PolicyInput,
};
use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, DialOutcome, Origin,
    RadioMode, ScanningMode,
};
use prns_core::interfaces::{
    BitrateBps, ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus,
};
use prns_runtime::manifold::grant::FrameTarget;
use prns_runtime::runtime::{EmbassyFleet as Fleet, OutboundFrame};

const DIAL_TRACK: usize = 6;

const ACTION_CAP: usize = 6;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_LANES: usize = 2;
const OUTBOUND_TIMEOUT: Duration = Duration::from_secs(2);

const ACTION_OVERFLOW_REASON: &str = "BLE policy action capacity exceeded";
const DIAL_INVARIANT_REASON: &str = "BLE dial admission invariant failed";
const RADIO_CONTROL_REASON: &str = "BLE radio control failed";
const INGRESS_PRESSURE_REASON: &str = "BLE receive pressure";
const SETUP_FAILURE_REASON: &str = "BLE setup failed; retrying";
const TRANSPORT_CLOSURE_REASON: &str = "BLE transport closed; retrying";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BluetoothRecoveryReason {
    IngressPressure,
    SetupFailure,
    TransportClosure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BluetoothRecoveryCounters {
    pub ingress_pressure: u32,
    pub setup_failures: u32,
    pub transport_closures: u32,
}

impl BluetoothRecoveryCounters {
    const ZERO: Self = Self {
        ingress_pressure: 0,
        setup_failures: 0,
        transport_closures: 0,
    };
}

impl BluetoothRecoveryReason {
    const fn as_u8(self) -> u8 {
        match self {
            Self::IngressPressure => 1,
            Self::SetupFailure => 2,
            Self::TransportClosure => 3,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::IngressPressure),
            2 => Some(Self::SetupFailure),
            3 => Some(Self::TransportClosure),
            _ => None,
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::IngressPressure => INGRESS_PRESSURE_REASON,
            Self::SetupFailure => SETUP_FAILURE_REASON,
            Self::TransportClosure => TRANSPORT_CLOSURE_REASON,
        }
    }
}

pub struct BluetoothMemberStatus {
    id: CriticalSectionMutex<Cell<InterfaceId>>,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    active: AtomicBool,
}

impl BluetoothMemberStatus {
    const fn new() -> Self {
        Self {
            id: CriticalSectionMutex::new(Cell::new(InterfaceId::new([0u8; 8]))),
            connection: AtomicU8::new(ConnectionState::Disconnected.as_u8()),
            rx: AtomicU64::new(0),
            tx: AtomicU64::new(0),
            active: AtomicBool::new(false),
        }
    }

    fn assign(&self, id: InterfaceId) {
        self.id.lock(|cell| cell.set(id));
        self.connection
            .store(ConnectionState::Connected.as_u8(), Ordering::Relaxed);
        self.rx.store(0, Ordering::Relaxed);
        self.tx.store(0, Ordering::Relaxed);
        self.active.store(true, Ordering::Relaxed);
    }

    fn retire(&self) {
        self.connection
            .store(ConnectionState::Disconnected.as_u8(), Ordering::Relaxed);
        self.active.store(false, Ordering::Relaxed);
    }

    fn add_rx(&self, bytes: u64) {
        self.rx.fetch_add(bytes, Ordering::Relaxed);
    }

    fn add_tx(&self, bytes: u64) {
        self.tx.fetch_add(bytes, Ordering::Relaxed);
    }
}

impl InterfaceStatus for BluetoothMemberStatus {
    fn id(&self) -> InterfaceId {
        self.id.lock(|cell| cell.get())
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
}

pub struct BluetoothAutoShared<const MEMBERS: usize> {
    id: InterfaceId,
    enabled: AtomicBool,
    enabled_changed: Signal<CriticalSectionRawMutex, bool>,
    up: AtomicBool,
    failed: AtomicBool,
    fatal_failure_reason: CriticalSectionMutex<Cell<Option<&'static str>>>,
    recovery_reason: AtomicU8,
    peers: AtomicU32,
    recovery_counters: CriticalSectionMutex<Cell<BluetoothRecoveryCounters>>,
    members: [BluetoothMemberStatus; MEMBERS],
}

impl<const MEMBERS: usize> BluetoothAutoShared<MEMBERS> {
    #[must_use]
    pub const fn new(id: InterfaceId) -> Self {
        Self {
            id,
            enabled: AtomicBool::new(true),
            enabled_changed: Signal::new(),
            up: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            fatal_failure_reason: CriticalSectionMutex::new(Cell::new(None)),
            recovery_reason: AtomicU8::new(0),
            peers: AtomicU32::new(0),
            recovery_counters: CriticalSectionMutex::new(Cell::new(
                BluetoothRecoveryCounters::ZERO,
            )),
            members: [const { BluetoothMemberStatus::new() }; MEMBERS],
        }
    }
}

#[derive(Clone, Copy)]
pub struct BluetoothAutoStatus<const MEMBERS: usize> {
    shared: &'static BluetoothAutoShared<MEMBERS>,
}

impl<const MEMBERS: usize> BluetoothAutoStatus<MEMBERS> {
    #[must_use]
    pub const fn new(shared: &'static BluetoothAutoShared<MEMBERS>) -> Self {
        Self { shared }
    }

    fn increment_recovery_counter(&self, reason: BluetoothRecoveryReason) {
        self.shared.recovery_counters.lock(|slot| {
            let mut counters = slot.get();
            match reason {
                BluetoothRecoveryReason::IngressPressure => {
                    counters.ingress_pressure = counters.ingress_pressure.saturating_add(1);
                }
                BluetoothRecoveryReason::SetupFailure => {
                    counters.setup_failures = counters.setup_failures.saturating_add(1);
                }
                BluetoothRecoveryReason::TransportClosure => {
                    counters.transport_closures = counters.transport_closures.saturating_add(1);
                }
            }
            slot.set(counters);
        });
    }

    pub fn note_ingress_pressure(&self) {
        self.increment_recovery_counter(BluetoothRecoveryReason::IngressPressure);
        self.shared.recovery_reason.store(
            BluetoothRecoveryReason::IngressPressure.as_u8(),
            Ordering::Relaxed,
        );
    }

    pub fn note_successful_admission(&self) {
        let _ = self.shared.recovery_reason.compare_exchange(
            BluetoothRecoveryReason::IngressPressure.as_u8(),
            0,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    pub fn note_setup_failure(&self) {
        self.increment_recovery_counter(BluetoothRecoveryReason::SetupFailure);
        self.shared.recovery_reason.store(
            BluetoothRecoveryReason::SetupFailure.as_u8(),
            Ordering::Relaxed,
        );
    }

    pub fn note_transport_closure(&self) {
        self.increment_recovery_counter(BluetoothRecoveryReason::TransportClosure);
        self.shared.recovery_reason.store(
            BluetoothRecoveryReason::TransportClosure.as_u8(),
            Ordering::Relaxed,
        );
    }

    fn note_settled_link(&self) {
        self.shared.recovery_reason.store(0, Ordering::Relaxed);
    }

    #[must_use]
    pub fn recovery_reason(&self) -> Option<BluetoothRecoveryReason> {
        BluetoothRecoveryReason::from_u8(self.shared.recovery_reason.load(Ordering::Relaxed))
    }

    #[must_use]
    pub fn recovery_counters(&self) -> BluetoothRecoveryCounters {
        self.shared.recovery_counters.lock(Cell::get)
    }

    #[must_use]
    pub fn ingress_pressure_events(&self) -> u32 {
        self.recovery_counters().ingress_pressure
    }

    #[must_use]
    pub fn setup_failure_events(&self) -> u32 {
        self.recovery_counters().setup_failures
    }

    #[must_use]
    pub fn transport_closure_events(&self) -> u32 {
        self.recovery_counters().transport_closures
    }

    fn mark_up(&self) {
        self.shared.up.store(true, Ordering::Relaxed);
    }

    fn mark_failed(&self, reason: &'static str) {
        self.shared
            .fatal_failure_reason
            .lock(|slot| slot.set(Some(reason)));
        self.shared.failed.store(true, Ordering::Relaxed);
    }

    fn is_failed(&self) -> bool {
        self.shared.failed.load(Ordering::Relaxed)
    }

    pub fn enable(&self) {
        self.update_enabled(true);
    }

    pub fn disable(&self) {
        self.update_enabled(false);
    }

    pub fn toggle_enabled(&self) {
        let enabled = !self.shared.enabled.fetch_xor(true, Ordering::Relaxed);
        self.shared.enabled_changed.signal(enabled);
    }

    fn update_enabled(&self, enabled: bool) {
        if self.shared.enabled.swap(enabled, Ordering::Relaxed) != enabled {
            self.shared.enabled_changed.signal(enabled);
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    async fn wait_until_enabled(&self) {
        self.wait_for_enabled_state(true).await;
    }

    async fn wait_until_disabled(&self) {
        self.wait_for_enabled_state(false).await;
    }

    async fn wait_for_enabled_state(&self, enabled: bool) {
        loop {
            if self.is_enabled() == enabled {
                return;
            }
            if self.shared.enabled_changed.wait().await == enabled {
                return;
            }
        }
    }

    fn member(&self, slot: usize) -> &'static BluetoothMemberStatus {
        &self.shared.members[slot]
    }

    fn republish_peer_count(&self) {
        let count = self
            .shared
            .members
            .iter()
            .filter(|member| member.active.load(Ordering::Relaxed))
            .count();
        self.shared.peers.store(count as u32, Ordering::Relaxed);
    }

    pub fn members(&self) -> impl Iterator<Item = &'static BluetoothMemberStatus> {
        self.shared
            .members
            .iter()
            .filter(|member| member.active.load(Ordering::Relaxed))
    }
}

impl<const MEMBERS: usize> InterfaceStatus for BluetoothAutoStatus<MEMBERS> {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            ConnectionState::Disabled
        } else if self.is_failed() {
            ConnectionState::Failed
        } else if !self.shared.up.load(Ordering::Relaxed) {
            ConnectionState::Initializing
        } else if matches!(
            self.recovery_reason(),
            Some(BluetoothRecoveryReason::IngressPressure)
        ) && self.shared.peers.load(Ordering::Relaxed) > 0
        {
            ConnectionState::Degraded
        } else if self.recovery_reason().is_some() && self.shared.peers.load(Ordering::Relaxed) == 0
        {
            ConnectionState::Reconnecting
        } else if self.shared.peers.load(Ordering::Relaxed) > 0 {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    fn rx_bytes(&self) -> u64 {
        self.shared
            .members
            .iter()
            .map(|member| member.rx.load(Ordering::Relaxed))
            .sum()
    }

    fn tx_bytes(&self) -> u64 {
        self.shared
            .members
            .iter()
            .map(|member| member.tx.load(Ordering::Relaxed))
            .sum()
    }

    fn failure_reason(&self) -> Option<&'static str> {
        self.shared
            .fatal_failure_reason
            .lock(Cell::get)
            .or_else(|| {
                self.recovery_reason()
                    .map(BluetoothRecoveryReason::description)
            })
    }
}

struct Active<L: BleLink> {
    identity: BleIdentity,
    id: InterfaceId,
    slot: usize,
    address: BleAddress,
    source: L::Source,
    sink: L::Sink,
}

struct PendingActions<const CAP: usize> {
    actions: heapless::Vec<PolicyAction, CAP>,
    overflowed: bool,
}

impl<const CAP: usize> PendingActions<CAP> {
    const fn new() -> Self {
        Self {
            actions: heapless::Vec::new(),
            overflowed: false,
        }
    }

    fn push(&mut self, action: PolicyAction) {
        if self.actions.push(action).is_err() {
            self.overflowed = true;
        }
    }

    fn take(&mut self) -> heapless::Vec<PolicyAction, CAP> {
        ::core::mem::take(&mut self.actions)
    }

    fn clear(&mut self) {
        self.actions.clear();
        self.overflowed = false;
    }
}

enum HandshakeStage {
    ColumbaReceive,
    ColumbaSend {
        identity: BleIdentity,
    },
    NativeSend {
        handshake: Option<Handshake>,
        control: Control,
    },
    NativeReceive {
        handshake: Option<Handshake>,
    },
    NativeReply {
        handshake: Option<Handshake>,
        control: Control,
        outcome: HandshakeOutcome,
    },
}

struct PendingHandshake<L: BleLink> {
    link: L,
    address: BleAddress,
    origin: Origin,
    deadline: Instant,
    stage: HandshakeStage,
}

impl<L: BleLink> PendingHandshake<L> {
    fn new(link: L, origin: Origin, local: LocalPeer) -> Self {
        let stage = if link.peer_protocol() == PeerProtocol::Columba {
            HandshakeStage::ColumbaReceive
        } else {
            let (handshake, opening) = Handshake::begin(role_for(origin), local, None);
            match opening {
                Some(control) => HandshakeStage::NativeSend {
                    handshake: Some(handshake),
                    control,
                },
                None => HandshakeStage::NativeReceive {
                    handshake: Some(handshake),
                },
            }
        };
        Self {
            address: link.address(),
            link,
            origin,
            deadline: Instant::now() + HANDSHAKE_TIMEOUT,
            stage,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandshakeFailure {
    Timeout,
    Link,
    Aborted,
    InvariantViolation,
}

struct HandshakeDone<L: BleLink> {
    address: BleAddress,
    origin: Origin,
    outcome: Result<(EstablishedPeer, L), HandshakeFailure>,
}

enum HandshakeStep<L: BleLink> {
    Advanced,
    Done(HandshakeDone<L>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SendState {
    NotSelected,
    Pending,
    Sent,
    Failed,
}

enum SupervisorStep<L: BleLink> {
    Disabled,
    Handshake(HandshakeStep<L>),
    Backend(BleEvent<L>),
    Inbound(usize, Result<usize, <L::Source as BleSource>::Error>),
    Outbound,
}

pub struct BluetoothAuto<B, const MEMBERS: usize> {
    backend: B,
    local: LocalPeer,
    status: BluetoothAutoStatus<MEMBERS>,
    bitrate: BitrateBps,
}

impl<B, const MEMBERS: usize> BluetoothAuto<B, MEMBERS>
where
    B: BleBackend<MEMBERS>,
{
    #[must_use]
    pub fn new(
        backend: B,
        identity: BleIdentity,
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
        group_tag: [u8; 4],
        shared: &'static BluetoothAutoShared<MEMBERS>,
    ) -> Self {
        Self {
            backend,
            local: LocalPeer {
                identity,
                endpoint,
                capabilities,
                group_tag,
            },
            status: BluetoothAutoStatus::new(shared),
            bitrate: contract::BLE_BITRATE_GUESS_BPS,
        }
    }

    #[must_use]
    pub fn status(&self) -> BluetoothAutoStatus<MEMBERS> {
        self.status
    }

    pub async fn run<M, const FRAME: usize, const NOTIFY: usize, const LIFECYCLE: usize>(
        self,
        mut fleet: Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    ) where
        M: RawMutex + 'static,
    {
        let Self {
            mut backend,
            local: configured_local,
            status,
            bitrate,
        } = self;
        if let Some(reason) = backend.blocked() {
            status.mark_failed(reason);
            ::core::future::pending::<()>().await;
            return;
        }
        let configured_capabilities = configured_local.capabilities;
        let mut local = configured_local;
        prepare_radio(&mut backend, &mut local, configured_capabilities, &status).await;
        if status.is_failed() {
            let _ = backend.set_radio_mode(RadioMode::Off).await;
            ::core::future::pending::<()>().await;
            return;
        }
        let mut manager = ConnectionPolicy::<MEMBERS, DIAL_TRACK>::new(local);
        let mut members: [Option<Active<B::Link>>; MEMBERS] = [const { None }; MEMBERS];
        let mut inbufs: [[u8; contract::BLE_HW_MTU]; MEMBERS] =
            [[0u8; contract::BLE_HW_MTU]; MEMBERS];
        let mut handshakes: [Option<PendingHandshake<B::Link>>; HANDSHAKE_LANES] =
            [const { None }; HANDSHAKE_LANES];
        let mut pending = PendingActions::<ACTION_CAP>::new();
        let mut outbound_first = false;
        status.mark_up();
        manager.start(&mut |action| pending.push(action));
        apply_radio(
            &mut pending,
            &mut manager,
            &status,
            &mut fleet,
            &mut backend,
            &mut members,
        )
        .await;

        loop {
            if status.is_failed() {
                handshakes.fill_with(|| None);
                pending.clear();
                disable_members(&status, &mut fleet, &mut backend, &mut members).await;
                ::core::future::pending::<()>().await;
                return;
            }
            if !status.is_enabled() {
                handshakes.fill_with(|| None);
                disable_members(&status, &mut fleet, &mut backend, &mut members).await;
                pending.clear();
                status.wait_until_enabled().await;
                local = configured_local;
                prepare_radio(&mut backend, &mut local, configured_capabilities, &status).await;
                if status.is_failed() {
                    continue;
                }
                manager = ConnectionPolicy::<MEMBERS, DIAL_TRACK>::new(local);
                manager.start(&mut |action| pending.push(action));
                apply_radio(
                    &mut pending,
                    &mut manager,
                    &status,
                    &mut fleet,
                    &mut backend,
                    &mut members,
                )
                .await;
                continue;
            }
            let step = next_step(
                &status,
                &mut backend,
                &mut handshakes,
                local,
                &mut members,
                &mut inbufs,
                &fleet,
                outbound_first,
            )
            .await;
            outbound_first = !matches!(&step, SupervisorStep::Outbound);
            let now_ms = Instant::now().as_millis();
            match step {
                SupervisorStep::Disabled => {}
                SupervisorStep::Handshake(HandshakeStep::Advanced) => {}
                SupervisorStep::Handshake(HandshakeStep::Done(HandshakeDone {
                    address,
                    origin,
                    outcome,
                })) => match outcome {
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
                        apply_settled(
                            link,
                            bitrate,
                            &mut manager,
                            &mut pending,
                            &status,
                            &mut fleet,
                            &mut backend,
                            &mut members,
                        )
                        .await;
                    }
                    Err(_) => {
                        status.note_setup_failure();
                        manager.handle(
                            PolicyInput::HandshakeFailed { address, origin },
                            &mut |action| pending.push(action),
                        );
                        apply_radio(
                            &mut pending,
                            &mut manager,
                            &status,
                            &mut fleet,
                            &mut backend,
                            &mut members,
                        )
                        .await;
                    }
                },
                SupervisorStep::Backend(BleEvent::Sighting { address, .. }) => {
                    manager.handle(PolicyInput::Sighting { address, now_ms }, &mut |action| {
                        pending.push(action)
                    });
                    apply_radio(
                        &mut pending,
                        &mut manager,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                    )
                    .await;
                }
                SupervisorStep::Backend(BleEvent::Inbound(link)) => {
                    queue_handshake(
                        link,
                        Origin::Accepted,
                        local,
                        &mut manager,
                        &mut handshakes,
                        &mut backend,
                    )
                    .await;
                }
                SupervisorStep::Backend(BleEvent::LinkReady { link, origin, .. }) => {
                    queue_handshake(
                        link,
                        origin,
                        local,
                        &mut manager,
                        &mut handshakes,
                        &mut backend,
                    )
                    .await;
                }
                SupervisorStep::Backend(BleEvent::DialFailed { address }) => {
                    status.note_setup_failure();
                    manager.handle(PolicyInput::DialFailed { address, now_ms }, &mut |action| {
                        pending.push(action)
                    });
                    apply_radio(
                        &mut pending,
                        &mut manager,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                    )
                    .await;
                }
                SupervisorStep::Inbound(index, received) => {
                    deliver_inbound(
                        index,
                        received.map_err(|_| ()),
                        &mut manager,
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                        &mut inbufs,
                    )
                    .await;
                }
                SupervisorStep::Outbound => {
                    // `outbound_ready` is a coalescing signal, while the lane is a queue. One
                    // wake therefore means "one or more frames", not "exactly one frame". Drain
                    // every committed frame before waiting again or a burst's tail can sleep
                    // indefinitely until unrelated traffic happens to signal the lane.
                    while let Some(frame) = fleet.try_next_outbound() {
                        send_outbound(
                            &frame,
                            &mut manager,
                            &mut pending,
                            &status,
                            &mut fleet,
                            &mut backend,
                            &mut members,
                        )
                        .await;
                    }
                }
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the scheduler owns one borrow per independently wakeable supervisor branch"
)]
async fn next_step<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    status: &BluetoothAutoStatus<MEMBERS>,
    backend: &mut B,
    handshakes: &mut [Option<PendingHandshake<B::Link>>; HANDSHAKE_LANES],
    local: LocalPeer,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
    inbufs: &mut [[u8; contract::BLE_HW_MTU]; MEMBERS],
    fleet: &Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    outbound_first: bool,
) -> SupervisorStep<B::Link>
where
    B: BleBackend<MEMBERS>,
{
    if outbound_first {
        return match select5(
            status.wait_until_disabled(),
            fleet.outbound_ready(),
            advance_handshakes(handshakes, local),
            backend.next_event(),
            recv_any(members, inbufs),
        )
        .await
        {
            Either5::First(()) => SupervisorStep::Disabled,
            Either5::Second(()) => SupervisorStep::Outbound,
            Either5::Third(step) => SupervisorStep::Handshake(step),
            Either5::Fourth(event) => SupervisorStep::Backend(event),
            Either5::Fifth((index, received)) => SupervisorStep::Inbound(index, received),
        };
    }
    match select5(
        status.wait_until_disabled(),
        advance_handshakes(handshakes, local),
        backend.next_event(),
        recv_any(members, inbufs),
        fleet.outbound_ready(),
    )
    .await
    {
        Either5::First(()) => SupervisorStep::Disabled,
        Either5::Second(step) => SupervisorStep::Handshake(step),
        Either5::Third(event) => SupervisorStep::Backend(event),
        Either5::Fourth((index, received)) => SupervisorStep::Inbound(index, received),
        Either5::Fifth(()) => SupervisorStep::Outbound,
    }
}

async fn prepare_radio<B, const MEMBERS: usize>(
    backend: &mut B,
    local: &mut LocalPeer,
    configured_capabilities: LinkCapabilities,
    status: &BluetoothAutoStatus<MEMBERS>,
) where
    B: BleBackend<MEMBERS>,
{
    if backend.set_radio_mode(RadioMode::On).await.is_err() {
        status.mark_failed(RADIO_CONTROL_REASON);
        return;
    }
    match backend.local_capabilities(configured_capabilities).await {
        Ok(capabilities) => local.capabilities = capabilities,
        Err(_) => status.mark_failed(RADIO_CONTROL_REASON),
    }
}

async fn queue_handshake<B, const MEMBERS: usize>(
    link: B::Link,
    origin: Origin,
    local: LocalPeer,
    manager: &mut ConnectionPolicy<MEMBERS, DIAL_TRACK>,
    handshakes: &mut [Option<PendingHandshake<B::Link>>; HANDSHAKE_LANES],
    backend: &mut B,
) where
    B: BleBackend<MEMBERS>,
{
    let address = link.address();
    match handshakes.iter_mut().find(|entry| entry.is_none()) {
        Some(entry) if manager.begin_handshake(origin) => {
            *entry = Some(PendingHandshake::new(link, origin, local));
        }
        _ => {
            drop(link);
            backend.on_link_closed(address).await;
        }
    }
}

async fn advance_handshakes<L: BleLink>(
    handshakes: &mut [Option<PendingHandshake<L>>; HANDSHAKE_LANES],
    local: LocalPeer,
) -> HandshakeStep<L> {
    let [first, second] = handshakes;
    match select(
        advance_handshake(first, local),
        advance_handshake(second, local),
    )
    .await
    {
        Either::First(step) | Either::Second(step) => step,
    }
}

async fn advance_handshake<L: BleLink>(
    pending: &mut Option<PendingHandshake<L>>,
    local: LocalPeer,
) -> HandshakeStep<L> {
    let completion = match pending.as_mut() {
        Some(pending) => {
            let deadline = pending.deadline;
            match with_deadline(deadline, async {
                let mut next_stage = None;
                let completion = match &mut pending.stage {
                    HandshakeStage::ColumbaReceive => {
                        match pending.link.receive_columba_peer_identity().await {
                            Ok(identity) if pending.origin == Origin::Dialed => {
                                next_stage = Some(HandshakeStage::ColumbaSend { identity });
                                None
                            }
                            Ok(identity) => Some(Ok(EstablishedPeer {
                                identity,
                                transport: EstablishedTransport::ColumbaGatt,
                                peer_rssi: None,
                            })),
                            Err(_) => Some(Err(HandshakeFailure::Link)),
                        }
                    }
                    HandshakeStage::ColumbaSend { identity } => {
                        let identity = *identity;
                        match pending.link.send_columba_identity(local.identity).await {
                            Ok(()) => Some(Ok(EstablishedPeer {
                                identity,
                                transport: EstablishedTransport::ColumbaGatt,
                                peer_rssi: None,
                            })),
                            Err(_) => Some(Err(HandshakeFailure::Link)),
                        }
                    }
                    HandshakeStage::NativeSend { handshake, control } => {
                        let control = *control;
                        match pending.link.control_send(&control).await {
                            Ok(()) => match handshake.take() {
                                Some(handshake) => {
                                    next_stage = Some(HandshakeStage::NativeReceive {
                                        handshake: Some(handshake),
                                    });
                                    None
                                }
                                None => Some(Err(HandshakeFailure::InvariantViolation)),
                            },
                            Err(_) => Some(Err(HandshakeFailure::Link)),
                        }
                    }
                    HandshakeStage::NativeReceive { handshake } => {
                        match pending.link.control_recv().await {
                            Ok(control) => match handshake.as_mut() {
                                Some(active) => {
                                    let reaction = active.absorb(control);
                                    match reaction.reply {
                                        Some(control) => match handshake.take() {
                                            Some(handshake) => {
                                                next_stage = Some(HandshakeStage::NativeReply {
                                                    handshake: Some(handshake),
                                                    control,
                                                    outcome: reaction.outcome,
                                                });
                                                None
                                            }
                                            None => Some(Err(HandshakeFailure::InvariantViolation)),
                                        },
                                        None => match reaction.outcome {
                                            HandshakeOutcome::Pending => None,
                                            HandshakeOutcome::Settled(established) => {
                                                Some(Ok(established))
                                            }
                                            HandshakeOutcome::Aborted(_) => {
                                                Some(Err(HandshakeFailure::Aborted))
                                            }
                                        },
                                    }
                                }
                                None => Some(Err(HandshakeFailure::InvariantViolation)),
                            },
                            Err(_) => Some(Err(HandshakeFailure::Link)),
                        }
                    }
                    HandshakeStage::NativeReply {
                        handshake,
                        control,
                        outcome,
                    } => {
                        let control = *control;
                        let outcome = *outcome;
                        match pending.link.control_send(&control).await {
                            Ok(()) => match outcome {
                                HandshakeOutcome::Pending => match handshake.take() {
                                    Some(handshake) => {
                                        next_stage = Some(HandshakeStage::NativeReceive {
                                            handshake: Some(handshake),
                                        });
                                        None
                                    }
                                    None => Some(Err(HandshakeFailure::InvariantViolation)),
                                },
                                HandshakeOutcome::Settled(established) => Some(Ok(established)),
                                HandshakeOutcome::Aborted(_) => {
                                    Some(Err(HandshakeFailure::Aborted))
                                }
                            },
                            Err(_) => Some(Err(HandshakeFailure::Link)),
                        }
                    }
                };
                if let Some(stage) = next_stage {
                    pending.stage = stage;
                }
                completion
            })
            .await
            {
                Ok(completion) => completion,
                Err(_) => Some(Err(HandshakeFailure::Timeout)),
            }
        }
        None => ::core::future::pending().await,
    };
    let Some(outcome) = completion else {
        return HandshakeStep::Advanced;
    };
    let Some(pending) = pending.take() else {
        return HandshakeStep::Advanced;
    };
    HandshakeStep::Done(HandshakeDone {
        address: pending.address,
        origin: pending.origin,
        outcome: outcome.map(|established| (established, pending.link)),
    })
}

async fn recv_or_pending<L: BleLink>(
    member: &mut Option<Active<L>>,
    buf: &mut [u8; contract::BLE_HW_MTU],
) -> Result<usize, <L::Source as BleSource>::Error> {
    match member {
        Some(active) => active.source.recv_frame(buf).await,
        None => ::core::future::pending().await,
    }
}

#[expect(
    clippy::expect_used,
    reason = "from_fn runs exactly MEMBERS times over a zip of two [_; MEMBERS] arrays, so the iterator cannot run dry; the disjoint-borrow trick has no panic-free spelling without unsafe"
)]
async fn recv_any<L: BleLink, const MEMBERS: usize>(
    members: &mut [Option<Active<L>>; MEMBERS],
    bufs: &mut [[u8; contract::BLE_HW_MTU]; MEMBERS],
) -> (usize, Result<usize, <L::Source as BleSource>::Error>) {
    let mut pairs = members.iter_mut().zip(bufs.iter_mut());
    let futures: [_; MEMBERS] = ::core::array::from_fn(|_| {
        let (member, buf) = pairs.next().expect("one pair per member slot");
        recv_or_pending(member, buf)
    });
    let (result, index) = select_array(futures).await;
    (index, result)
}

async fn apply_one<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    action: PolicyAction,
    pending: &mut PendingActions<ACTION_CAP>,
    manager: &mut ConnectionPolicy<MEMBERS, DIAL_TRACK>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    match action {
        PolicyAction::Dial(address) => match backend.dial(address).await {
            DialOutcome::Started => {}
            DialOutcome::Busy | DialOutcome::UnknownPeer | DialOutcome::RadioOff => {
                manager.handle(
                    PolicyInput::DialFailed {
                        address,
                        now_ms: Instant::now().as_millis(),
                    },
                    &mut |action| pending.push(action),
                );
            }
            DialOutcome::InvariantViolation => status.mark_failed(DIAL_INVARIANT_REASON),
        },
        PolicyAction::Evict { slot, .. } => {
            if let Some(member) = members[slot].take() {
                fleet.deregister_member(member.id).await;
                status.member(slot).retire();
                status.republish_peer_count();
                backend.on_link_closed(member.address).await;
            }
        }
        PolicyAction::NotifyClosed(address) => backend.on_link_closed(address).await,
        PolicyAction::SetAdvertising(mode) => {
            if backend.set_advertising(mode).await.is_err() {
                status.mark_failed(RADIO_CONTROL_REASON);
            }
        }
        PolicyAction::SetScanning(mode) => {
            if backend.set_scanning(mode).await.is_err() {
                status.mark_failed(RADIO_CONTROL_REASON);
            }
        }
        PolicyAction::Admit { .. } | PolicyAction::Reject { .. } => {}
    }
}

async fn apply_radio<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    pending: &mut PendingActions<ACTION_CAP>,
    manager: &mut ConnectionPolicy<MEMBERS, DIAL_TRACK>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    loop {
        if pending.overflowed {
            status.mark_failed(ACTION_OVERFLOW_REASON);
            pending.actions.clear();
            return;
        }
        let actions = pending.take();
        if actions.is_empty() {
            return;
        }
        for action in actions {
            apply_one(action, pending, manager, status, fleet, backend, members).await;
            if status.is_failed() {
                return;
            }
        }
    }
}

async fn disable_members<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    let advertising = backend.set_advertising(AdvertisingMode::Off).await;
    let scanning = backend.set_scanning(ScanningMode::Off).await;
    let radio = backend.set_radio_mode(RadioMode::Off).await;
    if advertising.is_err() || scanning.is_err() || radio.is_err() {
        status.mark_failed(RADIO_CONTROL_REASON);
    }
    let mut changed = false;
    for (slot, entry) in members.iter_mut().enumerate() {
        if let Some(id) = entry.as_ref().map(|member| member.id) {
            fleet.deregister_member(id).await;
            let Some(member) = entry.take() else {
                continue;
            };
            status.member(slot).retire();
            backend.on_link_closed(member.address).await;
            changed = true;
        }
    }
    if changed {
        status.republish_peer_count();
    }
}

async fn close_member<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    slot: usize,
    manager: &mut ConnectionPolicy<MEMBERS, DIAL_TRACK>,
    pending: &mut PendingActions<ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    let Some(id) = members[slot].as_ref().map(|member| member.id) else {
        return;
    };
    fleet.deregister_member(id).await;
    let Some(member) = members[slot].take() else {
        return;
    };
    status.member(slot).retire();
    status.republish_peer_count();
    manager.handle(
        PolicyInput::Closed {
            identity: member.identity,
            address: member.address,
        },
        &mut |action| pending.push(action),
    );
    apply_radio(pending, manager, status, fleet, backend, members).await;
}

fn selected<L: BleLink>(member: &Active<L>, target: FrameTarget) -> bool {
    match target {
        FrameTarget::Direct(id) => member.id == id,
        FrameTarget::Fan(FanTarget::Only(id)) => member.id == id,
        FrameTarget::Fan(FanTarget::All) => true,
        FrameTarget::Fan(FanTarget::AllExcept(id)) => member.id != id,
    }
}

async fn send_member<L: BleLink>(
    member: &mut Option<Active<L>>,
    state: &mut SendState,
    frame: &[u8],
) {
    if *state != SendState::Pending {
        return;
    }
    let Some(member) = member.as_mut() else {
        *state = SendState::Failed;
        return;
    };
    *state = if member.sink.send_frame(frame).await.is_ok() {
        SendState::Sent
    } else {
        SendState::Failed
    };
}

#[expect(
    clippy::expect_used,
    reason = "from_fn runs exactly MEMBERS times over a zip of two [_; MEMBERS] arrays, so the iterator cannot run dry; the disjoint-borrow trick has no panic-free spelling without unsafe"
)]
async fn send_members<L: BleLink, const MEMBERS: usize>(
    members: &mut [Option<Active<L>>; MEMBERS],
    states: &mut [SendState; MEMBERS],
    frame: &[u8],
) {
    let mut pairs = members.iter_mut().zip(states.iter_mut());
    let futures: [_; MEMBERS] = ::core::array::from_fn(|_| {
        let (member, state) = pairs.next().expect("one pair per member slot");
        send_member(member, state, frame)
    });
    join_array(futures).await;
}

async fn send_outbound<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    frame: &OutboundFrame<FRAME>,
    manager: &mut ConnectionPolicy<MEMBERS, DIAL_TRACK>,
    pending: &mut PendingActions<ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    if frame.is_empty() {
        return;
    }
    let mut states = ::core::array::from_fn(|slot| match members[slot].as_ref() {
        Some(member) if selected(member, frame.target()) => SendState::Pending,
        _ => SendState::NotSelected,
    });
    let sends = send_members(members, &mut states, frame.bytes());
    match select(
        status.wait_until_disabled(),
        with_timeout(OUTBOUND_TIMEOUT, sends),
    )
    .await
    {
        Either::First(()) => return,
        Either::Second(_) => {}
    }
    for (slot, state) in states.into_iter().enumerate() {
        match state {
            SendState::NotSelected => {}
            SendState::Sent => status.member(slot).add_tx(frame.len() as u64),
            SendState::Pending | SendState::Failed => {
                status.note_transport_closure();
                close_member(slot, manager, pending, status, fleet, backend, members).await;
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "embedded serve-loop internals pass the loop's split-borrowed locals; bundling awaits an on-hardware validation pass"
)]
async fn deliver_inbound<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    index: usize,
    received: Result<usize, ()>,
    manager: &mut ConnectionPolicy<MEMBERS, DIAL_TRACK>,
    pending: &mut PendingActions<ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
    inbufs: &mut [[u8; contract::BLE_HW_MTU]; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    match received {
        Ok(0) => {}
        Ok(len) => {
            if let Some(member) = members[index].as_ref() {
                if fleet
                    .deliver_inbound(member.id, &inbufs[index][..len])
                    .await
                    .is_ok()
                {
                    status.member(member.slot).add_rx(len as u64);
                }
            }
        }
        Err(()) => {
            status.note_transport_closure();
            close_member(index, manager, pending, status, fleet, backend, members).await;
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "embedded serve-loop internals pass the loop's split-borrowed locals; bundling awaits an on-hardware validation pass"
)]
async fn apply_settled<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    link: B::Link,
    bitrate: BitrateBps,
    manager: &mut ConnectionPolicy<MEMBERS, DIAL_TRACK>,
    pending: &mut PendingActions<ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    let address = link.address();
    if pending.overflowed {
        status.mark_failed(ACTION_OVERFLOW_REASON);
        drop(link);
        backend.on_link_closed(address).await;
        return;
    }
    let actions = pending.take();
    let mut held = Some(link);
    for action in actions {
        match action {
            PolicyAction::Admit {
                identity,
                slot,
                address,
                lane,
                ..
            } => {
                if let Some(mut link) = held.take() {
                    if !matches!(lane, L2capPlan::None) {
                        let _ = link.upgrade(&lane).await;
                    }
                    let (source, sink) = link.into_data();
                    let id = InterfaceId::from_channel_tag(
                        InterfaceKind::BluetoothPeer,
                        identity.as_bytes(),
                    );
                    fleet
                        .register_member(contract::descriptor(id, bitrate))
                        .await;
                    status.member(slot).assign(id);
                    status.republish_peer_count();
                    status.note_settled_link();
                    members[slot] = Some(Active {
                        identity,
                        id,
                        slot,
                        address,
                        source,
                        sink,
                    });
                }
            }
            PolicyAction::Reject { address, .. } => {
                held = None;
                backend.on_link_closed(address).await;
            }
            other => {
                apply_one(other, pending, manager, status, fleet, backend, members).await;
            }
        }
    }
    if held.is_some() {
        drop(held.take());
        backend.on_link_closed(address).await;
    }
    apply_radio(pending, manager, status, fleet, backend, members).await;
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use embassy_futures::block_on;
    use embassy_futures::select::{select, Either};
    use prns_core::interfaces::bluetooth_auto::{Endpoint, Nrf52Host};

    use super::*;

    const CAPS: LinkCapabilities = LinkCapabilities {
        l2cap: None,
        link_mtu: contract::BLE_HW_MTU as u16,
    };

    #[derive(Debug)]
    struct MockError;

    #[derive(Clone, Copy)]
    enum MockSinkMode {
        Ready,
        Blocked,
    }

    struct MockSource;

    struct MockSink {
        mode: MockSinkMode,
    }

    struct MockLink {
        address: BleAddress,
        protocol: PeerProtocol,
        incoming: Option<Control>,
        identity: BleIdentity,
        track_drop: bool,
    }

    static DROPS: AtomicUsize = AtomicUsize::new(0);

    impl Drop for MockLink {
        fn drop(&mut self) {
            if self.track_drop {
                DROPS.fetch_add(1, AtomicOrdering::Relaxed);
            }
        }
    }

    impl BleLink for MockLink {
        type Error = MockError;
        type Source = MockSource;
        type Sink = MockSink;

        fn peer_protocol(&self) -> PeerProtocol {
            self.protocol
        }

        fn address(&self) -> BleAddress {
            self.address
        }

        async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, MockError> {
            Ok(self.identity)
        }

        async fn send_columba_identity(&mut self, _identity: BleIdentity) -> Result<(), MockError> {
            Ok(())
        }

        async fn control_send(&mut self, _msg: &Control) -> Result<(), MockError> {
            Ok(())
        }

        async fn control_recv(&mut self) -> Result<Control, MockError> {
            match self.incoming.take() {
                Some(control) => Ok(control),
                None => ::core::future::pending().await,
            }
        }

        async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), MockError> {
            Ok(())
        }

        fn into_data(self) -> (MockSource, MockSink) {
            (
                MockSource,
                MockSink {
                    mode: MockSinkMode::Ready,
                },
            )
        }
    }

    impl BleSource for MockSource {
        type Error = MockError;

        async fn recv_frame(&mut self, _out: &mut [u8]) -> Result<usize, MockError> {
            ::core::future::pending().await
        }
    }

    impl BleSink for MockSink {
        type Error = MockError;

        async fn send_frame(&mut self, _frame: &[u8]) -> Result<(), MockError> {
            match self.mode {
                MockSinkMode::Ready => Ok(()),
                MockSinkMode::Blocked => ::core::future::pending().await,
            }
        }
    }

    fn local(identity: u8) -> LocalPeer {
        LocalPeer {
            identity: BleIdentity::new([identity; 16]),
            endpoint: Endpoint::Nrf52(Nrf52Host::Nrf52),
            capabilities: CAPS,
            group_tag: contract::default_group_tag(),
        }
    }

    fn link(address: u8, incoming: Option<Control>, track_drop: bool) -> MockLink {
        MockLink {
            address: BleAddress::new([address; 6]),
            protocol: PeerProtocol::Native,
            incoming,
            identity: BleIdentity::new([address; 16]),
            track_drop,
        }
    }

    fn active(id: u8, mode: MockSinkMode) -> Active<MockLink> {
        Active {
            identity: BleIdentity::new([id; 16]),
            id: InterfaceId::new([id; 8]),
            slot: usize::from(id),
            address: BleAddress::new([id; 6]),
            source: MockSource,
            sink: MockSink { mode },
        }
    }

    #[test]
    fn simultaneous_handshakes_let_the_ready_peer_settle() {
        let local = local(1);
        let hello = Control::Hello {
            identity: BleIdentity::new([3; 16]),
            endpoint: Endpoint::Nrf52(Nrf52Host::Nrf52),
            capabilities: CAPS,
            peer_rssi: None,
            group_tag: Some(contract::default_group_tag()),
        };
        let mut handshakes = [
            Some(PendingHandshake::new(
                link(2, None, false),
                Origin::Dialed,
                local,
            )),
            Some(PendingHandshake::new(
                link(3, Some(hello), false),
                Origin::Accepted,
                local,
            )),
        ];

        block_on(async {
            assert!(matches!(
                advance_handshakes(&mut handshakes, local).await,
                HandshakeStep::Advanced
            ));
            assert!(matches!(
                advance_handshakes(&mut handshakes, local).await,
                HandshakeStep::Advanced
            ));
            let step = advance_handshakes(&mut handshakes, local).await;
            assert!(matches!(&step, HandshakeStep::Done(_)));
            if let HandshakeStep::Done(done) = step {
                assert_eq!(done.address, BleAddress::new([3; 6]));
                assert_eq!(done.origin, Origin::Accepted);
                assert!(done.outcome.is_ok());
            }
        });
        assert!(handshakes[0].is_some());
        assert!(handshakes[1].is_none());
    }

    #[test]
    fn cancelling_a_handshake_drops_its_link() {
        DROPS.store(0, AtomicOrdering::Relaxed);
        let mut handshakes = [
            Some(PendingHandshake::new(
                link(2, None, true),
                Origin::Dialed,
                local(1),
            )),
            None,
        ];

        handshakes[0] = None;

        assert_eq!(DROPS.load(AtomicOrdering::Relaxed), 1);
    }

    #[test]
    fn blocked_peer_does_not_hide_completed_fanout() {
        let mut members = [
            Some(active(0, MockSinkMode::Ready)),
            Some(active(1, MockSinkMode::Blocked)),
        ];
        let mut states = [SendState::Pending, SendState::Pending];

        block_on(async {
            assert!(matches!(
                select(send_members(&mut members, &mut states, b"frame"), async {}).await,
                Either::Second(())
            ));
        });

        assert!(matches!(states, [SendState::Sent, SendState::Pending]));
    }

    #[test]
    fn policy_action_overflow_is_explicit() {
        let mut pending = PendingActions::<1>::new();
        pending.push(PolicyAction::SetAdvertising(AdvertisingMode::On));
        pending.push(PolicyAction::SetScanning(ScanningMode::On));

        assert!(pending.overflowed);
        assert_eq!(pending.actions.len(), 1);
    }

    #[test]
    fn failed_status_survives_disable_and_reenable() {
        static SHARED: BluetoothAutoShared<1> = BluetoothAutoShared::new(InterfaceId::new([9; 8]));
        let status = BluetoothAutoStatus::new(&SHARED);
        status.mark_up();
        status.mark_failed(ACTION_OVERFLOW_REASON);

        assert_eq!(status.connection(), ConnectionState::Failed);
        assert_eq!(status.failure_reason(), Some(ACTION_OVERFLOW_REASON));
        status.disable();
        assert_eq!(status.connection(), ConnectionState::Disabled);
        status.enable();
        assert_eq!(status.connection(), ConnectionState::Failed);
    }

    fn recovery_view<const MEMBERS: usize>(
        status: &BluetoothAutoStatus<MEMBERS>,
    ) -> (
        ConnectionState,
        Option<BluetoothRecoveryReason>,
        BluetoothRecoveryCounters,
        Option<&'static str>,
    ) {
        (
            status.connection(),
            status.recovery_reason(),
            status.recovery_counters(),
            status.failure_reason(),
        )
    }

    #[test]
    fn recovery_status_distinguishes_pressure_setup_and_closure() {
        static SHARED: BluetoothAutoShared<2> = BluetoothAutoShared::new(InterfaceId::new([10; 8]));
        let status = BluetoothAutoStatus::new(&SHARED);

        assert_eq!(
            recovery_view(&status),
            (
                ConnectionState::Initializing,
                None,
                BluetoothRecoveryCounters::ZERO,
                None,
            )
        );
        status.mark_up();
        assert_eq!(
            recovery_view(&status),
            (
                ConnectionState::Disconnected,
                None,
                BluetoothRecoveryCounters::ZERO,
                None,
            )
        );

        status.note_setup_failure();
        assert_eq!(
            recovery_view(&status),
            (
                ConnectionState::Reconnecting,
                Some(BluetoothRecoveryReason::SetupFailure),
                BluetoothRecoveryCounters {
                    ingress_pressure: 0,
                    setup_failures: 1,
                    transport_closures: 0,
                },
                Some(SETUP_FAILURE_REASON),
            )
        );

        status.member(0).assign(InterfaceId::new([11; 8]));
        status.republish_peer_count();
        assert_eq!(status.connection(), ConnectionState::Connected);
        assert_eq!(status.failure_reason(), Some(SETUP_FAILURE_REASON));
        status.note_settled_link();
        assert_eq!(status.connection(), ConnectionState::Connected);

        status.note_ingress_pressure();
        assert_eq!(
            recovery_view(&status),
            (
                ConnectionState::Degraded,
                Some(BluetoothRecoveryReason::IngressPressure),
                BluetoothRecoveryCounters {
                    ingress_pressure: 1,
                    setup_failures: 1,
                    transport_closures: 0,
                },
                Some(INGRESS_PRESSURE_REASON),
            )
        );
        status.note_successful_admission();
        assert_eq!(status.connection(), ConnectionState::Connected);
        assert_eq!(status.failure_reason(), None);

        status.note_transport_closure();
        assert_eq!(status.connection(), ConnectionState::Connected);
        assert_eq!(status.failure_reason(), Some(TRANSPORT_CLOSURE_REASON));
        status.member(0).retire();
        status.republish_peer_count();
        assert_eq!(status.connection(), ConnectionState::Reconnecting);

        status.member(1).assign(InterfaceId::new([12; 8]));
        status.republish_peer_count();
        status.note_settled_link();
        assert_eq!(
            recovery_view(&status),
            (
                ConnectionState::Connected,
                None,
                BluetoothRecoveryCounters {
                    ingress_pressure: 1,
                    setup_failures: 1,
                    transport_closures: 1,
                },
                None,
            )
        );
    }

    #[test]
    fn recovery_counters_saturate_as_one_snapshot() {
        static SHARED: BluetoothAutoShared<1> = BluetoothAutoShared::new(InterfaceId::new([13; 8]));
        let status = BluetoothAutoStatus::new(&SHARED);
        SHARED.recovery_counters.lock(|slot| {
            slot.set(BluetoothRecoveryCounters {
                ingress_pressure: u32::MAX,
                setup_failures: u32::MAX,
                transport_closures: u32::MAX,
            });
        });

        status.note_ingress_pressure();
        status.note_setup_failure();
        status.note_transport_closure();

        assert_eq!(
            status.recovery_counters(),
            BluetoothRecoveryCounters {
                ingress_pressure: u32::MAX,
                setup_failures: u32::MAX,
                transport_closures: u32::MAX,
            }
        );
    }
}
