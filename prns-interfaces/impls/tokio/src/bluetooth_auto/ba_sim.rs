//! Simulated Bluetooth Auto HIL — BA-SIM-* failure catalog.
//!
//! Contract: `validation/platforms/bluetooth-auto-sim-failure-catalog.md`.
//! Option C′ (locked): MacOs applies `columba_connection_role` when dial-keys /
//! addresses are comparable; Android (and MacOs dialers) back off after
//! [`EMPTY_GATT_MISS_LIMIT`] empty-GATT dials. Legacy DualRole without a shared
//! key fail-opens Dial on production Mac (see catalog).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use prns_core::interfaces::bluetooth_auto::{
    advertisement_group_tag, columba_connection_role, default_group_tag, discovery_groups_match,
    encode_advertisement, group_tag, is_keeper, l2cap_arrangement, l2cap_plan, needs_redial,
    role_for, AdvertisingMode, AndroidHost, AppleHost, BleAddress, BleBackend, BleEvent,
    BleIdentity, BleLink, BleRoleCapabilities, BleSink, BleSource, CloseReason,
    ColumbaConnectionRole, ConnectionPolicy, Control, DialOutcome, Endpoint, EstablishedPeer,
    EstablishedTransport, Handshake, HandshakeOutcome, HandshakeRole, L2capArrangement, L2capPlan,
    LinkCapabilities, LocalPeer, Origin, PeerProtocol, PolicyAction, PolicyInput, Psm,
    ScanningMode, HANDSHAKE_SLACK,
};

const MAX_PEERS: usize = 8;
const DIAL_TRACK: usize = 16;
const EMPTY_GATT_MISS_LIMIT: u32 = 3;
const BA_SIM_DEADLINE_STEPS: usize = 80;

#[derive(Debug)]
struct Closed;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlatformProfile {
    /// Applies address-sort before dial (option C′ Mac target).
    MacOs,
    /// Pre-fix Mac: forwards every DualRole sighting (field race).
    MacOsUnelected,
    /// Applies address-sort and empty-GATT backoff after N misses.
    Android,
    /// Accept-only; never initiates.
    PeripheralOnly,
}

/// Mirrors `prns_ffi::bluetooth_auto::macos::backend::DialAdmission`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DialAdmission {
    AttachCentralSession,
    YieldToSystemConnection,
    YieldToInboundSession,
    CancelStaleSystemConnection,
}

const fn dial_admission(
    already_system_connected: bool,
    target_has_inbound_session: bool,
    has_central_session: bool,
) -> DialAdmission {
    if target_has_inbound_session {
        DialAdmission::YieldToInboundSession
    } else if already_system_connected && has_central_session {
        DialAdmission::YieldToSystemConnection
    } else if already_system_connected {
        DialAdmission::CancelStaleSystemConnection
    } else {
        DialAdmission::AttachCentralSession
    }
}

/// Mirrors `prns_ffi::bluetooth_auto::macos::backend::ScanLease`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScanLease {
    Inactive,
    Renewed,
    Expired,
}

