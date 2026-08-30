use super::discovery::{
    BusyOperationGuard, DiscoveryRole, DiscoveryState, DiscoveryWindow, LiveLinkGuard,
};
use super::*;

#[derive(Debug)]
pub struct Closed;

#[derive(Clone, Copy)]
pub(super) struct SeenPeer {
    pub(super) kind: AddrKind,
    pub(super) addr: BdAddr,
    pub(super) rssi: i8,
}

#[derive(Clone, Copy)]
pub(super) struct DialTarget {
    pub(super) kind: AddrKind,
    pub(super) addr: BdAddr,
    transient_client_retries: TransientClientRetries,
}

#[derive(Clone, Copy)]
enum TransientClientRetries {
    EightRemaining,
    SevenRemaining,
    SixRemaining,
    FiveRemaining,
    FourRemaining,
    ThreeRemaining,
    TwoRemaining,
    OneRemaining,
    Exhausted,
}

enum TransientClientRetry {
    Retry(DialTarget),
    Backoff,
}

pub(super) enum TransientClientRetryOutcome {
    Queued,
    Exhausted,
    QueueBusy,
}

#[derive(Clone, Copy)]
struct RecentSighting {
    address: [u8; 6],
    emitted_at_ms: u64,
}

#[derive(Debug, Eq, PartialEq)]
enum SightingAdmissionOutcome {
    Admit,
    Coalesce,
}

struct SightingAdmission {
    recent: [Option<RecentSighting>; SIGHTING_DEPTH],
}

impl SightingAdmission {
    const fn new() -> Self {
        Self {
            recent: [None; SIGHTING_DEPTH],
        }
    }

    fn classify(&mut self, address: [u8; 6], now_ms: u64) -> SightingAdmissionOutcome {
        if let Some(recent) = self
            .recent
            .iter_mut()
            .flatten()
            .find(|recent| recent.address == address)
        {
            if now_ms.saturating_sub(recent.emitted_at_ms) < SIGHTING_COALESCE_MS {
                return SightingAdmissionOutcome::Coalesce;
            }
            recent.emitted_at_ms = now_ms;
            return SightingAdmissionOutcome::Admit;
        }
        let sighting = RecentSighting {
            address,
            emitted_at_ms: now_ms,
        };
        if let Some(slot) = self.recent.iter_mut().find(|entry| entry.is_none()) {
            *slot = Some(sighting);
            return SightingAdmissionOutcome::Admit;
        }
        if let Some(slot) = self.recent.iter_mut().min_by_key(|entry| {
            entry
                .as_ref()
                .map_or(u64::MAX, |recent| recent.emitted_at_ms)
        }) {
            *slot = Some(sighting);
        }
        SightingAdmissionOutcome::Admit
    }
}

impl DialTarget {
    fn new(kind: AddrKind, addr: BdAddr) -> Self {
        Self {
            kind,
            addr,
            transient_client_retries: TransientClientRetries::EightRemaining,
        }
    }

    fn after_transient_client_disconnect(self) -> TransientClientRetry {
        let transient_client_retries = match self.transient_client_retries {
            TransientClientRetries::EightRemaining => TransientClientRetries::SevenRemaining,
            TransientClientRetries::SevenRemaining => TransientClientRetries::SixRemaining,
            TransientClientRetries::SixRemaining => TransientClientRetries::FiveRemaining,
            TransientClientRetries::FiveRemaining => TransientClientRetries::FourRemaining,
            TransientClientRetries::FourRemaining => TransientClientRetries::ThreeRemaining,
            TransientClientRetries::ThreeRemaining => TransientClientRetries::TwoRemaining,
            TransientClientRetries::TwoRemaining => TransientClientRetries::OneRemaining,
            TransientClientRetries::OneRemaining => TransientClientRetries::Exhausted,
            TransientClientRetries::Exhausted => return TransientClientRetry::Backoff,
        };
        TransientClientRetry::Retry(Self {
            kind: self.kind,
            addr: self.addr,
            transient_client_retries,
        })
    }
}

pub(super) enum SlotJob {
    Accept {
        connection: Connection<'static, DefaultPacketPool>,
        slot: BleSlotLease,
    },
    Dial {
        connection: Connection<'static, DefaultPacketPool>,
        slot: BleSlotLease,
        target: DialTarget,
    },
}

