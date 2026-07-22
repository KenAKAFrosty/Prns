use ::core::cell::Cell;

use embassy_futures::select::{select3, select4, select_array, Either3, Either4};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, RawMutex};
use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Instant};
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use prns_core::engine::FanTarget;
use prns_core::interfaces::bluetooth_auto::{
    self as contract, BleAddress, BleIdentity, Endpoint, EstablishedPeer, EstablishedTransport,
    Handshake, HandshakeOutcome, HandshakeRole, L2capPlan, LinkCapabilities, LocalPeer,
    PeerProtocol,
};
use prns_core::interfaces::bluetooth_auto::{
    role_for, ConnectionPolicy, PolicyAction, PolicyInput,
};
use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin, ScanningMode,
};
use prns_core::interfaces::{
    BitrateBps, ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus,
};
use prns_runtime::reactor::grant::FrameTarget;
use prns_runtime::runtime::EmbassyFleet as Fleet;

const DIAL_TRACK: usize = 6;

const ACTION_CAP: usize = 6;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

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
    peers: AtomicU32,
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
            peers: AtomicU32::new(0),
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
    pub fn new(shared: &'static BluetoothAutoShared<MEMBERS>) -> Self {
        Self { shared }
    }

    fn mark_up(&self) {
        self.shared.up.store(true, Ordering::Relaxed);
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
        } else if !self.shared.up.load(Ordering::Relaxed) {
            ConnectionState::Initializing
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
}