const fn scan_lease(enabled: bool, activity_observed: bool) -> ScanLease {
    match (enabled, activity_observed) {
        (false, _) => ScanLease::Inactive,
        (true, true) => ScanLease::Renewed,
        (true, false) => ScanLease::Expired,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DialFault {
    None,
    EmptyGatt,
}

struct AirNode {
    address: BleAddress,
    profile: PlatformProfile,
    advertising: bool,
    scanning: bool,
    empty_gatt_misses: u32,
    dial_attempts: u32,
    dials_after_first_miss: u32,
    saw_first_miss: bool,
    dial_suppressed: bool,
    settled: bool,
}

struct Air {
    nodes: HashMap<BleAddress, AirNode>,
    events: HashMap<BleAddress, VecDeque<BleEvent<SimLink>>>,
    dial_fault: DialFault,
    force_dual_dial: bool,
    /// When true, `upgrade` fails for Open/Accept plans (GATT floor retained).
    l2cap_upgrade_fails: bool,
    /// Scanners that must not receive sightings (inbound-only accept path).
    block_sightings: HashMap<BleAddress, bool>,
}

impl Air {
    fn push_event(&mut self, to: BleAddress, event: BleEvent<SimLink>) {
        self.events.entry(to).or_default().push_back(event);
    }
}

#[derive(Clone)]
struct SimBleBackend {
    address: BleAddress,
    air: Arc<Mutex<Air>>,
}

struct SimLink {
    address: BleAddress,
    control_tx: tokio::sync::mpsc::Sender<Control>,
    control_rx: tokio::sync::mpsc::Receiver<Control>,
    data_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    data_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    upgrade_fails: bool,
    peer_protocol: PeerProtocol,
}

struct SimSource {
    data_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
}

struct SimSink {
    data_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
}

fn link_pair(
    peer_for_a: BleAddress,
    peer_for_b: BleAddress,
    upgrade_fails: bool,
) -> (SimLink, SimLink) {
    let (ctl_a_tx, ctl_a_rx) = tokio::sync::mpsc::channel(8);
    let (ctl_b_tx, ctl_b_rx) = tokio::sync::mpsc::channel(8);
    let (dat_a_tx, dat_a_rx) = tokio::sync::mpsc::channel(8);
    let (dat_b_tx, dat_b_rx) = tokio::sync::mpsc::channel(8);
    (
        SimLink {
            address: peer_for_a,
            control_tx: ctl_a_tx,
            control_rx: ctl_b_rx,
            data_tx: dat_a_tx,
            data_rx: dat_b_rx,
            upgrade_fails,
            peer_protocol: PeerProtocol::Native,
        },
        SimLink {
            address: peer_for_b,
            control_tx: ctl_b_tx,
            control_rx: ctl_a_rx,
            data_tx: dat_b_tx,
            data_rx: dat_a_rx,
            upgrade_fails,
            peer_protocol: PeerProtocol::Native,
        },
    )
}

impl BleLink for SimLink {
    type Error = Closed;
    type Source = SimSource;
    type Sink = SimSink;

    fn peer_protocol(&self) -> PeerProtocol {
        self.peer_protocol
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

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), Closed> {
        if self.upgrade_fails && !matches!(plan, L2capPlan::None) {
            return Err(Closed);
        }
        Ok(())
    }

    fn into_data(self) -> (SimSource, SimSink) {
        (
            SimSource {
                data_rx: self.data_rx,
            },
            SimSink {
                data_tx: self.data_tx,
            },
        )
    }
}

impl BleSource for SimSource {
    type Error = Closed;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Closed> {
        let frame = self.data_rx.recv().await.ok_or(Closed)?;
        let len = frame.len().min(out.len());
        out[..len].copy_from_slice(&frame[..len]);
        Ok(len)
    }
}

impl BleSink for SimSink {
    type Error = Closed;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Closed> {
        self.data_tx.send(frame.to_vec()).await.map_err(|_| Closed)
    }
}

impl BleBackend<MAX_PEERS> for SimBleBackend {
    type Error = Closed;
    type Link = SimLink;

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), Closed> {
        let mut air = self.air.lock().map_err(|_| Closed)?;
        if let Some(node) = air.nodes.get_mut(&self.address) {
            node.advertising = mode.is_on();
        }
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), Closed> {
        let mut air = self.air.lock().map_err(|_| Closed)?;
        if let Some(node) = air.nodes.get_mut(&self.address) {
            node.scanning = mode.is_on();
        }
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<SimLink> {
        loop {
            {
                let mut air = match self.air.lock() {
                    Ok(air) => air,
                    Err(_) => core::future::pending().await,
                };
                if let Some(queue) = air.events.get_mut(&self.address) {
                    if let Some(event) = queue.pop_front() {
                        return event;
                    }
                }
            }
            tokio::task::yield_now().await;
        }
    }

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        let mut air = match self.air.lock() {
            Ok(air) => air,
            Err(_) => return DialOutcome::Busy,
        };
        let suppressed = air
            .nodes
            .get(&self.address)
            .is_some_and(|node| node.dial_suppressed);
        if suppressed {
            return DialOutcome::Busy;
        }
        let fault = air.dial_fault;
        if let Some(node) = air.nodes.get_mut(&self.address) {
            node.dial_attempts = node.dial_attempts.saturating_add(1);
            if node.saw_first_miss {
                node.dials_after_first_miss = node.dials_after_first_miss.saturating_add(1);
            }
        }
        match fault {
            DialFault::EmptyGatt => {
                if let Some(node) = air.nodes.get_mut(&self.address) {
                    node.empty_gatt_misses = node.empty_gatt_misses.saturating_add(1);
                    node.saw_first_miss = true;
                    if matches!(
                        node.profile,
                        PlatformProfile::Android
                            | PlatformProfile::MacOs
                            | PlatformProfile::MacOsUnelected
                    ) && node.empty_gatt_misses >= EMPTY_GATT_MISS_LIMIT
                    {
                        node.dial_suppressed = true;
                    }
                }
                let dialer = self.address;
                air.push_event(dialer, BleEvent::DialFailed { address });
                DialOutcome::Started
            }
            DialFault::None => {
                let upgrade_fails = air.l2cap_upgrade_fails;
                let (mine, theirs) = link_pair(address, self.address, upgrade_fails);
                air.push_event(
                    self.address,
                    BleEvent::LinkReady {
                        link: mine,
                        origin: Origin::Dialed,
                        peer_rssi: Some(-50),
                    },
                );
                air.push_event(address, BleEvent::Inbound(theirs));
                DialOutcome::Started
            }
        }
    }
}

struct PairWorld {
    air: Arc<Mutex<Air>>,
    backend_a: SimBleBackend,
    backend_b: SimBleBackend,
    addr_a: BleAddress,
    addr_b: BleAddress,
    local_a: LocalPeer,
    local_b: LocalPeer,
}

fn caps(psm: u16) -> LinkCapabilities {
    LinkCapabilities {
        l2cap: Psm::new(psm),
        link_mtu: 500,
    }
}

fn mac_endpoint() -> Endpoint {
    Endpoint::CoreBluetooth(AppleHost::MacOs)
}

fn android_endpoint() -> Endpoint {
    Endpoint::Android(AndroidHost::Android)
}

fn pair_world(
    addr_a: [u8; 6],
    addr_b: [u8; 6],
    profile_a: PlatformProfile,
    profile_b: PlatformProfile,
    group_a: [u8; 4],
    group_b: [u8; 4],
    dial_fault: DialFault,
) -> PairWorld {
    let addr_a = BleAddress::new(addr_a);
    let addr_b = BleAddress::new(addr_b);
    let local_a = LocalPeer {
        identity: BleIdentity::new([0xA1; 16]),
        endpoint: match profile_a {
            PlatformProfile::MacOs | PlatformProfile::MacOsUnelected => mac_endpoint(),
            PlatformProfile::Android | PlatformProfile::PeripheralOnly => android_endpoint(),
        },
        capabilities: caps(0x00c0),
        group_tag: group_a,
    };
    let local_b = LocalPeer {
        identity: BleIdentity::new([0xB2; 16]),
        endpoint: match profile_b {
            PlatformProfile::MacOs | PlatformProfile::MacOsUnelected => mac_endpoint(),
            PlatformProfile::Android | PlatformProfile::PeripheralOnly => android_endpoint(),
        },
        capabilities: caps(0x008b),
        group_tag: group_b,
    };
    let mut nodes = HashMap::new();
    nodes.insert(
        addr_a,
        AirNode {
            address: addr_a,
            profile: profile_a,
            advertising: false,
            scanning: false,
            empty_gatt_misses: 0,
            dial_attempts: 0,
            dials_after_first_miss: 0,
            saw_first_miss: false,
            dial_suppressed: false,
            settled: false,
        },
    );
    nodes.insert(
        addr_b,
        AirNode {
            address: addr_b,
            profile: profile_b,
            advertising: false,
            scanning: false,
            empty_gatt_misses: 0,
            dial_attempts: 0,
            dials_after_first_miss: 0,
            saw_first_miss: false,
            dial_suppressed: false,
            settled: false,
        },
    );
    let air = Arc::new(Mutex::new(Air {
        nodes,
        events: HashMap::new(),
        dial_fault,
        force_dual_dial: false,
        l2cap_upgrade_fails: false,
        block_sightings: HashMap::new(),
    }));
    PairWorld {
        backend_a: SimBleBackend {
            address: addr_a,
            air: Arc::clone(&air),
        },
        backend_b: SimBleBackend {
            address: addr_b,
            air: Arc::clone(&air),
        },
        air,
        addr_a,
        addr_b,
        local_a,
        local_b,
    }
}

fn role_caps(profile: PlatformProfile) -> BleRoleCapabilities {
    match profile {
        PlatformProfile::PeripheralOnly => BleRoleCapabilities::PeripheralOnly,
        PlatformProfile::MacOs | PlatformProfile::MacOsUnelected | PlatformProfile::Android => {
            BleRoleCapabilities::DualRole
        }
    }
}

fn should_dial(
    local_profile: PlatformProfile,
    local: BleAddress,
    peer_profile: PlatformProfile,
    peer: BleAddress,
) -> bool {
    match local_profile {
        PlatformProfile::MacOsUnelected => true,
        PlatformProfile::PeripheralOnly => false,
        PlatformProfile::MacOs | PlatformProfile::Android => matches!(
            columba_connection_role(
                local,
                role_caps(local_profile),
                peer,
                role_caps(peer_profile),
            ),
            ColumbaConnectionRole::Dial
        ),
    }
}

fn deliver_sightings(air: &mut Air) {
    let advertisers: Vec<BleAddress> = air
        .nodes
        .values()
        .filter(|n| n.advertising)
        .map(|n| n.address)
        .collect();
    let scanners: Vec<BleAddress> = air
        .nodes
        .values()
        .filter(|n| n.scanning)
        .map(|n| n.address)
        .collect();
    for scanner in scanners {
        if air.block_sightings.get(&scanner).copied().unwrap_or(false) {
            continue;
        }
        for advertiser in &advertisers {
            if scanner == *advertiser {
                continue;
            }
            air.push_event(
                scanner,
                BleEvent::Sighting {
                    address: *advertiser,
                    rssi: Some(-55),
                },
            );
        }
    }
}

async fn drive_native_handshake(
    link: &mut SimLink,
    role: HandshakeRole,
    local: LocalPeer,
) -> Result<EstablishedPeer, CloseReason> {
    let (mut handshake, opening) = Handshake::begin(role, local, Some(-55));
    if let Some(msg) = opening {
        link.control_send(&msg)
            .await
            .map_err(|_| CloseReason::Incompatible)?;
    }
    loop {
        let msg = link
            .control_recv()
            .await
            .map_err(|_| CloseReason::Incompatible)?;
        let reaction = handshake.absorb(msg);
        if let Some(reply) = reaction.reply {
            link.control_send(&reply)
                .await
                .map_err(|_| CloseReason::Incompatible)?;
        }
        match reaction.outcome {
            HandshakeOutcome::Settled(established) => return Ok(established),
            HandshakeOutcome::Aborted(reason) => return Err(reason),
            HandshakeOutcome::Pending => {}
        }
    }
}

struct StepOutcome {
    settled_a: bool,
    settled_b: bool,
    dials_a: u32,
    dials_b: u32,
    dials_after_miss_b: u32,
    suppressed_b: bool,
}

async fn run_pair_steps(world: &mut PairWorld, steps: usize) -> StepOutcome {
    let _ = world.backend_a.set_advertising(AdvertisingMode::On).await;
    let _ = world.backend_b.set_advertising(AdvertisingMode::On).await;
    let _ = world.backend_a.set_scanning(ScanningMode::On).await;
    let _ = world.backend_b.set_scanning(ScanningMode::On).await;

    {
        let mut air = world.air.lock().expect("air lock");
        deliver_sightings(&mut air);
    }

    let mut policy_a = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(world.local_a);
    let mut policy_b = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(world.local_b);
    let mut pending_a = Vec::new();
    let mut pending_b = Vec::new();
    policy_a.start(&mut |action| pending_a.push(action));
    policy_b.start(&mut |action| pending_b.push(action));

    let force_dual = world
        .air
        .lock()
        .map(|air| air.force_dual_dial)
        .unwrap_or(false);

    let mut open_a: Option<(SimLink, Origin)> = None;
    let mut open_b: Option<(SimLink, Origin)> = None;

    for step in 0..steps {
        let now_ms = (step as u64).saturating_mul(250);
        if step % 6 == 0 {
            if let Ok(mut air) = world.air.lock() {
                deliver_sightings(&mut air);
            }
        }
        apply_pending(
            &mut world.backend_a,
            world.addr_a,
            &mut pending_a,
            force_dual,
        )
        .await;
        apply_pending(
            &mut world.backend_b,
            world.addr_b,
            &mut pending_b,
            force_dual,
        )
        .await;

        collect_backend_events(
            &mut world.backend_a,
            world.addr_a,
            &mut policy_a,
            &mut pending_a,
            now_ms,
            &mut open_a,
        )
        .await;
        collect_backend_events(
            &mut world.backend_b,
            world.addr_b,
            &mut policy_b,
            &mut pending_b,
            now_ms,
            &mut open_b,
        )
        .await;

        if let (Some((mut link_a, origin_a)), Some((mut link_b, origin_b))) =
            (open_a.take(), open_b.take())
        {
            if !policy_a.begin_handshake(origin_a) || !policy_b.begin_handshake(origin_b) {
                continue;
            }
            let local_a = world.local_a;
            let local_b = world.local_b;
            let (result_a, result_b) = tokio::join!(
                drive_native_handshake(&mut link_a, role_for(origin_a), local_a),
                drive_native_handshake(&mut link_b, role_for(origin_b), local_b),
            );
            if let Ok(ref established) = result_a {
                let _ =
                    try_l2cap_upgrade(&mut link_a, role_for(origin_a), local_a, established).await;
            }
            if let Ok(ref established) = result_b {
                let _ =
                    try_l2cap_upgrade(&mut link_b, role_for(origin_b), local_b, established).await;
            }
            finish_handshake(
                &world.air,
                world.addr_a,
                &mut policy_a,
                &mut pending_a,
                link_a.address(),
                origin_a,
                result_a,
                now_ms,
            );
            finish_handshake(
                &world.air,
                world.addr_b,
                &mut policy_b,
                &mut pending_b,
                link_b.address(),
                origin_b,
                result_b,
                now_ms,
            );
        }

        apply_pending(
            &mut world.backend_a,
            world.addr_a,
            &mut pending_a,
            force_dual,
        )
        .await;
        apply_pending(
            &mut world.backend_b,
            world.addr_b,
            &mut pending_b,
            force_dual,
        )
        .await;
    }

    let air = world.air.lock().expect("air lock");
    let a = air.nodes.get(&world.addr_a).expect("node a");
    let b = air.nodes.get(&world.addr_b).expect("node b");
    StepOutcome {
        settled_a: a.settled,
        settled_b: b.settled,
        dials_a: a.dial_attempts,
        dials_b: b.dial_attempts,
        dials_after_miss_b: b.dials_after_first_miss,
        suppressed_b: b.dial_suppressed,
    }
}

async fn try_l2cap_upgrade(
    link: &mut SimLink,
    role: HandshakeRole,
    local: LocalPeer,
    established: &EstablishedPeer,
) -> Result<(), Closed> {
    let EstablishedTransport::Native {
        endpoint,
        capabilities,
    } = established.transport
    else {
        return Ok(());
    };
    let arrangement = l2cap_arrangement(local.endpoint, endpoint);
    let plan = l2cap_plan(
        arrangement,
        role,
        local.endpoint,
        &local.capabilities,
        &capabilities,
    );
    link.upgrade(&plan).await
}

fn finish_handshake(
    air: &Arc<Mutex<Air>>,
    local_addr: BleAddress,
    policy: &mut ConnectionPolicy<MAX_PEERS, DIAL_TRACK>,
    pending: &mut Vec<PolicyAction>,
    address: BleAddress,
    origin: Origin,
    result: Result<EstablishedPeer, CloseReason>,
    now_ms: u64,
) {
    match result {
        Ok(established) => {
            let mut rejected = false;
            policy.handle(
                PolicyInput::Settled {
                    address,
                    origin,
                    established,
                    now_ms,
                },
                &mut |action| {
                    if matches!(action, PolicyAction::Reject { .. }) {
                        rejected = true;
                    }
                    pending.push(action);
                },
            );
            if !rejected {
                if let Ok(mut air) = air.lock() {
                    if let Some(node) = air.nodes.get_mut(&local_addr) {
                        node.settled = true;
                    }
                }
            }
        }
        Err(_) => {
            policy.handle(
                PolicyInput::HandshakeFailed { address, origin },
                &mut |action| pending.push(action),
            );
        }
    }
}

async fn collect_backend_events(
    backend: &mut SimBleBackend,
    local_addr: BleAddress,
    policy: &mut ConnectionPolicy<MAX_PEERS, DIAL_TRACK>,
    pending: &mut Vec<PolicyAction>,
    now_ms: u64,
    open: &mut Option<(SimLink, Origin)>,
) {
    for _ in 0..8 {
        let has_event = backend
            .air
            .lock()
            .ok()
            .and_then(|air| air.events.get(&local_addr).map(|q| !q.is_empty()))
            .unwrap_or(false);
        if !has_event {
            break;
        }
        match backend.next_event().await {
            BleEvent::Sighting { address, .. } => {
                let (local_profile, peer_profile) = {
                    let air = backend.air.lock().expect("air");
                    let local_profile = air.nodes.get(&local_addr).map(|n| n.profile);
                    let peer_profile = air.nodes.get(&address).map(|n| n.profile);
                    (local_profile, peer_profile)
                };
                // Election happens at the radio edge before policy sees the sighting —
                // matching Android shouldDial / option C′ Mac pre-filter. Forced dual-dial
                // scenarios still forward every sighting.
                let forward = {
                    let air = backend.air.lock().expect("air");
                    let suppressed = air
                        .nodes
                        .get(&local_addr)
                        .is_some_and(|n| n.dial_suppressed);
                    !suppressed
                        && (air.force_dual_dial
                            || local_profile.zip(peer_profile).is_some_and(
                                |(local_profile, peer_profile)| {
                                    should_dial(local_profile, local_addr, peer_profile, address)
                                },
                            ))
                };
                if forward {
                    policy.handle(PolicyInput::Sighting { address, now_ms }, &mut |action| {
                        pending.push(action)
                    });
                }
            }
            BleEvent::DialFailed { address } => {
                policy.handle(PolicyInput::DialFailed { address, now_ms }, &mut |action| {
                    pending.push(action)
                });
            }
            BleEvent::LinkReady {
                link,
                origin,
                peer_rssi: _,
            } => {
                *open = Some((link, origin));
            }
            BleEvent::Inbound(link) => {
                *open = Some((link, Origin::Accepted));
            }
        }
    }
}

async fn apply_pending(
    backend: &mut SimBleBackend,
    _local_addr: BleAddress,
    pending: &mut Vec<PolicyAction>,
    _force_dual: bool,
) {
    let actions = std::mem::take(pending);
    for action in actions {
        match action {
            PolicyAction::Dial(address) => {
                let _ = backend.dial(address).await;
            }
            PolicyAction::SetAdvertising(mode) => {
                let _ = backend.set_advertising(mode).await;
            }
            PolicyAction::SetScanning(mode) => {
                let _ = backend.set_scanning(mode).await;
            }
            PolicyAction::Reject { .. }
            | PolicyAction::Admit { .. }
            | PolicyAction::Evict { .. }
            | PolicyAction::NotifyClosed(_) => {}
        }
    }
}

/// Phone MAC lower than Mac → Android elects Dial under DualRole sort.
fn phone_wins_sort_addrs() -> ([u8; 6], [u8; 6]) {
    ([0x10, 0, 0, 0, 0, 1], [0xF0, 0, 0, 0, 0, 1])
}

/// Mac MAC lower than phone → Mac elects Dial.
fn mac_wins_sort_addrs() -> ([u8; 6], [u8; 6]) {
    ([0x10, 0, 0, 0, 0, 1], [0xF0, 0, 0, 0, 0, 1])
}

#[tokio::test]
async fn ba_sim_01_happy_mac_android_phone_wins_sort() {
    let (phone, mac) = phone_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        out.settled_a && out.settled_b,
        "BA-SIM-01 expected settle; dials mac={} phone={}",
        out.dials_a,
        out.dials_b
    );
    assert_eq!(out.dials_a, 0, "Mac must Accept when phone wins sort");
    assert!(out.dials_b >= 1, "Android must Dial when it wins sort");
    eprintln!(
        "BA-SIM-01 PASS profiles=MacOs,Android dials_mac={} dials_phone={} settled=true",
        out.dials_a, out.dials_b
    );
}

