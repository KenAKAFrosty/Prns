use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::vec::Vec;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::tcp::tokio_socket::{tune, CONNECT_TIMEOUT};
use crate::wifi_aware::member::WifiAwareMember;
use prns_core::interfaces::wifi_aware::core::{
    AwareDataPlan, AwareEndpoint, NdpRole, RendezvousToken, FAMILY_TAG, MAX_NDP_PEERS,
    WIFI_AWARE_BITRATE_GUESS_BPS,
};
use prns_core::interfaces::wifi_aware::manager::{AwarePolicy, ManagerAction, ManagerInput};
use prns_core::interfaces::wifi_aware::seam::{DiscoveryMode, WifiAwareBackend, WifiAwareEvent};
use prns_core::interfaces::{
    BitrateBps, ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus, TransferRates,
};
use prns_runtime::reactor::driver::TokioInterfaceStatus;
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

const PEER_TRACK: usize = MAX_NDP_PEERS;
const RECENT_MEMBER_GRACE: Duration = Duration::from_secs(3);
const REDIAL_WAIT: Duration = Duration::from_millis(150);
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

pub struct WifiAwareAuto<B> {
    backend: B,
    bitrate: BitrateBps,
    status: WifiAwareStatus,
}

impl<B: WifiAwareBackend> WifiAwareAuto<B> {
    pub fn new(backend: B) -> Self {
        let status = WifiAwareStatus::new(InterfaceId::from_channel_tag(
            InterfaceKind::WifiAware,
            FAMILY_TAG,
        ));
        Self {
            backend,
            bitrate: WIFI_AWARE_BITRATE_GUESS_BPS,
            status,
        }
    }

    #[must_use]
    pub fn with_bitrate(mut self, bitrate: BitrateBps) -> Self {
        self.bitrate = bitrate;
        self
    }

    #[must_use]
    pub fn status(&self) -> WifiAwareStatus {
        self.status.clone()
    }
}

#[derive(Clone)]
pub struct WifiAwareStatus {
    shared: Arc<WifiAwareShared>,
}

struct WifiAwareShared {
    id: InterfaceId,
    enabled: watch::Sender<bool>,
    up: AtomicBool,
    failed: AtomicBool,
    failure_reason: Mutex<Option<&'static str>>,
    unavailable_reason: Mutex<Option<&'static str>>,
    members: Mutex<Vec<TokioInterfaceStatus>>,
    last_member_at: Mutex<Option<Instant>>,
}

