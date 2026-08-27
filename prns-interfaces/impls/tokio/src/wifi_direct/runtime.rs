use std::collections::HashMap;
use std::io;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::vec::Vec;

use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};

use crate::tcp::{tune, CONNECT_TIMEOUT};
use crate::wifi_direct::member::WifiDirectMember;
use prns_core::interfaces::wifi_auto::{
    classify_beacon, peering_token, BeaconVerdict, DISCOVERY_GROUP,
};
use prns_core::interfaces::wifi_direct::{
    DataPlanePlan, GoIntent, GroupRole, SegmentAddress, FAMILY_TAG, WIFI_DIRECT_BEACON_PORT,
    WIFI_DIRECT_BITRATE_GUESS_BPS, WIFI_DIRECT_RENDEZVOUS_PORT,
};
use prns_core::interfaces::wifi_direct::{
    DiscoveryMode, WifiDirectBackend, WifiDirectEvent, WifiDirectGroup,
};
use prns_core::interfaces::wifi_direct::{GroupPolicy, PolicyAction, PolicyInput};
use prns_core::interfaces::{
    BitrateBps, ConnectionState, EffectiveInterfacePolicy, InterfaceId, InterfaceKind,
    InterfaceStatus, TransferRates,
};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

const DIAL_TRACK: usize = 16;
const RECENT_MEMBER_GRACE: Duration = Duration::from_secs(3);
const REDIAL_WAIT: Duration = Duration::from_secs(3);
const BEACON_PERIOD: Duration = Duration::from_secs(2);

pub struct WifiDirectAuto<B> {
    backend: B,
    intent: GoIntent,
    interface_policy: EffectiveInterfacePolicy,
    status: WifiDirectStatus,
}

impl<B: WifiDirectBackend> WifiDirectAuto<B> {
    pub fn new(backend: B, intent: GoIntent) -> Self {
        let status = WifiDirectStatus::new(InterfaceId::from_channel_tag(
            InterfaceKind::WifiDirect,
            FAMILY_TAG,
        ));
        Self {
            backend,
            intent,
            interface_policy: prns_core::interfaces::wifi_direct::defaults_for_bitrate(
                WIFI_DIRECT_BITRATE_GUESS_BPS,
            )
            .configured(Default::default()),
            status,
        }
    }

    #[must_use]
    pub fn with_bitrate(mut self, bitrate: BitrateBps) -> Self {
        self.interface_policy = prns_core::interfaces::wifi_direct::defaults_for_bitrate(bitrate)
            .configured(Default::default());
        self
    }

    #[must_use]
    pub fn with_policy(mut self, policy: EffectiveInterfacePolicy) -> Self {
        self.interface_policy = policy;
        self
    }

    #[must_use]
    pub fn status(&self) -> WifiDirectStatus {
        self.status.clone()
    }
}

#[derive(Clone)]
pub struct WifiDirectStatus {
    shared: Arc<WifiDirectShared>,
}

struct WifiDirectShared {
    id: InterfaceId,
    enabled: watch::Sender<bool>,
    up: AtomicBool,
    failed: AtomicBool,
    failure_reason: Mutex<Option<&'static str>>,
    unavailable_reason: Mutex<Option<&'static str>>,
    members: Mutex<Vec<TokioInterfaceStatus>>,
    last_member_at: Mutex<Option<Instant>>,
}