#[tokio::test]
async fn ba_sim_02_unelected_macos_exhibits_field_dual_dial_race() {
    // Characterization of pre-fix Mac CB: no pre-dial election.
    // Phone wins sort and dials; unelected Mac also dials into empty GATT.
    let (phone, mac) = phone_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOsUnelected,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::EmptyGatt,
    );
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        out.dials_a >= 1,
        "unelected Mac must exhibit the field race by also dialing (dials_mac={})",
        out.dials_a
    );
    eprintln!(
        "BA-SIM-02 UNELECTED race dials_mac={} dials_phone={} (fixed MacOs profile must dial 0)",
        out.dials_a, out.dials_b
    );
}

#[tokio::test]
async fn ba_sim_02_empty_gatt_option_c_backoff() {
    let (phone, mac) = phone_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::EmptyGatt,
    );
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        !out.settled_a && !out.settled_b,
        "empty GATT must not settle"
    );
    assert_eq!(
        out.dials_a, 0,
        "Mac election: Accept-only when phone wins sort"
    );
    assert!(
        out.suppressed_b,
        "Android must suppress dial after {EMPTY_GATT_MISS_LIMIT} empty-GATT misses (dials_phone={})",
        out.dials_b
    );
    assert!(
        out.dials_after_miss_b < 10,
        "BA-SIM-02 fail: unbounded redial after miss ({})",
        out.dials_after_miss_b
    );
    assert!(
        out.dials_b <= EMPTY_GATT_MISS_LIMIT + 2,
        "dial count {} exceeds option C′ bound",
        out.dials_b
    );
    eprintln!(
        "BA-SIM-02 PASS profiles=MacOs,Android dials_after_miss={} settled=false idle=true",
        out.dials_after_miss_b
    );
}