impl WifiAwareStatus {
    fn new(id: InterfaceId) -> Self {
        let (enabled, _) = watch::channel(true);
        Self {
            shared: Arc::new(WifiAwareShared {
                id,
                enabled,
                up: AtomicBool::new(false),
                failed: AtomicBool::new(false),
                failure_reason: Mutex::new(None),
                unavailable_reason: Mutex::new(None),
                members: Mutex::new(Vec::new()),
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

    fn set_unavailable(&self, reason: Option<&'static str>) {
        if let Ok(mut slot) = self.shared.unavailable_reason.lock() {
            *slot = reason;
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
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

    #[must_use]
    pub fn members(&self) -> Vec<TokioInterfaceStatus> {
        match self.shared.members.lock() {
            Ok(members) => members.clone(),
            Err(_) => Vec::new(),
        }
    }

    fn set_members(&self, members: Vec<TokioInterfaceStatus>) {
        if !members.is_empty() {
            if let Ok(mut last_member_at) = self.shared.last_member_at.lock() {
                *last_member_at = Some(Instant::now());
            }
        }
        if let Ok(mut slot) = self.shared.members.lock() {
            *slot = members;
        }
    }

    fn unavailable_reason(&self) -> Option<&'static str> {
        self.shared
            .unavailable_reason
            .lock()
            .ok()
            .and_then(|slot| *slot)
    }
}

impl InterfaceStatus for WifiAwareStatus {
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
        } else if self.unavailable_reason().is_some() {
            ConnectionState::Disconnected
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
            .or_else(|| self.unavailable_reason())
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

struct TokioMember {
    attached: AttachedInterface,
    status: TokioInterfaceStatus,
}

struct OpenHandle {
    role: NdpRole,
    task: JoinHandle<()>,
}

/// Everything the running supervisor holds for the live data planes. Aware is many-to-one on the
/// device but one-member-per-peer up here: `members` holds the admitted framed links, `opens` the
/// in-flight dial/accept task for a peer whose keeper path has been chosen but not yet linked, and
/// `endpoints` the address each settled NDP handed back so an `OpenDataPlane` can find its target.
#[derive(Default)]
struct DataPlane {
    members: HashMap<InterfaceId, TokioMember>,
    member_by_peer: HashMap<RendezvousToken, InterfaceId>,
    member_meta: HashMap<InterfaceId, (RendezvousToken, NdpRole)>,
    opens: HashMap<RendezvousToken, OpenHandle>,
    endpoints: HashMap<(RendezvousToken, NdpRole), AwareEndpoint>,
}

impl DataPlane {
    // Tears the member and any in-flight open for a peer, but leaves the settled endpoints alone: a
    // keeper swap emits CloseDataPlane immediately before the OpenDataPlane that reuses the surviving
    // path's endpoint, so wiping them here would strand the responder's listener. Endpoints retire on
    // their own DataPathDown (per role) or on a full teardown.
    fn close_peer(&mut self, peer: RendezvousToken) {
        if let Some(handle) = self.opens.remove(&peer) {
            handle.task.abort();
        }
        if let Some(id) = self.member_by_peer.remove(&peer) {
            if let Some(member) = self.members.remove(&id) {
                member.attached.teardown();
            }
            self.member_meta.remove(&id);
        }
    }

    fn teardown_all(&mut self) {
        for (_, handle) in self.opens.drain() {
            handle.task.abort();
        }
        for (_, member) in self.members.drain() {
            member.attached.teardown();
        }
        self.member_by_peer.clear();
        self.member_meta.clear();
        self.endpoints.clear();
    }
}

enum Opened {
    Ready {
        peer: RendezvousToken,
        role: NdpRole,
        stream: TcpStream,
        addr: SocketAddr,
    },
    Failed {
        peer: RendezvousToken,
        role: NdpRole,
    },
}

enum Step {
    Event(WifiAwareEvent),
    Opened(Opened),
    Closed(InterfaceId),
    Tick,
    Disabled,
}

impl<B: WifiAwareBackend> InterfaceSupervisor for WifiAwareAuto<B> {
    const KIND: InterfaceKind = InterfaceKind::WifiAware;

    fn channel_tag(&self) -> &[u8] {
        FAMILY_TAG
    }

    async fn run(self, fleet: Fleet) {
        let Self {
            mut backend,
            bitrate,
            status,
        } = self;
        if let Some(reason) = backend.blocked() {
            status.mark_failed(Some(reason));
            std::future::pending::<()>().await;
        }
        let started = Instant::now();
        let local = backend.local_token();
        let mut policy = AwarePolicy::<PEER_TRACK>::new(local);
        let mut pending: Vec<ManagerAction> = Vec::new();
        let mut dp = DataPlane::default();
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (opened_tx, mut opened_rx) = mpsc::unbounded_channel::<Opened>();
        status.mark_up();
        policy.start(&mut |action| pending.push(action));
        apply(&mut pending, &mut backend, &mut dp, &opened_tx).await;
        loop {
            if !status.is_enabled() {
                let _ = backend.set_discovery(DiscoveryMode::Off).await;
                dp.teardown_all();
                pending.clear();
                status.set_members(Vec::new());
                status.wait_until_enabled().await;
                policy = AwarePolicy::<PEER_TRACK>::new(local);
                policy.start(&mut |action| pending.push(action));
                apply(&mut pending, &mut backend, &mut dp, &opened_tx).await;
                continue;
            }
            let step = tokio::select! {
                event = backend.next_event() => Step::Event(event),
                Some(opened) = opened_rx.recv() => Step::Opened(opened),
                Some(id) = closed_rx.recv() => Step::Closed(id),
                () = wait_deadline(policy.next_deadline_ms(), started) => Step::Tick,
                () = status.wait_until_disabled() => Step::Disabled,
            };
            let now_ms = started.elapsed().as_millis() as u64;
            let mut emit = |action| pending.push(action);
            match step {
                Step::Disabled => {}
                Step::Tick => policy.handle(ManagerInput::Tick { now_ms }, &mut emit),
                Step::Event(event) => match event {
                    WifiAwareEvent::PeerDiscovered { peer } => {
                        policy.handle(ManagerInput::PeerDiscovered { peer, now_ms }, &mut emit);
                    }
                    WifiAwareEvent::NdpRequested { peer } => {
                        policy.handle(ManagerInput::NdpRequested { peer, now_ms }, &mut emit);
                    }
                    WifiAwareEvent::DataPathUp {
                        peer,
                        role,
                        endpoint,
                    } => {
                        dp.endpoints.insert((peer, role), endpoint);
                        policy.handle(ManagerInput::DataPathUp { peer, role, now_ms }, &mut emit);
                    }
                    WifiAwareEvent::DataPathDown { peer, role, .. } => {
                        dp.endpoints.remove(&(peer, role));
                        policy.handle(ManagerInput::DataPathDown { peer, role, now_ms }, &mut emit);
                    }
                    WifiAwareEvent::NdpFailed { peer, role } => {
                        policy.handle(ManagerInput::NdpFailed { peer, role, now_ms }, &mut emit);
                    }
                    WifiAwareEvent::AvailabilityChanged(state) => {
                        policy.handle(
                            ManagerInput::AvailabilityChanged { state, now_ms },
                            &mut emit,
                        );
                    }
                },
                Step::Opened(Opened::Ready {
                    peer,
                    role,
                    stream,
                    addr,
                }) => {
                    if dp.opens.get(&peer).map(|handle| handle.role) == Some(role) {
                        dp.opens.remove(&peer);
                        let member =
                            WifiAwareMember::new(addr.to_string().into_bytes(), stream, bitrate)
                                .report_close_to(closed_tx.clone());
                        let id = member.id();
                        let member_status = member.status();
                        let attached = fleet.add(member);
                        dp.members.insert(
                            id,
                            TokioMember {
                                attached,
                                status: member_status,
                            },
                        );
                        dp.member_by_peer.insert(peer, id);
                        dp.member_meta.insert(id, (peer, role));
                    }
                }
                Step::Opened(Opened::Failed { peer, role }) => {
                    if dp.opens.get(&peer).map(|handle| handle.role) == Some(role) {
                        dp.opens.remove(&peer);
                        policy.handle(ManagerInput::DataPathDown { peer, role, now_ms }, &mut emit);
                    }
                }
                Step::Closed(id) => {
                    if let Some((peer, role)) = dp.member_meta.get(&id).copied() {
                        policy.handle(ManagerInput::DataPathDown { peer, role, now_ms }, &mut emit);
                    }
                }
            }
            apply(&mut pending, &mut backend, &mut dp, &opened_tx).await;
            status.set_members(
                dp.members
                    .values()
                    .map(|member| member.status.clone())
                    .collect(),
            );
            status.set_unavailable(policy.park_reason());
        }
    }
}

impl<B: WifiAwareBackend> prns_core::interfaces::ReportsStatus for WifiAwareAuto<B> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

async fn apply<B: WifiAwareBackend>(
    pending: &mut Vec<ManagerAction>,
    backend: &mut B,
    dp: &mut DataPlane,
    opened: &mpsc::UnboundedSender<Opened>,
) {
    let actions = std::mem::take(pending);
    for action in actions {
        match action {
            ManagerAction::SetDiscovery(mode) => {
                let _ = backend.set_discovery(mode).await;
            }
            ManagerAction::RequestDataPath { peer, role } => {
                backend.request_data_path(peer, role).await;
            }
            ManagerAction::AbandonDataPath { peer, role } => {
                backend.abandon_data_path(peer, role).await;
            }
            ManagerAction::OpenDataPlane { peer, role } => {
                if let Some(handle) = dp.opens.remove(&peer) {
                    handle.task.abort();
                }
                let Some(endpoint) = dp.endpoints.get(&(peer, role)).copied() else {
                    continue;
                };
                let plan = role.data_plane(endpoint);
                let sink = opened.clone();
                let task = tokio::spawn(open_path(peer, role, plan, sink));
                dp.opens.insert(peer, OpenHandle { role, task });
            }
            ManagerAction::CloseDataPlane { peer } => dp.close_peer(peer),
        }
    }
}

async fn open_path(
    peer: RendezvousToken,
    role: NdpRole,
    plan: AwareDataPlan,
    sink: mpsc::UnboundedSender<Opened>,
) {
    let linked = match plan {
        AwareDataPlan::Dial { addr, scope, port } => dial(addr, scope, port).await,
        AwareDataPlan::Listen { addr, scope, port } => listen(addr, scope, port).await,
    };
    let message = match linked {
        Some((stream, addr)) => Opened::Ready {
            peer,
            role,
            stream,
            addr,
        },
        None => Opened::Failed { peer, role },
    };
    let _ = sink.send(message);
}

async fn dial(addr: Ipv6Addr, scope: u32, port: u16) -> Option<(TcpStream, SocketAddr)> {
    let target = SocketAddr::V6(SocketAddrV6::new(addr, port, 0, scope));
    let deadline = Instant::now() + OPEN_TIMEOUT;
    loop {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target)).await {
            Ok(Ok(stream)) => {
                tune(&stream);
                return Some((stream, target));
            }
            _ => {
                if Instant::now() >= deadline {
                    return None;
                }
                tokio::time::sleep(REDIAL_WAIT).await;
            }
        }
    }
}

async fn listen(addr: Ipv6Addr, scope: u32, port: u16) -> Option<(TcpStream, SocketAddr)> {
    let bind = SocketAddr::V6(SocketAddrV6::new(addr, port, 0, scope));
    let listener = TcpListener::bind(bind).await.ok()?;
    match tokio::time::timeout(OPEN_TIMEOUT, listener.accept()).await {
        Ok(Ok((stream, addr))) => {
            tune(&stream);
            Some((stream, addr))
        }
        _ => None,
    }
}

async fn wait_deadline(deadline_ms: Option<u64>, started: Instant) {
    match deadline_ms {
        Some(at_ms) => {
            let now_ms = started.elapsed().as_millis() as u64;
            tokio::time::sleep(Duration::from_millis(at_ms.saturating_sub(now_ms))).await;
        }
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn revocation_reads_as_disconnected_with_the_reason_never_failed() {
        let status = WifiAwareStatus::new(InterfaceId::new([0xA1; 8]));
        status.mark_up();

        status.set_unavailable(Some("Wi-Fi Aware disabled by the platform"));
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert_eq!(
            status.failure_reason(),
            Some("Wi-Fi Aware disabled by the platform")
        );

        status.set_unavailable(None);
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert_eq!(status.failure_reason(), None);
    }

    #[test]
    fn a_blocked_backend_reads_as_failed_with_its_reason() {
        let status = WifiAwareStatus::new(InterfaceId::new([0xA1; 8]));
        status.mark_failed(Some("no Wi-Fi Aware on this platform"));
        assert_eq!(status.connection(), ConnectionState::Failed);
        assert_eq!(
            status.failure_reason(),
            Some("no Wi-Fi Aware on this platform")
        );
    }

    // A minimal Aware fabric over channels: each node announces the peer once, and an initiator's
    // request sends a Setup across so the peer surfaces an NdpRequested (its responder half). Every
    // requested path settles a loopback endpoint on a shared port, so the keeper duel plays out and
    // the survivor forms a real TCP link.
    enum Fabric {
        Setup,
    }

    struct LoopbackAwareBackend {
        local: RendezvousToken,
        peer: RendezvousToken,
        port: u16,
        announced: bool,
        queued: VecDeque<WifiAwareEvent>,
        to_peer: mpsc::Sender<Fabric>,
        from_peer: mpsc::Receiver<Fabric>,
    }

    impl LoopbackAwareBackend {
        fn pair(port: u16) -> (Self, Self) {
            let (a_tx, a_rx) = mpsc::channel(8);
            let (b_tx, b_rx) = mpsc::channel(8);
            let a = Self {
                local: RendezvousToken::new(1),
                peer: RendezvousToken::new(2),
                port,
                announced: false,
                queued: VecDeque::new(),
                to_peer: b_tx,
                from_peer: a_rx,
            };
            let b = Self {
                local: RendezvousToken::new(2),
                peer: RendezvousToken::new(1),
                port,
                announced: false,
                queued: VecDeque::new(),
                to_peer: a_tx,
                from_peer: b_rx,
            };
            (a, b)
        }

        fn endpoint(&self) -> AwareEndpoint {
            AwareEndpoint {
                addr: Ipv6Addr::LOCALHOST,
                scope: 0,
                port: self.port,
            }
        }
    }

    impl WifiAwareBackend for LoopbackAwareBackend {
        type Error = std::convert::Infallible;

        fn local_token(&self) -> RendezvousToken {
            self.local
        }

        async fn set_discovery(&mut self, _mode: DiscoveryMode) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn request_data_path(&mut self, peer: RendezvousToken, role: NdpRole) {
            if matches!(role, NdpRole::Initiator) {
                let _ = self.to_peer.send(Fabric::Setup).await;
            }
            self.queued.push_back(WifiAwareEvent::DataPathUp {
                peer,
                role,
                endpoint: self.endpoint(),
            });
        }

        async fn abandon_data_path(&mut self, _peer: RendezvousToken, _role: NdpRole) {}

        async fn next_event(&mut self) -> WifiAwareEvent {
            if !self.announced {
                self.announced = true;
                return WifiAwareEvent::PeerDiscovered { peer: self.peer };
            }
            if let Some(event) = self.queued.pop_front() {
                return event;
            }
            match self.from_peer.recv().await {
                Some(Fabric::Setup) => WifiAwareEvent::NdpRequested { peer: self.peer },
                None => std::future::pending().await,
            }
        }
    }

    fn free_loopback_port() -> u16 {
        let probe =
            std::net::TcpListener::bind("[::1]:0").expect("binds an ephemeral loopback port");
        probe
            .local_addr()
            .expect("the bound address is known")
            .port()
    }

    #[tokio::test]
    async fn two_supervisors_settle_the_keeper_path_and_link_over_loopback() {
        let (backend_a, backend_b) = LoopbackAwareBackend::pair(free_loopback_port());
        let auto_a = WifiAwareAuto::new(backend_a);
        let auto_b = WifiAwareAuto::new(backend_b);
        let status_a = auto_a.status();
        let status_b = auto_b.status();
        let (fleet_a, _tail_a) = Fleet::detached(InterfaceId::new([0xA0; 8]));
        let (fleet_b, _tail_b) = Fleet::detached(InterfaceId::new([0xB0; 8]));
        tokio::spawn(auto_a.run(fleet_a));
        tokio::spawn(auto_b.run(fleet_b));

        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if status_a.connection() == ConnectionState::Connected
                && status_b.connection() == ConnectionState::Connected
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "both nodes link over the keeper NDP within the window",
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