struct Active<L: BleLink> {
    identity: BleIdentity,
    id: InterfaceId,
    slot: usize,
    address: BleAddress,
    source: L::Source,
    sink: L::Sink,
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
        shared: &'static BluetoothAutoShared<MEMBERS>,
    ) -> Self {
        Self {
            backend,
            local: LocalPeer {
                identity,
                endpoint,
                capabilities,
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
            local,
            status,
            bitrate,
        } = self;
        let mut manager = ConnectionPolicy::<MEMBERS, DIAL_TRACK>::new(local);
        let mut members: [Option<Active<B::Link>>; MEMBERS] = [const { None }; MEMBERS];
        let mut inbufs: [[u8; contract::BLE_HW_MTU]; MEMBERS] =
            [[0u8; contract::BLE_HW_MTU]; MEMBERS];
        let mut pending: heapless::Vec<PolicyAction, ACTION_CAP> = heapless::Vec::new();
        status.mark_up();
        manager.start(&mut |action| {
            let _ = pending.push(action);
        });
        apply_radio(
            &mut pending,
            &status,
            &mut fleet,
            &mut backend,
            &mut members,
        )
        .await;

        loop {
            if !status.is_enabled() {
                disable_members(&status, &mut fleet, &mut backend, &mut members).await;
                pending.clear();
                status.wait_until_enabled().await;
                manager = ConnectionPolicy::<MEMBERS, DIAL_TRACK>::new(local);
                manager.start(&mut |action| {
                    let _ = pending.push(action);
                });
                apply_radio(
                    &mut pending,
                    &status,
                    &mut fleet,
                    &mut backend,
                    &mut members,
                )
                .await;
                continue;
            }
            let outcome = select4(
                backend.next_event(),
                fleet.outbound_ready(),
                recv_any(&mut members, &mut inbufs),
                status.wait_until_disabled(),
            )
            .await;
            let now_ms = Instant::now().as_millis();
            match outcome {
                Either4::First(BleEvent::Sighting { address, .. }) => {
                    manager.handle(PolicyInput::Sighting { address, now_ms }, &mut |action| {
                        let _ = pending.push(action);
                    });
                    apply_radio(
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                    )
                    .await;
                }
                Either4::First(BleEvent::Inbound(link)) => {
                    settle_into_fleet(
                        link,
                        Origin::Accepted,
                        local,
                        bitrate,
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
                Either4::First(BleEvent::LinkReady { link, origin, .. }) => {
                    settle_into_fleet(
                        link,
                        origin,
                        local,
                        bitrate,
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
                Either4::First(BleEvent::DialFailed { address }) => {
                    manager.handle(PolicyInput::DialFailed { address, now_ms }, &mut |action| {
                        let _ = pending.push(action);
                    });
                    apply_radio(
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                    )
                    .await;
                }
                Either4::Second(()) => {
                    drain_outbound(
                        &mut manager,
                        &mut pending,
                        &status,
                        &mut fleet,
                        &mut backend,
                        &mut members,
                    )
                    .await;
                }
                Either4::Third((index, received)) => {
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
                Either4::Fourth(()) => {}
            }
        }
    }
}

/// L2CAP upgrade waits until [`settle_into_fleet`] admits the keeper so a rejected dial-race connection never consumes a radio-contending setup window.
async fn settle<L: BleLink>(
    mut link: L,
    role: HandshakeRole,
    local: LocalPeer,
) -> Option<(EstablishedPeer, L)> {
    let established = drive_handshake(&mut link, role, local).await?;
    Some((established, link))
}

async fn drive_handshake<L: BleLink>(
    link: &mut L,
    role: HandshakeRole,
    local: LocalPeer,
) -> Option<EstablishedPeer> {
    with_timeout(HANDSHAKE_TIMEOUT, async {
        if link.peer_protocol() == PeerProtocol::Columba {
            let identity = link.receive_columba_peer_identity().await.ok()?;
            if role == HandshakeRole::Dialer {
                link.send_columba_identity(local.identity).await.ok()?;
            }
            return Some(EstablishedPeer {
                identity,
                transport: EstablishedTransport::ColumbaGatt,
                peer_rssi: None,
            });
        }
        let (mut handshake, opening) = Handshake::begin(role, local, None);
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
                HandshakeOutcome::Settled(established) => return Some(established),
                HandshakeOutcome::Aborted(_) => return None,
                HandshakeOutcome::Pending => {}
            }
        }
    })
    .await
    .ok()
    .flatten()
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
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    match action {
        PolicyAction::Dial(address) => backend.dial(address).await,
        PolicyAction::Evict { slot, .. } => {
            if let Some(id) = members[slot].as_ref().map(|member| member.id) {
                fleet.deregister_member(id).await;
                let _ = members[slot].take();
                status.member(slot).retire();
                status.republish_peer_count();
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

async fn apply_radio<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    pending: &mut heapless::Vec<PolicyAction, ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    let actions = ::core::mem::take(pending);
    for action in actions {
        apply_one(action, status, fleet, backend, members).await;
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
    let _ = backend.set_advertising(AdvertisingMode::Off).await;
    let _ = backend.set_scanning(ScanningMode::Off).await;
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
    pending: &mut heapless::Vec<PolicyAction, ACTION_CAP>,
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
        &mut |action| {
            let _ = pending.push(action);
        },
    );
    apply_radio(pending, status, fleet, backend, members).await;
}

async fn drain_outbound<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    manager: &mut ConnectionPolicy<MEMBERS, DIAL_TRACK>,
    pending: &mut heapless::Vec<PolicyAction, ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    while let Some(frame) = fleet.try_next_outbound() {
        if frame.is_empty() {
            continue;
        }
        for slot in 0..MEMBERS {
            let selected = match members[slot].as_ref() {
                Some(member) => match frame.target() {
                    FrameTarget::Direct(id) => member.id == id,
                    FrameTarget::Fan(FanTarget::Only(id)) => member.id == id,
                    FrameTarget::Fan(FanTarget::All) => true,
                    FrameTarget::Fan(FanTarget::AllExcept(id)) => member.id != id,
                },
                None => false,
            };
            if !selected {
                continue;
            }
            let sent = match members[slot].as_mut() {
                Some(member) => member.sink.send_frame(frame.bytes()).await.is_ok(),
                None => false,
            };
            if sent {
                status.member(slot).add_tx(frame.len() as u64);
            } else {
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
    pending: &mut heapless::Vec<PolicyAction, ACTION_CAP>,
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
                    .try_deliver_inbound(member.id, &inbufs[index][..len])
                    .is_ok()
                {
                    status.member(member.slot).add_rx(len as u64);
                }
            }
        }
        Err(()) => {
            close_member(index, manager, pending, status, fleet, backend, members).await;
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "embedded serve-loop internals pass the loop's split-borrowed locals; bundling awaits an on-hardware validation pass"
)]
async fn settle_into_fleet<
    B,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    link: B::Link,
    origin: Origin,
    local: LocalPeer,
    bitrate: BitrateBps,
    manager: &mut ConnectionPolicy<MEMBERS, DIAL_TRACK>,
    pending: &mut heapless::Vec<PolicyAction, ACTION_CAP>,
    status: &BluetoothAutoStatus<MEMBERS>,
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    backend: &mut B,
    members: &mut [Option<Active<B::Link>>; MEMBERS],
    inbufs: &mut [[u8; contract::BLE_HW_MTU]; MEMBERS],
) where
    B: BleBackend<MEMBERS>,
{
    let address = link.address();
    let role = role_for(origin);
    let mut handshake = ::core::pin::pin!(settle(link, role, local));
    let settled = loop {
        match select3(
            handshake.as_mut(),
            fleet.outbound_ready(),
            recv_any(members, inbufs),
        )
        .await
        {
            Either3::First(result) => break result,
            Either3::Second(()) => {
                drain_outbound(manager, pending, status, fleet, backend, members).await;
            }
            Either3::Third((index, received)) => {
                deliver_inbound(
                    index,
                    received.map_err(|_| ()),
                    manager,
                    pending,
                    status,
                    fleet,
                    backend,
                    members,
                    inbufs,
                )
                .await;
            }
        }
    };
    let now_ms = Instant::now().as_millis();
    let Some((established, link)) = settled else {
        manager.handle(
            PolicyInput::HandshakeFailed { address, origin },
            &mut |action| {
                let _ = pending.push(action);
            },
        );
        apply_radio(pending, status, fleet, backend, members).await;
        return;
    };
    manager.handle(
        PolicyInput::Settled {
            address,
            origin,
            established,
            now_ms,
        },
        &mut |action| {
            let _ = pending.push(action);
        },
    );
    let actions = ::core::mem::take(pending);
    let mut held = Some(link);
    for action in actions {
        match action {
            PolicyAction::Admit {
                identity,
                slot,
                address,
                lane,
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
            PolicyAction::Reject { .. } => held = None,
            other => apply_one(other, status, fleet, backend, members).await,
        }
    }
}