#[tokio::test]
async fn ba_sim_03_mac_wins_address_sort() {
    let (mac, phone) = mac_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(out.settled_a && out.settled_b, "BA-SIM-03 expected settle");
    assert!(out.dials_a >= 1, "Mac must Dial when it wins sort");
    // Opens(Android) may needs_redial so Android dials once as the designated CoC opener.
    assert!(
        out.dials_b <= 2,
        "Android dial count {} should stay small after Mac-won election",
        out.dials_b
    );
    eprintln!(
        "BA-SIM-03 PASS dials_mac={} dials_phone={} settled=true",
        out.dials_a, out.dials_b
    );
}

#[tokio::test]
async fn ba_sim_04_simultaneous_dual_dial_keeper() {
    let (mac, phone) = mac_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    {
        let mut air = world.air.lock().expect("air");
        air.force_dual_dial = true;
    }
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        out.settled_a || out.settled_b,
        "BA-SIM-04 expected at least one side to keep a settled peer"
    );
    assert!(out.dials_a >= 1 && out.dials_b >= 1, "both sides dialed");
    eprintln!(
        "BA-SIM-04 PASS dual_dial dials_mac={} dials_phone={} settled_mac={} settled_phone={}",
        out.dials_a, out.dials_b, out.settled_a, out.settled_b
    );
}