impl WifiDirectStatus {
    fn new(id: InterfaceId) -> Self {
        let (enabled, _) = watch::channel(true);
        Self {
            shared: Arc::new(WifiDirectShared {
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

impl InterfaceStatus for WifiDirectStatus {
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

struct BeaconRig {
    socket: UdpSocket,
    interval: tokio::time::Interval,
    local: Ipv6Addr,
    dest: SocketAddr,
}

enum Plane {
    Down,
    Hosting {
        listener: TcpListener,
        beacon: Option<BeaconRig>,
    },
    Resolving {
        socket: UdpSocket,
        local: Ipv6Addr,
        scope: u32,
    },
    Dialing {
        target: SocketAddr,
    },
    Linked {
        target: SocketAddr,
    },
}

enum PlaneEvent {
    Accepted(TcpStream, SocketAddr),
    Dialed(TcpStream, SocketAddr),
    Resolved(SocketAddr),
    Nothing,
}

enum Step<G> {
    Event(WifiDirectEvent<G>),
    Plane(PlaneEvent),
    Closed(InterfaceId),
    Tick,
    Disabled,
}

impl<B: WifiDirectBackend> InterfaceSupervisor for WifiDirectAuto<B> {
    const KIND: InterfaceKind = InterfaceKind::WifiDirect;

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.interface_policy
    }

    fn channel_tag(&self) -> &[u8] {
        FAMILY_TAG
    }

    async fn run(self, fleet: Fleet) {
        let Self {
            mut backend,
            intent,
            interface_policy,
            status,
        } = self;
        if let Some(reason) = backend.blocked() {
            status.mark_failed(Some(reason));
            std::future::pending::<()>().await;
        }
        let started = Instant::now();
        let mut policy = GroupPolicy::<DIAL_TRACK>::new(intent);
        let mut pending: Vec<PolicyAction> = Vec::new();
        let mut members: HashMap<InterfaceId, TokioMember> = HashMap::new();
        let mut plane = Plane::Down;
        let mut current_plan: Option<DataPlanePlan> = None;
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<InterfaceId>();
        status.mark_up();
        policy.start(&mut |action| pending.push(action));
        apply(
            &mut pending,
            &mut backend,
            &mut plane,
            &mut members,
            &current_plan,
            intent,
        )
        .await;
        loop {
            if !status.is_enabled() {
                let _ = backend.set_discovery(DiscoveryMode::Off).await;
                backend.remove_group().await;
                for (_, member) in members.drain() {
                    member.attached.teardown();
                }
                plane = Plane::Down;
                current_plan = None;
                pending.clear();
                status.set_members(Vec::new());
                status.wait_until_enabled().await;
                policy = GroupPolicy::<DIAL_TRACK>::new(intent);
                policy.start(&mut |action| pending.push(action));
                apply(
                    &mut pending,
                    &mut backend,
                    &mut plane,
                    &mut members,
                    &current_plan,
                    intent,
                )
                .await;
                continue;
            }
            let step = tokio::select! {
                event = backend.next_event() => Step::Event(event),
                plane_event = plane_step(&mut plane) => Step::Plane(plane_event),
                Some(id) = closed_rx.recv() => Step::Closed(id),
                () = wait_formation_deadline(policy.formation_deadline_ms(), started) => Step::Tick,
                () = status.wait_until_disabled() => Step::Disabled,
            };
            let now_ms = started.elapsed().as_millis() as u64;
            let mut emit = |action| pending.push(action);
            match step {
                Step::Disabled => {}
                Step::Tick => policy.handle(PolicyInput::Tick { now_ms }, &mut emit),
                Step::Event(event) => match event {
                    WifiDirectEvent::Sighting {
                        peer, initiative, ..
                    } => {
                        policy.handle(
                            PolicyInput::Sighting {
                                peer,
                                initiative,
                                now_ms,
                            },
                            &mut emit,
                        );
                    }
                    WifiDirectEvent::PeerGone { .. } => {}
                    WifiDirectEvent::Invitation { peer } => {
                        policy.handle(PolicyInput::Invitation { peer, now_ms }, &mut emit);
                    }
                    WifiDirectEvent::GroupOffer { peer } => {
                        policy.handle(PolicyInput::GroupOffer { peer, now_ms }, &mut emit);
                    }
                    WifiDirectEvent::GroupFormed { group } => {
                        current_plan = Some(group.data_plane());
                        let role = group.role();
                        policy.handle(PolicyInput::GroupFormed { role, now_ms }, &mut emit);
                    }
                    WifiDirectEvent::GroupLost { .. } => {
                        current_plan = None;
                        policy.handle(PolicyInput::GroupLost { now_ms }, &mut emit);
                    }
                    WifiDirectEvent::FormationFailed { peer } => {
                        policy.handle(PolicyInput::FormationFailed { peer, now_ms }, &mut emit);
                    }
                    WifiDirectEvent::FormationProgress => {
                        policy.handle(PolicyInput::FormationProgress { now_ms }, &mut emit);
                    }
                    WifiDirectEvent::AvailabilityChanged(state) => {
                        policy.handle(
                            PolicyInput::AvailabilityChanged { state, now_ms },
                            &mut emit,
                        );
                    }
                },
                Step::Plane(PlaneEvent::Accepted(stream, peer)) => {
                    admit(
                        stream,
                        peer,
                        interface_policy,
                        &fleet,
                        &closed_tx,
                        &mut members,
                    );
                    policy.handle(
                        PolicyInput::MembersChanged {
                            count: members.len(),
                        },
                        &mut emit,
                    );
                }
                Step::Plane(PlaneEvent::Dialed(stream, target)) => {
                    admit(
                        stream,
                        target,
                        interface_policy,
                        &fleet,
                        &closed_tx,
                        &mut members,
                    );
                    plane = Plane::Linked { target };
                    policy.handle(
                        PolicyInput::MembersChanged {
                            count: members.len(),
                        },
                        &mut emit,
                    );
                }
                Step::Plane(PlaneEvent::Resolved(target)) => {
                    plane = Plane::Dialing { target };
                }
                Step::Plane(PlaneEvent::Nothing) => {}
                Step::Closed(id) => {
                    if let Some(member) = members.remove(&id) {
                        member.attached.teardown();
                    }
                    if let Plane::Linked { target } = plane {
                        if policy.role() == Some(GroupRole::Client) {
                            plane = Plane::Dialing { target };
                        }
                    }
                    policy.handle(
                        PolicyInput::MembersChanged {
                            count: members.len(),
                        },
                        &mut emit,
                    );
                }
            }
            apply(
                &mut pending,
                &mut backend,
                &mut plane,
                &mut members,
                &current_plan,
                intent,
            )
            .await;
            status.set_members(
                members
                    .values()
                    .map(|member| member.status.clone())
                    .collect(),
            );
            status.set_unavailable(policy.phase_reason());
        }
    }
}

impl<B: WifiDirectBackend> prns_core::interfaces::ReportsStatus for WifiDirectAuto<B> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

async fn apply<B: WifiDirectBackend>(
    pending: &mut Vec<PolicyAction>,
    backend: &mut B,
    plane: &mut Plane,
    members: &mut HashMap<InterfaceId, TokioMember>,
    current_plan: &Option<DataPlanePlan>,
    intent: GoIntent,
) {
    let actions = std::mem::take(pending);
    for action in actions {
        match action {
            PolicyAction::SetDiscovery(mode) => {
                let _ = backend.set_discovery(mode).await;
            }
            PolicyAction::Form { peer, intent } => backend.form_group(peer, intent).await,
            PolicyAction::Accept { peer } => backend.accept_invitation(peer, intent).await,
            PolicyAction::Join { peer } => backend.join_group(peer).await,
            PolicyAction::RemoveGroup => backend.remove_group().await,
            PolicyAction::OpenDataPlane { .. } => {
                *plane = match current_plan {
                    Some(plan) => open_plane(*plan).await,
                    None => Plane::Down,
                };
            }
            PolicyAction::CloseDataPlane => {
                for (_, member) in members.drain() {
                    member.attached.teardown();
                }
                *plane = Plane::Down;
            }
        }
    }
}

fn admit(
    stream: TcpStream,
    peer: SocketAddr,
    policy: EffectiveInterfacePolicy,
    fleet: &Fleet,
    closed: &mpsc::UnboundedSender<InterfaceId>,
    members: &mut HashMap<InterfaceId, TokioMember>,
) {
    let member = WifiDirectMember::with_policy(peer.to_string().into_bytes(), stream, policy)
        .report_close_to(closed.clone());
    let id = member.id();
    let status = member.status();
    let attached = fleet.add(member);
    members.insert(id, TokioMember { attached, status });
}

async fn wait_formation_deadline(deadline_ms: Option<u64>, started: Instant) {
    match deadline_ms {
        Some(at_ms) => {
            let now_ms = started.elapsed().as_millis() as u64;
            tokio::time::sleep(Duration::from_millis(at_ms.saturating_sub(now_ms))).await;
        }
        None => std::future::pending().await,
    }
}

async fn plane_step(plane: &mut Plane) -> PlaneEvent {
    match plane {
        Plane::Down | Plane::Linked { .. } => std::future::pending().await,
        Plane::Hosting { listener, beacon } => match beacon {
            Some(rig) => {
                tokio::select! {
                    accepted = listener.accept() => accepted_event(accepted).await,
                    _ = rig.interval.tick() => {
                        let token = peering_token(&rig.local);
                        let _ = rig.socket.send_to(token.as_bytes(), rig.dest).await;
                        PlaneEvent::Nothing
                    }
                }
            }
            None => accepted_event(listener.accept().await).await,
        },
        Plane::Resolving {
            socket,
            local,
            scope,
        } => {
            let owner = await_owner_beacon(socket, local).await;
            PlaneEvent::Resolved(SocketAddr::V6(SocketAddrV6::new(
                owner,
                WIFI_DIRECT_RENDEZVOUS_PORT,
                0,
                *scope,
            )))
        }
        Plane::Dialing { target } => {
            match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(*target)).await {
                Ok(Ok(stream)) => {
                    tune(&stream);
                    PlaneEvent::Dialed(stream, *target)
                }
                _ => {
                    tokio::time::sleep(REDIAL_WAIT).await;
                    PlaneEvent::Nothing
                }
            }
        }
    }
}

async fn accepted_event(accepted: io::Result<(TcpStream, SocketAddr)>) -> PlaneEvent {
    match accepted {
        Ok((stream, peer)) => {
            tune(&stream);
            PlaneEvent::Accepted(stream, peer)
        }
        Err(_) => {
            tokio::time::sleep(REDIAL_WAIT).await;
            PlaneEvent::Nothing
        }
    }
}

async fn await_owner_beacon(socket: &UdpSocket, local: &Ipv6Addr) -> Ipv6Addr {
    let mut buf = [0u8; 64];
    loop {
        let Ok((len, src)) = socket.recv_from(&mut buf).await else {
            tokio::time::sleep(REDIAL_WAIT).await;
            continue;
        };
        let SocketAddr::V6(src6) = src else {
            continue;
        };
        if let BeaconVerdict::Peer(owner) = classify_beacon(&buf[..len], src6.ip(), local) {
            return owner;
        }
    }
}

async fn open_plane(plan: DataPlanePlan) -> Plane {
    match plan {
        DataPlanePlan::HostRendezvous { local } => {
            let bind = segment_socket(local, WIFI_DIRECT_RENDEZVOUS_PORT);
            match TcpListener::bind(bind).await {
                Ok(listener) => Plane::Hosting {
                    listener,
                    beacon: beacon_rig(local).await,
                },
                Err(err) => {
                    crate::diagnostic_log::warn!(
                        "wifi-direct rendezvous bind on {bind} failed: {err}"
                    );
                    Plane::Down
                }
            }
        }
        DataPlanePlan::DialOwner { owner } => Plane::Dialing {
            target: segment_socket(owner, WIFI_DIRECT_RENDEZVOUS_PORT),
        },
        DataPlanePlan::ResolveOwnerByBeacon { local, scope } => match resolve_socket(scope).await {
            Some(socket) => Plane::Resolving {
                socket,
                local,
                scope,
            },
            None => Plane::Down,
        },
    }
}

async fn beacon_rig(local: SegmentAddress) -> Option<BeaconRig> {
    let SegmentAddress::V6LinkLocal { addr, scope } = local else {
        return None;
    };
    let bind = SocketAddr::V6(SocketAddrV6::new(addr, 0, 0, scope));
    let socket = match UdpSocket::bind(bind).await {
        Ok(socket) => socket,
        Err(err) => {
            crate::diagnostic_log::warn!("wifi-direct beacon socket on {bind} failed: {err}");
            return None;
        }
    };
    let dest = SocketAddr::V6(SocketAddrV6::new(
        DISCOVERY_GROUP,
        WIFI_DIRECT_BEACON_PORT,
        0,
        scope,
    ));
    Some(BeaconRig {
        socket,
        interval: tokio::time::interval(BEACON_PERIOD),
        local: addr,
        dest,
    })
}

async fn resolve_socket(scope: u32) -> Option<UdpSocket> {
    let bind = SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::UNSPECIFIED,
        WIFI_DIRECT_BEACON_PORT,
        0,
        0,
    ));
    let socket = match UdpSocket::bind(bind).await {
        Ok(socket) => socket,
        Err(err) => {
            crate::diagnostic_log::warn!("wifi-direct beacon listener on {bind} failed: {err}");
            return None;
        }
    };
    if let Err(err) = socket.join_multicast_v6(&DISCOVERY_GROUP, scope) {
        crate::diagnostic_log::warn!(
            "wifi-direct beacon group join on scope {scope} failed: {err}"
        );
    }
    Some(socket)
}

fn segment_socket(address: SegmentAddress, port: u16) -> SocketAddr {
    match address {
        SegmentAddress::V4(ip) => SocketAddr::V4(SocketAddrV4::new(ip, port)),
        SegmentAddress::V6LinkLocal { addr, scope } => {
            SocketAddr::V6(SocketAddrV6::new(addr, port, 0, scope))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::wifi_direct::{Initiative, PeerEvidence};
    use prns_core::interfaces::MacAddress;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn the_aggregate_status_lingers_degraded_after_the_last_member_drops() {
        let status = WifiDirectStatus::new(InterfaceId::new([0xD1; 8]));
        status.mark_up();

        status.set_members(std::vec![TokioInterfaceStatus::new_unaccounted(
            InterfaceId::new([0xD2; 8]),
            ConnectionState::Connected,
        )]);
        assert_eq!(status.connection(), ConnectionState::Connected);

        status.set_members(Vec::new());
        assert_eq!(status.connection(), ConnectionState::Degraded);

        tokio::time::sleep(RECENT_MEMBER_GRACE + Duration::from_millis(10)).await;
        assert_eq!(status.connection(), ConnectionState::Disconnected);
    }

    #[test]
    fn toggle_sleep_and_wake_change_the_aggregate_state() {
        let status = WifiDirectStatus::new(InterfaceId::new([0xD1; 8]));
        status.mark_up();
        assert!(status.is_enabled());

        status.toggle_enabled();
        assert!(!status.is_enabled());
        assert_eq!(status.connection(), ConnectionState::Disabled);

        status.enable();
        assert!(status.is_enabled());
        assert_eq!(status.connection(), ConnectionState::Disconnected);

        status.disable();
        assert!(!status.is_enabled());
        assert_eq!(status.connection(), ConnectionState::Disabled);
    }

    #[test]
    fn revocation_reads_as_disconnected_with_the_reason_never_failed() {
        let status = WifiDirectStatus::new(InterfaceId::new([0xD1; 8]));
        status.mark_up();

        status.set_unavailable(Some("Wi-Fi P2P disabled by the platform"));
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert_eq!(
            status.failure_reason(),
            Some("Wi-Fi P2P disabled by the platform")
        );

        status.set_unavailable(None);
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert_eq!(status.failure_reason(), None);
    }

    #[test]
    fn a_blocked_backend_reads_as_failed_with_its_reason() {
        let status = WifiDirectStatus::new(InterfaceId::new([0xD1; 8]));
        status.mark_failed(Some("no P2P on this platform"));
        assert_eq!(status.connection(), ConnectionState::Failed);
        assert_eq!(status.failure_reason(), Some("no P2P on this platform"));
    }

    #[tokio::test]
    async fn a_beacon_resolves_the_owner_address_past_junk() {
        let socket = UdpSocket::bind("[::1]:0")
            .await
            .expect("binds the listener");
        let addr = socket.local_addr().expect("the bound address is known");
        let sender = UdpSocket::bind("[::1]:0").await.expect("binds the sender");

        let owner: Ipv6Addr = "::1".parse().expect("parses");
        let local: Ipv6Addr = "fe80::1".parse().expect("parses");
        sender.send_to(b"junk", addr).await.expect("sends junk");
        sender
            .send_to(peering_token(&owner).as_bytes(), addr)
            .await
            .expect("sends the token");

        let resolved =
            tokio::time::timeout(Duration::from_secs(2), await_owner_beacon(&socket, &local))
                .await
                .expect("resolves within the window");
        assert_eq!(resolved, owner);
    }

    enum Wire {
        Invite { from: MacAddress },
        Accepted,
    }

    struct LoopbackGroup {
        role: GroupRole,
        plan: DataPlanePlan,
    }

    impl WifiDirectGroup for LoopbackGroup {
        fn role(&self) -> GroupRole {
            self.role
        }

        fn data_plane(&self) -> DataPlanePlan {
            self.plan
        }
    }

    struct LoopbackWifiDirectBackend {
        local: MacAddress,
        peer: MacAddress,
        sighting_pending: bool,
        queued: Option<WifiDirectEvent<LoopbackGroup>>,
        to_peer: mpsc::Sender<Wire>,
        from_peer: mpsc::Receiver<Wire>,
    }

    impl LoopbackWifiDirectBackend {
        fn pair() -> (Self, Self) {
            let addr_a = MacAddress::new([0xAA; 6]);
            let addr_b = MacAddress::new([0xBB; 6]);
            let (a_tx, a_rx) = mpsc::channel(8);
            let (b_tx, b_rx) = mpsc::channel(8);
            let a = Self {
                local: addr_a,
                peer: addr_b,
                sighting_pending: true,
                queued: None,
                to_peer: b_tx,
                from_peer: a_rx,
            };
            let b = Self {
                local: addr_b,
                peer: addr_a,
                sighting_pending: false,
                queued: None,
                to_peer: a_tx,
                from_peer: b_rx,
            };
            (a, b)
        }
    }

    impl WifiDirectBackend for LoopbackWifiDirectBackend {
        type Error = std::convert::Infallible;
        type Group = LoopbackGroup;

        async fn set_discovery(&mut self, _mode: DiscoveryMode) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn form_group(&mut self, _peer: MacAddress, _intent: GoIntent) {
            let _ = self.to_peer.send(Wire::Invite { from: self.local }).await;
        }

        async fn accept_invitation(&mut self, _peer: MacAddress, _intent: GoIntent) {
            let _ = self.to_peer.send(Wire::Accepted).await;
            self.queued = Some(WifiDirectEvent::GroupFormed {
                group: LoopbackGroup {
                    role: GroupRole::Owner,
                    plan: DataPlanePlan::HostRendezvous {
                        local: SegmentAddress::V4(Ipv4Addr::LOCALHOST),
                    },
                },
            });
        }

        async fn remove_group(&mut self) {}

        async fn next_event(&mut self) -> WifiDirectEvent<LoopbackGroup> {
            if self.sighting_pending {
                self.sighting_pending = false;
                return WifiDirectEvent::Sighting {
                    peer: self.peer,
                    evidence: PeerEvidence::ServiceRecord,
                    initiative: Initiative::Ours,
                };
            }
            if let Some(event) = self.queued.take() {
                return event;
            }
            match self.from_peer.recv().await {
                Some(Wire::Invite { from }) => WifiDirectEvent::Invitation { peer: from },
                Some(Wire::Accepted) => WifiDirectEvent::GroupFormed {
                    group: LoopbackGroup {
                        role: GroupRole::Client,
                        plan: DataPlanePlan::DialOwner {
                            owner: SegmentAddress::V4(Ipv4Addr::LOCALHOST),
                        },
                    },
                },
                None => std::future::pending().await,
            }
        }
    }

    #[tokio::test]
    async fn two_supervisors_form_a_group_and_link_over_loopback() {
        let (backend_a, backend_b) = LoopbackWifiDirectBackend::pair();
        let auto_a = WifiDirectAuto::new(backend_a, GoIntent::BALANCED);
        let auto_b = WifiDirectAuto::new(backend_b, GoIntent::BALANCED);
        let status_a = auto_a.status();
        let status_b = auto_b.status();
        let (fleet_a, _tail_a) = Fleet::detached(InterfaceId::new([0xA0; 8]));
        let (fleet_b, _tail_b) = Fleet::detached(InterfaceId::new([0xB0; 8]));
        tokio::spawn(auto_a.run(fleet_a));
        tokio::spawn(auto_b.run(fleet_b));

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if status_a.connection() == ConnectionState::Connected
                && status_b.connection() == ConnectionState::Connected
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "both cards go Live within the window",
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
