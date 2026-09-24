use super::*;
use personal_rns::interfaces::{tcp, InterfaceMode};
use personal_rns::runtime::{ConnectRemoteControlTargetError, SendError};
use personal_rns::tcp::TcpServerConnection;

const DISCOVERY_BUDGET_MS: u64 = 20_000;
const REBIND_BUDGET_MS: u64 = 8_000;
const REPLACEMENT_CYCLES: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RestartedNode {
    Target,
    Gateway,
    Controller,
}
impl RestartedNode {
    pub fn index(self) -> usize {
        match self {
            Self::Target => TARGET,
            Self::Gateway => HEALTHY,
            Self::Controller => PRIMARY,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BindingFault {
    MissingDevice,
    CoalescedDownUp,
    FatalReceive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Recovery {
    MissingUnicastPaths,
    DelayedRadioBoot,
    PageHandshakeLoss,
    RetiredPageRoute,
    AdapterReplacement,
    FailedHandshake,
    RetainedRestart(RestartedNode),
    ManagedBinding(BindingFault),
}

impl Recovery {
    pub fn persistence(self) -> Persistence {
        match self {
            Self::DelayedRadioBoot | Self::RetainedRestart(_) => {
                Persistence::Retained((0..NODE_COUNT).map(|_| RetainedState::new()).collect())
            }
            Self::MissingUnicastPaths
            | Self::PageHandshakeLoss
            | Self::RetiredPageRoute
            | Self::AdapterReplacement
            | Self::FailedHandshake
            | Self::ManagedBinding(_) => Persistence::Disabled,
        }
    }

    pub fn bindings(self) -> Bindings {
        match self {
            Self::DelayedRadioBoot => {
                let device = DeviceControl::new();
                device.set_presence(Presence::Absent);
                Bindings::ManagedTarget(device)
            }
            Self::RetiredPageRoute | Self::ManagedBinding(_) => {
                Bindings::ManagedTarget(DeviceControl::new())
            }
            Self::MissingUnicastPaths
            | Self::PageHandshakeLoss
            | Self::AdapterReplacement
            | Self::FailedHandshake
            | Self::RetainedRestart(_) => Bindings::Explicit,
        }
    }
}

pub(super) fn gateway(lab: &mut Lab<'_>) {
    gateway_at(lab, HEALTHY);
}
pub(super) fn gateway_at(lab: &mut Lab<'_>, gateway_node: usize) {
    for attached in lab.wired.drain(..) {
        attached.teardown();
    }
    assert!(lab.settle().is_empty());
    for from in 0..NODE_COUNT {
        for to in 0..NODE_COUNT {
            if from != to {
                lab.medium.set_path(
                    lab.nodes[from].radio,
                    lab.nodes[to].radio,
                    if [TARGET, HEALTHY].contains(&from) && [TARGET, HEALTHY].contains(&to) {
                        PathState::Reachable
                    } else {
                        PathState::Isolated
                    },
                );
            }
        }
    }
    let (controller, router) = tokio::io::duplex(8192);
    let client = TcpServerConnection::new(
        format!("lab-client-to-{gateway_node}").into_bytes(),
        controller,
        tcp::TCP_BITRATE_ESTIMATE,
    );
    let mut policy = tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE);
    policy.mode = InterfaceMode::Gateway;
    let server = TcpServerConnection::with_policy(
        format!("lab-gateway-{gateway_node}").into_bytes(),
        router,
        policy,
    );
    lab.wired
        .push(lab.nodes[PRIMARY].handle.add_interface(client));
    lab.wired
        .push(lab.nodes[gateway_node].handle.add_interface(server));
    assert!(lab.settle().is_empty());
}

pub(super) fn discover(lab: &mut Lab<'_>) {
    let handle = lab.nodes[PRIMARY].handle.clone();
    let task = lab.insert(async move {
        Event::Path(
            handle
                .request_path(target(TARGET).endpoint().destination_hash())
                .await,
        )
    });
    let Event::Path(result) = lab.wait(task, DISCOVERY_BUDGET_MS) else {
        panic!("path actor");
    };
    result.expect("cold discovery through gateway");
}

pub(super) fn announce_page(lab: &mut Lab<'_>, index: usize) {
    let handle = lab.nodes[index].handle.clone();
    lab.complete(async move {
        handle
            .announce_now(personal_rns::engine::AnnounceNow {
                destination: page(index).destination_hash().expect("page hash"),
                target: personal_rns::engine::AnnounceTarget::AllInterfaces,
                app_data: personal_rns::engine::AnnounceAppData::Registered,
            })
            .await
            .expect("controller-directed page announce");
    });
    assert!(lab.advance(1000).is_empty());
}

pub(super) fn connection(
    lab: &mut Lab<'_>,
) -> Result<personal_rns::routing::links::LinkId, ConnectRemoteControlTargetError> {
    let handle = lab.nodes[PRIMARY].handle.clone();
    let task = lab.insert(async move {
        Event::Connection(
            handle
                .connect_remote_control_target(target(TARGET).identity_hash())
                .await
                .map(|c| c.connection().link_id()),
        )
    });
    let Event::Connection(result) = lab.wait(task, 60_000) else {
        panic!("connection actor");
    };
    result
}

pub fn adapter_replacement(lab: &mut Lab<'_>) {
    gateway(lab);
    announce_page(lab, TARGET);
    announce_page(lab, HEALTHY);
    discover(lab);
    let link = lab.link(PRIMARY);
    lab.app(PRIMARY, link, b"cold");
    discover(lab);
    let old = lab.nodes[TARGET].radio;
    lab.replace_adapter(TARGET);
    assert_ne!(old, lab.nodes[TARGET].radio);
    lab.medium.set_path(
        lab.nodes[TARGET].radio,
        lab.nodes[HEALTHY].radio,
        PathState::Reachable,
    );
    lab.medium.set_path(
        lab.nodes[HEALTHY].radio,
        lab.nodes[TARGET].radio,
        PathState::Reachable,
    );
    // Only target/router are radio neighbors; the controller must use TCP.
    for index in [PRIMARY, OUTSIDER] {
        lab.medium.set_path(
            lab.nodes[TARGET].radio,
            lab.nodes[index].radio,
            PathState::Isolated,
        );
        lab.medium.set_path(
            lab.nodes[index].radio,
            lab.nodes[TARGET].radio,
            PathState::Isolated,
        );
    }
    announce_page(lab, TARGET);
    announce_page(lab, HEALTHY);
    discover(lab);
    let link = connection(lab).expect("recovered connection");
    lab.app(PRIMARY, link, b"replaced");
}

pub fn failed_handshake(lab: &mut Lab<'_>) {
    gateway(lab);
    announce_page(lab, TARGET);
    announce_page(lab, HEALTHY);
    discover(lab);
    let link = lab.link(PRIMARY);
    lab.app(PRIMARY, link, b"before-outage");
    for (from, to) in [(TARGET, HEALTHY), (HEALTHY, TARGET)] {
        lab.medium.set_path(
            lab.nodes[from].radio,
            lab.nodes[to].radio,
            PathState::Isolated,
        );
    }
    assert_eq!(
        connection(lab),
        Err(ConnectRemoteControlTargetError::EstablishLink(
            SendError::Failed(personal_rns::engine::EstablishLinkFailure::Timeout)
        ))
    );
    assert!(lab.advance(1000).is_empty());
    for (from, to) in [(TARGET, HEALTHY), (HEALTHY, TARGET)] {
        lab.medium.set_path(
            lab.nodes[from].radio,
            lab.nodes[to].radio,
            PathState::Reachable,
        );
    }
    announce_page(lab, TARGET);
    announce_page(lab, HEALTHY);
    discover(lab);
    let link = connection(lab).expect("rediscovered after failed handshake");
    lab.app(PRIMARY, link, b"recovered");
}

pub fn retained_restart(lab: &mut Lab<'_>, restarted: RestartedNode) {
    gateway(lab);
    announce_page(lab, TARGET);
    announce_page(lab, HEALTHY);
    discover(lab);
    let link = lab.link(PRIMARY);
    lab.app(PRIMARY, link, b"before-restart");
    for index in 0..NODE_COUNT {
        lab.flush(index);
    }
    lab.restart(restarted.index());
    gateway(lab);
    announce_page(lab, TARGET);
    announce_page(lab, HEALTHY);
    discover(lab);
    let link = connection(lab).expect("retained target pin and restored grant");
    lab.app(PRIMARY, link, b"retained-recovery");
}

pub fn managed_binding(lab: &mut Lab<'_>, device: &DeviceControl, fault: BindingFault) {
    gateway(lab);
    device.neighbor(lab.nodes[HEALTHY].radio);
    announce_page(lab, TARGET);
    announce_page(lab, HEALTHY);
    discover(lab);
    let link = lab.link(PRIMARY);
    lab.app(PRIMARY, link, b"managed-initial");
    let gateway_link = lab.link_to(PRIMARY, HEALTHY);
    let supervisor = lab.nodes[TARGET].adapter_id();
    let peer_ids = lab.peer_ids(TARGET);
    assert_eq!(peer_ids, [scope(TARGET).peer_id(mac(HEALTHY))]);
    let mut maximum_rebind_ticks = 0;
    for cycle in 0..REPLACEMENT_CYCLES {
        let previous = device.radio().expect("bound generation");
        match fault {
            BindingFault::MissingDevice => {
                let attempts = device.attempts();
                device.set_presence(Presence::Absent);
                for _ in 0..REBIND_BUDGET_MS {
                    assert!(lab.advance(1).is_empty());
                    if device.attempts() > attempts {
                        break;
                    }
                }
                assert_eq!(device.radio(), None);
                assert!(
                    device.attempts() > attempts,
                    "missing device reaches the actual source retry"
                );
                assert!(
                    device.attempts() - attempts <= 4,
                    "retry backoff prevents busy polling"
                );
                assert_eq!(
                    lab.nodes[TARGET].handle.interfaces().len(),
                    1,
                    "only the managed parent remains"
                );
                lab.app(PRIMARY, gateway_link, b"gateway-during-radio-outage");
                device.set_presence(Presence::Present);
            }
            BindingFault::CoalescedDownUp => {
                device.set_presence(Presence::Absent);
                device.set_presence(Presence::Present);
            }
            BindingFault::FatalReceive => lab
                .medium
                .set_receive_behavior(previous, ReceiveBehavior::Failed),
        }
        let mut elapsed = 0;
        loop {
            if device.radio().is_some_and(|current| current != previous) {
                break;
            }
            assert!(elapsed < REBIND_BUDGET_MS, "bounded automatic rebind");
            assert!(lab.advance(1).is_empty());
            elapsed += 1;
        }
        let current = device.radio().expect("replacement binding");
        maximum_rebind_ticks = maximum_rebind_ticks.max(elapsed);
        lab.nodes[TARGET].radio = current;
        assert_eq!(lab.nodes[TARGET].adapter_id(), supervisor);
        assert!(lab
            .medium
            .snapshot()
            .events
            .iter()
            .any(|event| matches!(event, HaLowEvent::Detached { radio } if *radio == previous)));
        announce_page(lab, TARGET);
        discover(lab);
        let link = connection(lab).expect("rebound control connection");
        lab.app(PRIMARY, link, &[cycle]);
        assert_eq!(lab.peer_ids(TARGET), peer_ids);
    }
    lab.measurements.push(Measurement::DeviceRecovery {
        cycles: REPLACEMENT_CYCLES,
        maximum_rebind_ticks,
        open_attempts: device.attempts(),
    });
}

#[test]
fn a_delayed_old_generation_cannot_admit_an_app_message_after_rebinding() {
    use personal_rns::remote_control::{RemoteControlAppMessage, RemoteControlRequest};
    let device = DeviceControl::new();
    with_fixture(
        medium(),
        42,
        Topology::Shared,
        Persistence::Disabled,
        Bindings::ManagedTarget(device.clone()),
        |lab| {
            gateway(lab);
            device.neighbor(lab.nodes[HEALTHY].radio);
            announce_page(lab, TARGET);
            announce_page(lab, HEALTHY);
            discover(lab);
            let old_link = lab.link(PRIMARY);
            let retired = device.radio().expect("bound radio");
            lab.arm(
                HEALTHY,
                FaultDestination::Peer(mac(TARGET)),
                prns_simulation::TransmissionAction::Delay {
                    by: prns_simulation::SimulationDurationInTicks::from_ticks(50),
                },
            );
            let pending = lab.request(
                PRIMARY,
                old_link,
                RemoteControlRequest::AppMessage(
                    RemoteControlAppMessage::from_slice(b"obsolete").expect("bounded message"),
                ),
            );
            assert!(lab.settle().is_empty());
            assert_eq!(lab.medium.snapshot().pending, 1);
            device.set_presence(Presence::Absent);
            device.set_presence(Presence::Present);
            let Event::Response(result) = lab.wait(pending, 1000) else {
                panic!("request actor");
            };
            assert_eq!(
                result,
                Err(SendError::Failed(
                    personal_rns::engine::SendRequestFailure::Timeout
                ))
            );
            assert!(!lab
                .calls
                .borrow()
                .iter()
                .any(|call| call.payload == b"obsolete"));
            assert!(
                lab.medium
                    .snapshot()
                    .events
                    .iter()
                    .any(|event| matches!(event,
            HaLowEvent::Delivery { to, outcome: DeliveryOutcome::Detached, .. } if *to == retired))
            );
            for _ in 0..REBIND_BUDGET_MS {
                if device.radio().is_some_and(|current| current != retired) {
                    break;
                }
                assert!(lab.advance(1).is_empty());
            }
            lab.nodes[TARGET].radio = device.radio().expect("bounded automatic replacement");
            announce_page(lab, TARGET);
            discover(lab);
            let fresh = connection(lab).expect("fresh controller session");
            lab.app(PRIMARY, fresh, b"current");
            wire_contract(lab);
        },
    );
}

pub fn run(lab: &mut Lab<'_>, recovery: Recovery, bindings: &Bindings) {
    match recovery {
        Recovery::MissingUnicastPaths => super::boot::missing_unicast_paths(lab),
        Recovery::DelayedRadioBoot => {
            let Bindings::ManagedTarget(device) = bindings else {
                panic!("managed cold boot configuration");
            };
            super::boot::delayed_radio_boot(lab, device);
        }
        Recovery::PageHandshakeLoss => page_handshake_loss(lab),
        Recovery::RetiredPageRoute => {
            let Bindings::ManagedTarget(device) = bindings else {
                panic!("managed page recovery configuration");
            };
            retired_page_route(lab, device);
        }
        Recovery::AdapterReplacement => adapter_replacement(lab),
        Recovery::FailedHandshake => failed_handshake(lab),
        Recovery::RetainedRestart(node) => retained_restart(lab, node),
        Recovery::ManagedBinding(fault) => {
            let Bindings::ManagedTarget(device) = bindings else {
                panic!("managed recovery configuration");
            };
            managed_binding(lab, device, fault);
        }
    }
    wire_contract(lab);
}

#[test]
fn gateway_recovery_matrix_replays_without_control_preannounce() {
    for seed in [0, 1, 42, 0x5eed] {
        for recovery in [
            Recovery::MissingUnicastPaths,
            Recovery::DelayedRadioBoot,
            Recovery::PageHandshakeLoss,
            Recovery::RetiredPageRoute,
            Recovery::AdapterReplacement,
            Recovery::FailedHandshake,
            Recovery::RetainedRestart(RestartedNode::Target),
            Recovery::RetainedRestart(RestartedNode::Gateway),
            Recovery::RetainedRestart(RestartedNode::Controller),
            Recovery::ManagedBinding(BindingFault::MissingDevice),
            Recovery::ManagedBinding(BindingFault::CoalescedDownUp),
            Recovery::ManagedBinding(BindingFault::FatalReceive),
        ] {
            let replay = || {
                let medium = medium();
                let persistence = recovery.persistence();
                let bindings = recovery.bindings();
                with_fixture(
                    medium.clone(),
                    seed,
                    Topology::Shared,
                    persistence,
                    bindings.clone(),
                    |lab| run(lab, recovery, &bindings),
                );
                if let Bindings::ManagedTarget(device) = bindings {
                    assert_eq!(
                        device.radio(),
                        None,
                        "binding ownership released on shutdown"
                    );
                }
                medium.snapshot()
            };
            assert_eq!(replay(), replay(), "recovery {recovery:?} seed {seed}");
        }
    }
}

pub(super) fn fetch_page(lab: &mut Lab<'_>, destination_node: usize) {
    let handle = lab.nodes[PRIMARY].handle.clone();
    let task = lab.insert(async move {
        let destination = page(destination_node)
            .destination_hash()
            .expect("page address");
        handle
            .request_path(destination)
            .await
            .expect("page discovery");
        Event::Linked(handle.establish_link(destination).await.expect("page link"))
    });
    let Event::Linked(link) = lab.wait(task, 60_000) else {
        panic!("page link actor");
    };
    let task = lab.request_bytes(
        PRIMARY,
        link,
        crate::traffic::RESOURCE_PATH,
        Vec::new(),
        10_000,
    );
    let Event::Response(result) = lab.wait(task, 10_000) else {
        panic!("page response actor");
    };
    assert_eq!(result.expect("page response"), crate::traffic::PAYLOAD);
}

fn retired_page_route(lab: &mut Lab<'_>, device: &DeviceControl) {
    gateway_at(lab, TARGET);
    for from in 0..NODE_COUNT {
        for to in 0..NODE_COUNT {
            if from == to {
                continue;
            }
            let state = if [
                (TARGET, OUTSIDER),
                (OUTSIDER, TARGET),
                (OUTSIDER, HEALTHY),
                (HEALTHY, OUTSIDER),
            ]
            .contains(&(from, to))
            {
                PathState::Reachable
            } else {
                PathState::Isolated
            };
            lab.medium
                .set_path(lab.nodes[from].radio, lab.nodes[to].radio, state);
        }
    }
    announce_page(lab, HEALTHY);
    announce_page(lab, TARGET);
    fetch_page(lab, HEALTHY);
    assert_eq!(lab.peer_ids(TARGET), [scope(TARGET).peer_id(mac(OUTSIDER))]);
    let previous = device.radio().expect("bound gateway");
    device.neighbor(lab.nodes[HEALTHY].radio);
    device.set_presence(Presence::Absent);
    device.set_presence(Presence::Present);
    for _ in 0..REBIND_BUDGET_MS {
        assert!(lab.advance(1).is_empty());
        if device.radio().is_some_and(|radio| radio != previous) {
            break;
        }
    }
    lab.nodes[TARGET].radio = device.radio().expect("replacement gateway binding");
    let mut bytes = [0; 1500];
    let envelope = personal_rns::interfaces::wifi_halow::encode(&[0], &mut bytes)
        .expect("invalid RNS frame in a valid envelope");
    assert_eq!(
        lab.medium
            .inject(lab.nodes[TARGET].radio, mac(HEALTHY), envelope),
        DeliveryOutcome::Queued
    );
    assert!(lab.settle().is_empty());
    assert_eq!(lab.peer_ids(TARGET), [scope(TARGET).peer_id(mac(HEALTHY))]);
    // This source frame establishes only a MAC lane. The cached page still
    // points through the retired relay and cannot be vouched for on that lane.
    fetch_page(lab, HEALTHY);
}

fn page_handshake_loss(lab: &mut Lab<'_>) {
    gateway_at(lab, TARGET);
    announce_page(lab, TARGET);
    announce_page(lab, HEALTHY);
    fetch_page(lab, HEALTHY);
    let healthy = lab.link(HEALTHY);
    let handle = lab.nodes[PRIMARY].handle.clone();
    let task = lab.insert(async move {
        Event::Path(
            handle
                .request_path(page(HEALTHY).destination_hash().expect("page address"))
                .await,
        )
    });
    let Event::Path(path) = lab.wait(task, DISCOVERY_BUDGET_MS) else {
        panic!("page discovery actor");
    };
    path.expect("cached discovery before loss");
    let before = lab.medium.snapshot().events.len();
    lab.arm(
        TARGET,
        FaultDestination::Peer(mac(HEALTHY)),
        prns_simulation::TransmissionAction::Drop,
    );
    let handle = lab.nodes[PRIMARY].handle.clone();
    let failed = lab.insert(async move {
        Event::PageConnection(
            handle
                .establish_link(page(HEALTHY).destination_hash().expect("page address"))
                .await,
        )
    });
    assert!(lab.settle().is_empty());
    let snapshot = lab.medium.snapshot();
    assert_eq!(snapshot.armed_faults, 0);
    let frame = snapshot.events[before..]
        .iter()
        .find_map(|event| match event {
            HaLowEvent::Transmitted {
                from,
                destination: Destination::Peer(peer),
                bytes,
                ..
            } if *from == lab.nodes[TARGET].radio && *peer == mac(HEALTHY) => Some(bytes),
            _ => None,
        })
        .expect("struck radio transmission");
    let (header, _) = personal_rns::wire::WirePacketHeader::parse(
        personal_rns::interfaces::wifi_halow::decode(frame).expect("envelope"),
    )
    .expect("header");
    assert_eq!(
        (
            header.packet_type,
            personal_rns::wire::DestinationHash::from_address(header.address)
        ),
        (
            personal_rns::wire::PacketType::LinkRequest,
            page(HEALTHY).destination_hash().expect("page address")
        )
    );
    lab.app(HEALTHY, healthy, b"control-during-page-handshake-loss");
    let Event::PageConnection(result) = lab.wait(failed, 60_000) else {
        panic!("page failure actor");
    };
    assert_eq!(
        result,
        Err(SendError::Failed(
            personal_rns::engine::EstablishLinkFailure::Timeout
        ))
    );
    fetch_page(lab, HEALTHY);
}