#[test]
fn ba_sim_05_needs_redial_opens_android() {
    let mac = mac_endpoint();
    let android = android_endpoint();
    let arrangement = l2cap_arrangement(mac, android);
    assert!(matches!(
        arrangement,
        L2capArrangement::Opens(endpoint) if endpoint == android
    ));
    assert!(
        needs_redial(arrangement, HandshakeRole::Dialer, mac),
        "Mac as Dialer must needs_redial for Opens(Android)"
    );
    assert!(
        needs_redial(arrangement, HandshakeRole::Listener, android),
        "Android as Listener must needs_redial for Opens(Android)"
    );
    assert!(!needs_redial(arrangement, HandshakeRole::Listener, mac));
    assert!(!needs_redial(arrangement, HandshakeRole::Dialer, android));
    let plan = l2cap_plan(
        arrangement,
        HandshakeRole::Dialer,
        android,
        &caps(0x008b),
        &caps(0x00c0),
    );
    assert!(matches!(plan, L2capPlan::Open { .. }));
    let mac_plan = l2cap_plan(
        arrangement,
        HandshakeRole::Listener,
        mac,
        &caps(0x00c0),
        &caps(0x008b),
    );
    assert_eq!(mac_plan, L2capPlan::Accept);
    assert!(!is_keeper(
        arrangement,
        HandshakeRole::Dialer,
        BleIdentity::new([1; 16]),
        mac,
        BleIdentity::new([2; 16]),
    ));
    eprintln!("BA-SIM-05 PASS needs_redial Opens(Android)");
}