pub(super) struct SlotChannels {
    pub(super) control_in: Channel<BridgeMutex, Control, CONTROL_QUEUE_DEPTH>,
    pub(super) control_out: Channel<BridgeMutex, Control, CONTROL_QUEUE_DEPTH>,
    pub(super) data_in: Channel<BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    pub(super) data_out: Channel<BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    pub(super) identity_in: Signal<BridgeMutex, BleIdentity>,
    pub(super) identity_out: Channel<BridgeMutex, BleIdentity, 1>,
    pub(super) data_plane: Signal<BridgeMutex, L2capPlan>,
    pub(super) shutdown: Signal<BridgeMutex, ()>,
    peer_addr: BlockingMutex<BridgeMutex, Cell<[u8; 6]>>,
    peer_protocol: BlockingMutex<BridgeMutex, Cell<PeerProtocol>>,
}

impl SlotChannels {
    const fn new() -> Self {
        Self {
            control_in: Channel::new(),
            control_out: Channel::new(),
            data_in: Channel::new(),
            data_out: Channel::new(),
            identity_in: Signal::new(),
            identity_out: Channel::new(),
            data_plane: Signal::new(),
            shutdown: Signal::new(),
            peer_addr: BlockingMutex::new(Cell::new([0u8; 6])),
            peer_protocol: BlockingMutex::new(Cell::new(PeerProtocol::Native)),
        }
    }

    pub(super) fn set_peer_addr(&self, bytes: [u8; 6]) {
        self.peer_addr.lock(|cell| cell.set(bytes));
    }

    fn addr(&self) -> [u8; 6] {
        self.peer_addr.lock(|cell| cell.get())
    }

    pub(super) fn set_peer_protocol(&self, peer_protocol: PeerProtocol) {
        self.peer_protocol.lock(|cell| cell.set(peer_protocol));
    }

    fn peer_protocol(&self) -> PeerProtocol {
        self.peer_protocol.lock(|cell| cell.get())
    }

    pub(super) fn clear_lanes(&self) {
        self.data_plane.reset();
        self.shutdown.reset();
        self.control_in.clear();
        self.control_out.clear();
        self.data_in.clear();
        self.data_out.clear();
        self.identity_in.reset();
        self.identity_out.clear();
    }

    fn link(
        &'static self,
        slot: BleSlotLink,
        outbound_frames: &'static BleFramePool,
    ) -> EmbeddedBleLink {
        EmbeddedBleLink {
            peer_protocol: self.peer_protocol(),
            control_in: self.control_in.receiver(),
            control_out: self.control_out.sender(),
            data_in: self.data_in.receiver(),
            data_out: self.data_out.sender(),
            identity_in: &self.identity_in,
            identity_out: self.identity_out.sender(),
            data_plane: &self.data_plane,
            plan: L2capPlan::None,
            address: self.addr(),
            outbound_frames,
            slot,
        }
    }
}

pub struct BleHub {
    pub(super) slots: [SlotChannels; PEER_CAPACITY],
    pub(super) connection_slots: BleSlotPool,
    pub(super) assign: [Channel<BridgeMutex, SlotJob, 1>; PEER_CAPACITY],
    pub(super) ready: Channel<BridgeMutex, BleReadySlot, PEER_CAPACITY>,
    pub(super) dial_failed: Channel<BridgeMutex, [u8; 6], PEER_CAPACITY>,
    pub(super) sightings: Channel<BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
    pub(super) dial_request: Channel<BridgeMutex, DialTarget, PEER_CAPACITY>,
    pub(super) inbound_frames: BleFramePool,
    pub(super) outbound_frames: BleFramePool,
    sighting_admission: BlockingMutex<BridgeMutex, RefCell<SightingAdmission>>,
    radio: RadioArbiter,
    discovery_turn: DiscoveryTurnArbiter,
    pub(super) advertise: Signal<BridgeMutex, bool>,
    pub(super) scan_enabled: Signal<BridgeMutex, bool>,
    pub(super) radio_enabled: AtomicBool,
    advertising_wanted: AtomicBool,
    discovery: DiscoveryState,
    pub(super) local_address: BlockingMutex<BridgeMutex, Cell<[u8; 6]>>,
    discovery_group_tag: BlockingMutex<BridgeMutex, Cell<[u8; 4]>>,
    status: BluetoothAutoStatus<PEER_CAPACITY>,
}