#[tokio::test]
async fn ba_sim_08_group_tag_mismatch() {
    let (mac, phone) = mac_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        group_tag(b"other"),
        DialFault::None,
    );
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        !out.settled_a && !out.settled_b,
        "BA-SIM-08 must not settle across groups"
    );
    eprintln!("BA-SIM-08 PASS group_mismatch settled=false");
}

#[tokio::test]
async fn ba_sim_20_reconnect_after_clean_close() {
    let (mac, phone) = mac_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    let first = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(first.settled_a && first.settled_b, "first settle required");

    {
        let mut air = world.air.lock().expect("air");
        for node in air.nodes.values_mut() {
            node.settled = false;
            node.dial_attempts = 0;
            node.dials_after_first_miss = 0;
            node.saw_first_miss = false;
            node.dial_suppressed = false;
            node.empty_gatt_misses = 0;
        }
        air.events.clear();
    }

    let second = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        second.settled_a && second.settled_b,
        "BA-SIM-20 second settle required"
    );
    eprintln!("BA-SIM-20 PASS reconnect settled=true");
}

fn policy_local(identity: u8, endpoint: Endpoint) -> LocalPeer {
    LocalPeer {
        identity: BleIdentity::new([identity; 16]),
        endpoint,
        capabilities: caps(0x00c0),
        group_tag: default_group_tag(),
    }
}

fn established_native(identity: u8, endpoint: Endpoint, psm: u16) -> EstablishedPeer {
    EstablishedPeer {
        identity: BleIdentity::new([identity; 16]),
        transport: EstablishedTransport::Native {
            endpoint,
            capabilities: caps(psm),
        },
        peer_rssi: Some(-50),
    }
}

fn collect_actions<const M: usize, const D: usize>(
    policy: &mut ConnectionPolicy<M, D>,
    input: PolicyInput,
) -> Vec<PolicyAction> {
    let mut actions = Vec::new();
    policy.handle(input, &mut |action| actions.push(action));
    actions
}

#[test]
fn ba_sim_06_zombie_system_connected_cancels_stale() {
    assert_eq!(
        dial_admission(true, false, false),
        DialAdmission::CancelStaleSystemConnection
    );
    assert_eq!(
        dial_admission(true, false, true),
        DialAdmission::YieldToSystemConnection
    );
    assert_ne!(
        dial_admission(true, false, false),
        DialAdmission::YieldToSystemConnection,
        "zombie ACL without Prns session must cancel, not yield forever"
    );
    eprintln!("BA-SIM-06 PASS CancelStaleSystemConnection");
}

#[test]
fn ba_sim_07_yield_to_live_inbound_session() {
    assert_eq!(
        dial_admission(false, true, false),
        DialAdmission::YieldToInboundSession
    );
    assert_eq!(
        dial_admission(true, true, true),
        DialAdmission::YieldToInboundSession,
        "inbound session wins over system-connected and central"
    );
    eprintln!("BA-SIM-07 PASS YieldToInboundSession");
}

#[test]
fn ba_sim_09_truncated_then_full_adv_still_dialable() {
    let local = default_group_tag();
    // Truncated: service UUID only, no manufacturer → treated as default group (not blacklisted).
    let mut truncated = [0u8; 31];
    let trunc_len = {
        // flags + service UUID128 only (no manufacturer)
        truncated[0] = 2;
        truncated[1] = 0x01;
        truncated[2] = 0x06;
        let mut uuid = prns_core::interfaces::bluetooth_auto::BLE_SERVICE_UUID_BYTES;
        uuid.reverse();
        truncated[3] = 17;
        truncated[4] = 0x07;
        truncated[5..21].copy_from_slice(&uuid);
        21
    };
    assert!(
        discovery_groups_match(local, &truncated[..trunc_len]),
        "truncated ADV must not permanently exclude default-group peers"
    );
    assert!(
        advertisement_group_tag(&truncated[..trunc_len]) == local,
        "missing manufacturer falls back to default group"
    );

    let mut full = [0u8; 31];
    let full_len =
        encode_advertisement(&mut full, BleRoleCapabilities::DualRole, local).expect("encode");
    assert!(discovery_groups_match(local, &full[..full_len]));

    // Policy still dials after a first "unknown"/truncated-style sighting.
    let mut policy =
        ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(policy_local(1, android_endpoint()));
    policy.start(&mut |_| {});
    let peer = BleAddress::new([0xAA; 6]);
    let first = collect_actions(
        &mut policy,
        PolicyInput::Sighting {
            address: peer,
            now_ms: 0,
        },
    );
    assert_eq!(first, vec![PolicyAction::Dial(peer)]);
    let second = collect_actions(
        &mut policy,
        PolicyInput::Sighting {
            address: peer,
            now_ms: 20_000,
        },
    );
    assert_eq!(
        second,
        vec![PolicyAction::Dial(peer)],
        "full ADV later must still be dialable"
    );
    eprintln!("BA-SIM-09 PASS truncated_then_full dialable=true");
}

#[tokio::test]
async fn ba_sim_10_group_change_drops_settled() {
    let (mac, phone) = mac_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    let first = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(first.settled_a && first.settled_b);

    let mut policy = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(world.local_a);
    policy.start(&mut |_| {});
    let peer_id = world.local_b.identity;
    let peer_addr = world.addr_b;
    let _ = collect_actions(
        &mut policy,
        PolicyInput::Settled {
            address: peer_addr,
            origin: Origin::Accepted,
            established: established_native(0xB2, android_endpoint(), 0x008b),
            now_ms: 1_000,
        },
    );
    assert_eq!(policy.settled_count(), 1);
    let closed = collect_actions(
        &mut policy,
        PolicyInput::Closed {
            identity: peer_id,
            address: peer_addr,
        },
    );
    assert!(
        closed
            .iter()
            .any(|a| matches!(a, PolicyAction::NotifyClosed(_))),
        "group change must close the old peer"
    );
    assert_eq!(policy.settled_count(), 0);

    // New group does not settle with the old peer tag.
    world.local_a.group_tag = group_tag(b"other");
    {
        let mut air = world.air.lock().expect("air");
        for node in air.nodes.values_mut() {
            node.settled = false;
            node.dial_attempts = 0;
            node.dial_suppressed = false;
            node.empty_gatt_misses = 0;
            node.saw_first_miss = false;
            node.dials_after_first_miss = 0;
        }
        air.events.clear();
    }
    let after = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        !after.settled_a && !after.settled_b,
        "mismatched groups after change must not settle"
    );
    eprintln!("BA-SIM-10 PASS group_change dropped=true");
}