impl BleHub {
    pub const fn new(status: BluetoothAutoStatus<PEER_CAPACITY>) -> Self {
        Self {
            slots: [const { SlotChannels::new() }; PEER_CAPACITY],
            connection_slots: ConnectionSlotPool::new(),
            assign: [const { Channel::new() }; PEER_CAPACITY],
            ready: Channel::new(),
            dial_failed: Channel::new(),
            sightings: Channel::new(),
            dial_request: Channel::new(),
            inbound_frames: SharedFramePool::new(),
            outbound_frames: SharedFramePool::new(),
            sighting_admission: BlockingMutex::new(RefCell::new(SightingAdmission::new())),
            radio: FairSemaphore::new(1),
            discovery_turn: FairSemaphore::new(1),
            advertise: Signal::new(),
            scan_enabled: Signal::new(),
            radio_enabled: AtomicBool::new(false),
            advertising_wanted: AtomicBool::new(false),
            discovery: DiscoveryState::new(),
            local_address: BlockingMutex::new(Cell::new([0; 6])),
            discovery_group_tag: BlockingMutex::new(Cell::new(DEFAULT_GROUP_TAG)),
            status,
        }
    }

    pub fn set_local_address(&self, local_address: [u8; 6]) {
        self.local_address.lock(|cell| cell.set(local_address));
    }

    pub fn set_discovery_group_tag(&self, group_tag: [u8; 4]) {
        self.discovery_group_tag.lock(|cell| cell.set(group_tag));
        if self.advertising_wanted.load(Ordering::Relaxed) {
            // Wake the acceptor so it rebuilds manufacturer data without cycling the radio.
            self.advertise.signal(true);
        }
    }

    pub fn discovery_group_tag(&self) -> [u8; 4] {
        self.discovery_group_tag.lock(|cell| cell.get())
    }

    pub(super) async fn acquire_radio(&self) -> RadioPermit<'_> {
        loop {
            match self.radio.acquire(1).await {
                Ok(permit) => return permit,
                Err(_) => yield_now().await,
            }
        }
    }

    fn admit_sighting(&self, address: [u8; 6], now_ms: u64) -> SightingAdmissionOutcome {
        self.sighting_admission
            .lock(|admission| admission.borrow_mut().classify(address, now_ms))
    }

    pub(super) async fn acquire_discovery_turn(&self) -> DiscoveryTurnPermit<'_> {
        loop {
            match self.discovery_turn.acquire(1).await {
                Ok(permit) => return permit,
                Err(_) => yield_now().await,
            }
        }
    }

    pub(super) fn note_ingress_pressure(&self) {
        self.status.note_ingress_pressure();
    }

    pub(super) fn track_live_link(&self) -> LiveLinkGuard<'_> {
        self.discovery.track_live_link()
    }

    pub(super) fn begin_busy_operation(&self) -> BusyOperationGuard<'_> {
        self.discovery.begin_busy_operation()
    }

    pub(super) fn note_link_activity(&self) {
        self.discovery.note_link_activity();
    }

    pub(super) fn retry_transient_client_disconnect(
        &self,
        target: DialTarget,
    ) -> TransientClientRetryOutcome {
        let target = match target.after_transient_client_disconnect() {
            TransientClientRetry::Retry(target) => target,
            TransientClientRetry::Backoff => return TransientClientRetryOutcome::Exhausted,
        };
        match self.dial_request.try_send(target) {
            Ok(()) => TransientClientRetryOutcome::Queued,
            Err(_) => TransientClientRetryOutcome::QueueBusy,
        }
    }

    pub(super) async fn await_discovery_turn(
        &self,
        enabled: &Signal<BridgeMutex, bool>,
        role: DiscoveryRole,
    ) -> Result<DiscoveryWindow, bool> {
        self.discovery.await_turn(enabled, role).await
    }

    pub(super) fn finish_discovery_turn(&self, window: DiscoveryWindow) {
        self.discovery.finish_turn(window);
    }

    pub fn backend(&'static self) -> EmbeddedBleBackend {
        EmbeddedBleBackend {
            hub: self,
            ready: self.ready.receiver(),
            dial_failed: self.dial_failed.receiver(),
            sightings: self.sightings.receiver(),
            dial_request: self.dial_request.sender(),
            seen: heapless::Vec::new(),
        }
    }
}

pub struct EmbeddedBleBackend {
    hub: &'static BleHub,
    ready: Receiver<'static, BridgeMutex, BleReadySlot, PEER_CAPACITY>,
    dial_failed: Receiver<'static, BridgeMutex, [u8; 6], PEER_CAPACITY>,
    sightings: Receiver<'static, BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
    dial_request: Sender<'static, BridgeMutex, DialTarget, PEER_CAPACITY>,
    seen: heapless::Vec<DialTarget, SEEN_CAP>,
}

impl EmbeddedBleBackend {
    fn remember(&mut self, peer: SeenPeer) {
        let target = DialTarget::new(peer.kind, peer.addr);
        if self
            .seen
            .iter()
            .any(|seen| seen.addr.into_inner() == peer.addr.into_inner())
        {
            return;
        }
        if self.seen.push(target).is_err() {
            self.seen.remove(0);
            let _ = self.seen.push(target);
        }
    }

    fn resolve(&self, address: BleAddress) -> Option<DialTarget> {
        self.seen
            .iter()
            .find(|seen| seen.addr.into_inner() == *address.octets())
            .copied()
    }
}

impl BleBackend<PEER_CAPACITY> for EmbeddedBleBackend {
    type Error = Closed;
    type Link = EmbeddedBleLink;

    fn local_group_tag(&self) -> Option<[u8; 4]> {
        Some(self.hub.discovery_group_tag())
    }

    fn drop_all_links(&mut self) {
        for (assign, slot) in self.hub.assign.iter().zip(self.hub.slots.iter()) {
            assign.clear();
            slot.shutdown.signal(());
        }
        for index in 0..PEER_CAPACITY {
            self.hub.connection_slots.request_close(index);
        }
    }

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), Closed> {
        self.hub
            .advertising_wanted
            .store(mode.is_on(), Ordering::Relaxed);
        self.hub.advertise.signal(mode.is_on());
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), Closed> {
        self.hub.scan_enabled.signal(mode.is_on());
        Ok(())
    }

    async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), Closed> {
        let enabled = mode.is_on();
        self.hub.radio_enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.hub.advertising_wanted.store(false, Ordering::Relaxed);
            self.hub.advertise.signal(false);
            self.hub.scan_enabled.signal(false);
            self.hub.dial_request.clear();
            self.hub.dial_failed.clear();
            self.hub.ready.clear();
            for (assign, slot) in self.hub.assign.iter().zip(self.hub.slots.iter()) {
                assign.clear();
                slot.shutdown.signal(());
            }
        }
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<EmbeddedBleLink> {
        match select3(
            self.ready.receive(),
            self.sightings.receive(),
            self.dial_failed.receive(),
        )
        .await
        {
            Either3::First(ready) => {
                let ReadyConnectionSlotParts { origin, link } = ready.into_parts();
                let index = link.index();
                match origin {
                    Origin::Accepted => BleEvent::Inbound(
                        self.hub.slots[index].link(link, &self.hub.outbound_frames),
                    ),
                    Origin::Dialed => BleEvent::LinkReady {
                        link: self.hub.slots[index].link(link, &self.hub.outbound_frames),
                        origin: Origin::Dialed,
                        peer_rssi: None,
                    },
                }
            }
            Either3::Second(peer) => {
                self.remember(peer);
                BleEvent::Sighting {
                    address: BleAddress::new(peer.addr.into_inner()),
                    rssi: Some(peer.rssi),
                }
            }
            Either3::Third(bytes) => BleEvent::DialFailed {
                address: BleAddress::new(bytes),
            },
        }
    }

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        if !self.hub.radio_enabled.load(Ordering::Relaxed) {
            return DialOutcome::RadioOff;
        }
        let Some(target) = self.resolve(address) else {
            return DialOutcome::UnknownPeer;
        };
        if self.dial_request.try_send(target).is_ok() {
            DialOutcome::Started
        } else {
            DialOutcome::Busy
        }
    }
}

pub struct EmbeddedBleLink {
    peer_protocol: PeerProtocol,
    control_in: Receiver<'static, BridgeMutex, Control, CONTROL_QUEUE_DEPTH>,
    control_out: Sender<'static, BridgeMutex, Control, CONTROL_QUEUE_DEPTH>,
    data_in: Receiver<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    data_out: Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    identity_in: &'static Signal<BridgeMutex, BleIdentity>,
    identity_out: Sender<'static, BridgeMutex, BleIdentity, 1>,
    data_plane: &'static Signal<BridgeMutex, L2capPlan>,
    plan: L2capPlan,
    address: [u8; 6],
    outbound_frames: &'static BleFramePool,
    slot: BleSlotLink,
}

impl BleLink for EmbeddedBleLink {
    type Error = Closed;
    type Source = EmbeddedBleSource;
    type Sink = EmbeddedBleSink;

    fn peer_protocol(&self) -> PeerProtocol {
        self.peer_protocol
    }