#[tokio::test]
async fn ba_sim_11_gatt_write_timeout_then_recover() {
    let (mac, phone) = mac_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    let first = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(first.settled_a && first.settled_b);

    {
        let mut air = world.air.lock().expect("air");
        for node in air.nodes.values_mut() {
            node.settled = false;
            node.dial_attempts = 0;
            node.dial_suppressed = false;
            node.empty_gatt_misses = 0;
            node.saw_first_miss = false;
            node.dials_after_first_miss = 0;
        }
        air.events.clear();
    }
    let mut policy = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(world.local_a);
    policy.start(&mut |_| {});
    let _ = collect_actions(
        &mut policy,
        PolicyInput::Settled {
            address: world.addr_b,
            origin: Origin::Accepted,
            established: established_native(0xB2, android_endpoint(), 0x008b),
            now_ms: 500,
        },
    );
    let _ = collect_actions(
        &mut policy,
        PolicyInput::Closed {
            identity: world.local_b.identity,
            address: world.addr_b,
        },
    );
    assert_eq!(
        policy.settled_count(),
        0,
        "write-timeout tear must clear member"
    );

    let second = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        second.settled_a && second.settled_b,
        "BA-SIM-11 expected rediscovery settle"
    );
    eprintln!("BA-SIM-11 PASS timeout_recover settled=true");
}

#[tokio::test]
async fn ba_sim_12_l2cap_open_fails_gatt_floor_retained() {
    let (mac, phone) = mac_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    {
        let mut air = world.air.lock().expect("air");
        air.l2cap_upgrade_fails = true;
    }
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        out.settled_a && out.settled_b,
        "BA-SIM-12 must retain GATT floor settle when CoC open fails"
    );
    let arrangement = l2cap_arrangement(mac_endpoint(), android_endpoint());
    let plan = l2cap_plan(
        arrangement,
        HandshakeRole::Dialer,
        android_endpoint(),
        &caps(0x008b),
        &caps(0x00c0),
    );
    assert!(matches!(plan, L2capPlan::Open { .. }));
    eprintln!("BA-SIM-12 PASS gatt_floor retained=true upgrade_failed=true");
}

#[tokio::test]
async fn ba_sim_13_l2cap_reader_death_tears_link() {
    let (mac, phone) = mac_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    let first = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(first.settled_a && first.settled_b);

    let mut policy = ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(world.local_a);
    policy.start(&mut |_| {});
    let _ = collect_actions(
        &mut policy,
        PolicyInput::Settled {
            address: world.addr_b,
            origin: Origin::Accepted,
            established: established_native(0xB2, android_endpoint(), 0x008b),
            now_ms: 1_000,
        },
    );
    let torn = collect_actions(
        &mut policy,
        PolicyInput::Closed {
            identity: world.local_b.identity,
            address: world.addr_b,
        },
    );
    assert!(torn
        .iter()
        .any(|a| matches!(a, PolicyAction::NotifyClosed(_))));
    assert_eq!(policy.settled_count(), 0, "reader death must clear settled");

    {
        let mut air = world.air.lock().expect("air");
        for node in air.nodes.values_mut() {
            node.settled = false;
            node.dial_attempts = 0;
            node.dial_suppressed = false;
            node.empty_gatt_misses = 0;
            node.saw_first_miss = false;
            node.dials_after_first_miss = 0;
        }
        air.events.clear();
    }
    let second = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        second.settled_a && second.settled_b,
        "rediscovery after tear"
    );
    eprintln!("BA-SIM-13 PASS reader_death torn=true rediscover=true");
}