    fn address(&self) -> BleAddress {
        BleAddress::new(self.address)
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), Closed> {
        match select(self.control_out.send(*msg), self.slot.wait_for_close()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn control_recv(&mut self) -> Result<Control, Closed> {
        match select(self.control_in.receive(), self.slot.wait_for_close()).await {
            Either::First(msg) => Ok(msg),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, Closed> {
        match select(self.identity_in.wait(), self.slot.wait_for_close()).await {
            Either::First(identity) => Ok(identity),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn send_columba_identity(&mut self, identity: BleIdentity) -> Result<(), Closed> {
        match select(self.identity_out.send(identity), self.slot.wait_for_close()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), Closed> {
        self.plan = *plan;
        Ok(())
    }

    fn into_data(self) -> (EmbeddedBleSource, EmbeddedBleSink) {
        self.data_plane.signal(self.plan);
        let ConnectionSlotDataOwners {
            source: source_slot,
            sink: sink_slot,
        } = self.slot.into_data();
        (
            EmbeddedBleSource {
                data_in: self.data_in,
                slot: source_slot,
            },
            EmbeddedBleSink {
                data_out: self.data_out,
                frames: self.outbound_frames,
                slot: sink_slot,
            },
        )
    }
}

pub struct EmbeddedBleSource {
    data_in: Receiver<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    slot: BleSlotSource,
}

impl BleSource for EmbeddedBleSource {
    type Error = Closed;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Closed> {
        match select(self.data_in.receive(), self.slot.wait_for_close()).await {
            Either::First(frame) => {
                let frame = frame.lock().await;
                let len = frame.len().min(out.len());
                out[..len].copy_from_slice(&frame[..len]);
                Ok(len)
            }
            Either::Second(()) => Err(Closed),
        }
    }
}

pub struct EmbeddedBleSink {
    data_out: Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    frames: &'static BleFramePool,
    slot: BleSlotSink,
}

impl BleSink for EmbeddedBleSink {
    type Error = Closed;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Closed> {
        let lease = match select(self.frames.lease(), self.slot.wait_for_close()).await {
            Either::First(Ok(lease)) => lease,
            Either::First(Err(error)) => {
                crate::diagnostic_log::warn!("ble frame lease failed: {error:?}");
                return Err(Closed);
            }
            Either::Second(()) => return Err(Closed),
        };
        lease.fill(frame).await.map_err(|_| Closed)?;
        match select(self.data_out.send(lease), self.slot.wait_for_close()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }
}

pub(super) struct ScanFunnel {
    pub(super) hub: &'static BleHub,
    pub(super) local_address: BleAddress,
}

impl EventHandler for ScanFunnel {
    fn on_adv_reports(&self, reports: LeAdvReportsIter) {
        for report in reports {
            let Ok(report) = report else { continue };
            let peer_address = BleAddress::from_hci_bytes(report.addr.into_inner());
            let capabilities =
                columba_role_capabilities(report.data).unwrap_or(BleRoleCapabilities::DualRole);
            let should_dial = columba_connection_role(
                self.local_address,
                BleRoleCapabilities::DualRole,
                peer_address,
                capabilities,
            ) == ColumbaConnectionRole::Dial;
            if contains_service(report.data)
                && discovery_groups_match(self.hub.discovery_group_tag(), report.data)
                && should_dial
            {
                let address = report.addr.into_inner();
                let outcome = self.hub.admit_sighting(address, Instant::now().as_millis());
                if outcome == SightingAdmissionOutcome::Admit {
                    let _ = self.hub.sightings.try_send(SeenPeer {
                        kind: report.addr_kind,
                        addr: report.addr,
                        rssi: report.rssi,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_client_disconnect_retry_is_bounded() {
        let mut target = DialTarget::new(AddrKind::PUBLIC, BdAddr::new([1, 2, 3, 4, 5, 6]));

        for _ in 0..8 {
            target = match target.after_transient_client_disconnect() {
                TransientClientRetry::Retry(target) => target,
                TransientClientRetry::Backoff => panic!("transient retry exhausted early"),
            };
        }

        assert!(matches!(
            target.after_transient_client_disconnect(),
            TransientClientRetry::Backoff
        ));
    }

    #[test]
    fn sightings_coalesce_per_address_until_the_retry_window() {
        let mut admission = SightingAdmission::new();
        let first = [1, 2, 3, 4, 5, 6];
        let second = [6, 5, 4, 3, 2, 1];

        assert_eq!(
            admission.classify(first, 1_000),
            SightingAdmissionOutcome::Admit
        );
        assert_eq!(
            admission.classify(first, 2_999),
            SightingAdmissionOutcome::Coalesce
        );
        assert_eq!(
            admission.classify(second, 2_999),
            SightingAdmissionOutcome::Admit
        );
        assert_eq!(
            admission.classify(first, 3_000),
            SightingAdmissionOutcome::Admit
        );
    }
}