#[tokio::test]
async fn ba_sim_14_peripheral_only_peer() {
    let dual = [0x10, 0, 0, 0, 0, 1];
    let peri = [0xF0, 0, 0, 0, 0, 1];
    assert!(should_dial(
        PlatformProfile::Android,
        BleAddress::new(dual),
        PlatformProfile::PeripheralOnly,
        BleAddress::new(peri),
    ));
    assert!(!should_dial(
        PlatformProfile::PeripheralOnly,
        BleAddress::new(peri),
        PlatformProfile::Android,
        BleAddress::new(dual),
    ));
    assert_eq!(
        columba_connection_role(
            BleAddress::new(dual),
            BleRoleCapabilities::PeripheralOnly,
            BleAddress::new(peri),
            BleRoleCapabilities::PeripheralOnly,
        ),
        ColumbaConnectionRole::Unavailable
    );

    let mut world = pair_world(
        dual,
        peri,
        PlatformProfile::Android,
        PlatformProfile::PeripheralOnly,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        out.settled_a && out.settled_b,
        "DualRole must dial PeripheralOnly"
    );
    assert!(out.dials_a >= 1);
    assert_eq!(out.dials_b, 0, "PeripheralOnly never dials");

    let mut both_peri = pair_world(
        dual,
        peri,
        PlatformProfile::PeripheralOnly,
        PlatformProfile::PeripheralOnly,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    let stuck = run_pair_steps(&mut both_peri, 40).await;
    assert!(
        !stuck.settled_a && !stuck.settled_b,
        "two PeripheralOnly must not settle"
    );
    eprintln!("BA-SIM-14 PASS peripheral_only dial=yes both_peri=no");
}

#[test]
fn ba_sim_15_capacity_stops_radio_then_resumes() {
    let mut policy = ConnectionPolicy::<1, 8>::new(policy_local(1, android_endpoint()));
    policy.start(&mut |_| {});
    let peer = BleAddress::new([2; 6]);
    let fill = collect_actions(
        &mut policy,
        PolicyInput::Settled {
            address: peer,
            origin: Origin::Accepted,
            established: established_native(2, android_endpoint(), 0x008b),
            now_ms: 0,
        },
    );
    assert!(fill
        .iter()
        .any(|a| matches!(a, PolicyAction::SetAdvertising(AdvertisingMode::Off))));
    assert!(fill
        .iter()
        .any(|a| matches!(a, PolicyAction::SetScanning(ScanningMode::Off))));
    assert_eq!(policy.settled_count(), 1);

    let reopen = collect_actions(
        &mut policy,
        PolicyInput::Closed {
            identity: BleIdentity::new([2; 16]),
            address: peer,
        },
    );
    assert!(reopen
        .iter()
        .any(|a| matches!(a, PolicyAction::SetAdvertising(AdvertisingMode::On))));
    assert!(reopen
        .iter()
        .any(|a| matches!(a, PolicyAction::SetScanning(ScanningMode::On))));
    assert_eq!(policy.settled_count(), 0);
    eprintln!("BA-SIM-15 PASS capacity off_then_on");
}

#[test]
fn ba_sim_16_handshake_flood_slack_gate() {
    let mut policy = ConnectionPolicy::<2, 8>::new(policy_local(1, android_endpoint()));
    for _ in 0..(2 + HANDSHAKE_SLACK) {
        assert!(policy.begin_handshake(Origin::Accepted));
    }
    assert!(
        !policy.begin_handshake(Origin::Accepted),
        "inbound flood past MAX_PEERS + HANDSHAKE_SLACK must refuse"
    );
    assert!(
        policy.begin_handshake(Origin::Dialed),
        "local dials stay ungated"
    );
    eprintln!("BA-SIM-16 PASS flood_gate refused=true");
}

#[tokio::test]
async fn ba_sim_17_mac_empty_gatt_backoff() {
    // Mac wins sort so MacOs is the dialer under empty GATT.
    let (mac, phone) = mac_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::EmptyGatt,
    );
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(!out.settled_a && !out.settled_b);
    assert!(
        out.dials_a >= EMPTY_GATT_MISS_LIMIT,
        "Mac dialer must attempt empty GATT"
    );
    let suppressed_a = {
        let air = world.air.lock().expect("air");
        air.nodes.get(&world.addr_a).expect("a").dial_suppressed
    };
    assert!(
        suppressed_a,
        "MacOs must suppress after {EMPTY_GATT_MISS_LIMIT} empty-GATT misses"
    );
    assert!(out.dials_a <= EMPTY_GATT_MISS_LIMIT + 2);
    eprintln!(
        "BA-SIM-17 PASS profiles=MacOs dials={} suppressed=true",
        out.dials_a
    );
}

#[test]
fn ba_sim_18_scan_lease_restarts_silent_scan() {
    assert_eq!(scan_lease(false, false), ScanLease::Inactive);
    assert_eq!(scan_lease(true, true), ScanLease::Renewed);
    assert_eq!(
        scan_lease(true, false),
        ScanLease::Expired,
        "enabled scan with no callbacks must expire for restart"
    );
    eprintln!("BA-SIM-18 PASS ScanLease::Expired");
}

#[tokio::test]
async fn ba_sim_19_inbound_hello_without_prior_sighting() {
    // Phone wins sort so Android dials while Mac only accepts inbound.
    let (phone, mac) = phone_wins_sort_addrs();
    let mut world = pair_world(
        mac,
        phone,
        PlatformProfile::MacOs,
        PlatformProfile::Android,
        default_group_tag(),
        default_group_tag(),
        DialFault::None,
    );
    {
        let mut air = world.air.lock().expect("air");
        // Mac never sights; only Android dials into Mac inbound.
        air.block_sightings.insert(world.addr_a, true);
    }
    let out = run_pair_steps(&mut world, BA_SIM_DEADLINE_STEPS).await;
    assert!(
        out.settled_a && out.settled_b,
        "BA-SIM-19 listener must settle from inbound alone"
    );
    assert_eq!(out.dials_a, 0, "Mac must not dial without sightings");
    assert!(out.dials_b >= 1, "Android must dial");
    eprintln!(
        "BA-SIM-19 PASS inbound_only dials_mac=0 dials_phone={} settled=true",
        out.dials_b
    );
}

#[test]
fn ba_sim_21_android_android_stays_gatt_only() {
    let a = android_endpoint();
    let b = android_endpoint();
    let arrangement = l2cap_arrangement(a, b);
    assert_eq!(arrangement, L2capArrangement::GattOnly);
    let plan = l2cap_plan(
        arrangement,
        HandshakeRole::Dialer,
        a,
        &caps(0x008b),
        &caps(0x008b),
    );
    assert_eq!(plan, L2capPlan::None);
    assert!(!needs_redial(arrangement, HandshakeRole::Dialer, a));
    eprintln!("BA-SIM-21 PASS GattOnly plan=None");
}

#[test]
fn ba_sim_22_columba_gatt_compat_path() {
    let mut policy =
        ConnectionPolicy::<MAX_PEERS, DIAL_TRACK>::new(policy_local(1, android_endpoint()));
    policy.start(&mut |_| {});
    let peer = BleAddress::new([9; 6]);
    let established = EstablishedPeer {
        identity: BleIdentity::new([9; 16]),
        transport: EstablishedTransport::ColumbaGatt,
        peer_rssi: Some(-60),
    };
    let actions = collect_actions(
        &mut policy,
        PolicyInput::Settled {
            address: peer,
            origin: Origin::Dialed,
            established,
            now_ms: 0,
        },
    );
    assert!(
        actions.iter().any(|a| matches!(
            a,
            PolicyAction::Admit {
                lane: L2capPlan::None,
                ..
            }
        )),
        "ColumbaGatt must admit on GATT-only lane"
    );
    assert_eq!(policy.settled_count(), 1);
    assert_eq!(PeerProtocol::Columba, PeerProtocol::Columba);
    eprintln!("BA-SIM-22 PASS ColumbaGatt admit lane=None");
}
