use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::sync::oneshot;

use crate::engine::Departure;
use crate::interfaces::{
    ConnectionView, InterfaceId, InterfaceKind, InterfaceOriginKind, InterfaceSnapshot,
    InterfaceStatus, InterfaceVitals, Membership, ReportsStatus, StatusView,
};
use crate::interfaces::{IfacContext, IfacSize};
use crate::manifold::driver::{HostCommand, TokioInterfaceStatus};
use crate::manifold::interface_seam::Interface;
use crate::node_introspection::{InterfaceIfacSnapshot, InterfaceInventoryEntry};

use super::super::PrnsNodeHandle;
use super::{drive_interfaces, DriverMsg, Fleet, RuntimeIfac};

struct LiveRun {
    live: Arc<AtomicUsize>,
}

impl LiveRun {
    fn new(live: Arc<AtomicUsize>, overlaps: Arc<AtomicUsize>) -> Self {
        if live.fetch_add(1, Ordering::SeqCst) != 0 {
            overlaps.fetch_add(1, Ordering::SeqCst);
        }
        Self { live }
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

fn handle() -> (PrnsNodeHandle, UnboundedReceiver<HostCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    (PrnsNodeHandle::over(commands), command_rx)
}

struct StatusInterface {
    tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl StatusInterface {
    fn new(tag: &[u8]) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::Pipe, tag);
        Self {
            tag: tag.to_vec(),
            status: TokioInterfaceStatus::new(id, crate::interfaces::ConnectionState::Connected),
        }
    }

    fn id(&self) -> InterfaceId {
        self.status.id()
    }
}

impl Interface for StatusInterface {
    const HW_MTU: usize = crate::wire::BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Pipe;

    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }

    fn descriptor(&self) -> crate::interfaces::InterfaceDescriptor {
        crate::interfaces::InterfaceDescriptor {
            id: self.id(),
            capabilities: crate::interfaces::InterfaceCapabilities {
                ingress: crate::interfaces::IngressCapability::Enabled,
                egress: crate::interfaces::EgressCapability::Enabled(
                    crate::interfaces::TransportCapability::CrossInterfaceOnly,
                ),
            },
            mode: crate::interfaces::InterfaceMode::Boundary,
            gravity: crate::interfaces::InterfaceGravity::new(-27),
            bitrate: crate::interfaces::BitrateBps::guess(1_000_000),
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: crate::interfaces::AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
            common: crate::interfaces::InterfaceCommonPolicy::RNS_DEFAULT,
        }
    }

    async fn run<S: crate::manifold::interface_seam::InterfaceSeam>(self, _seam: S) {}
}

impl ReportsStatus for StatusInterface {
    fn status_view(&self) -> Option<StatusView> {
        let status = self.status.clone();
        Some(Arc::new(move || std::vec![InterfaceVitals::of(&status)]))
    }

    fn connection_view(&self) -> Option<ConnectionView> {
        Some(ConnectionView::of(self.status.clone()))
    }
}

#[tokio::test]
async fn runtime_attachment_carries_ifac_wire_and_status_metadata() {
    let (handle, mut command_rx) = handle();
    let interface = StatusInterface::new(b"protected-wire");
    let id = interface.id();
    let ifac = IfacContext::derive(Some("private-net"), Some("secret"), IfacSize::WIDE).unwrap();
    let signature = ifac.ifac_signature();
    let _attached =
        handle.add_interface_with_ifac_name(interface, ifac, Some("private-net".into()));

    let HostCommand::AddInterface(add) = command_rx.recv().await.unwrap() else {
        panic!("expected an interface add");
    };
    assert_eq!(
        add.connection.as_ref().map(ConnectionView::connection),
        Some(crate::interfaces::ConnectionState::Connected)
    );
    let wire_ifac = add.ifac.unwrap();
    assert_eq!(wire_ifac.ifac_signature(), signature);
    assert_eq!(wire_ifac.ifac_size(), IfacSize::WIDE);
    assert!(handle.set_interface_name(id, "Protected wire"));

    assert_eq!(
        handle.interface_inventory(),
        std::vec![InterfaceInventoryEntry {
            name: Some("Protected wire".into()),
            origin: InterfaceOriginKind::Configured,
            snapshot: InterfaceSnapshot {
                id,
                mode: crate::interfaces::InterfaceMode::Boundary,
                gravity: crate::interfaces::InterfaceGravity::new(-27),
                connection: crate::interfaces::ConnectionState::Connected,
                failure_reason: None,
                rx_bytes: 0,
                tx_bytes: 0,
                transfer_rates: None,
                destinations: 0,
                links: 0,
                transported_links: 0,
                membership: Membership::Independent,
            },
            ifac: Some(InterfaceIfacSnapshot {
                signature,
                size: IfacSize::WIDE,
                network_name: Some("private-net".into()),
            }),
        }]
    );
}

#[tokio::test]
async fn a_fleet_member_inherits_its_supervisors_ifac() {
    let supervisor = InterfaceId::new([0x71; 8]);
    let (mut fleet, mut tail) = Fleet::detached(supervisor);
    let ifac = IfacContext::derive(Some("fleet-net"), None, IfacSize::NARROW).unwrap();
    let signature = ifac.ifac_signature();
    fleet.ifac = Some(RuntimeIfac {
        context: ifac,
        network_name: Some("fleet-net".into()),
    });
    let interface = StatusInterface::new(b"fleet-member");
    let id = interface.id();
    let _attached = fleet.add(interface);

    let HostCommand::AddInterface(add) = tail._commands.recv().await.unwrap() else {
        panic!("expected a fleet member add");
    };
    assert_eq!(add.ifac.unwrap().ifac_signature(), signature);
    let map = fleet.interfaces.lock().unwrap();
    assert_eq!(
        map.get(&id)
            .unwrap()
            .ifac
            .as_ref()
            .unwrap()
            .network_name
            .as_deref(),
        Some("fleet-net")
    );
}

#[tokio::test]
async fn a_self_completing_interface_run_deregisters_it() {
    let (msg_tx, msg_rx) = mpsc::unbounded_channel::<DriverMsg>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<HostCommand>();

    let id = InterfaceId::from_channel_tag(
        crate::interfaces::InterfaceKind::LocalClient,
        b"ephemeral-peer",
    );
    msg_tx
        .send(DriverMsg::Add {
            id,
            supervisor: None,
            build: Box::new(|| {
                let run: Pin<Box<dyn Future<Output = ()>>> = Box::pin(async {});
                run
            }),
        })
        .expect("the driver is listening");
    drop(msg_tx);

    let interfaces = Arc::new(Mutex::new(HashMap::new()));
    tokio::join!(
        drive_interfaces(std::vec![], msg_rx, cmd_tx, interfaces),
        async {
            let command = tokio::time::timeout(std::time::Duration::from_secs(1), cmd_rx.recv())
                .await
                .expect("the driver culls the completed interface within 1s")
                .expect("the command channel stays open");
            assert!(
                    matches!(
                        command,
                        HostCommand::RemoveInterface {
                            id: removed,
                            departure: Departure::MayReturn,
                        } if removed == id
                    ),
                    "an interface whose run ended on its own deregisters itself as a may-return departure"
                );
        }
    );
}

#[tokio::test]
async fn replacement_waits_for_the_previous_same_id_run_to_drop() {
    let (msg_tx, msg_rx) = mpsc::unbounded_channel::<DriverMsg>();
    let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let live = Arc::new(AtomicUsize::new(0));
    let overlaps = Arc::new(AtomicUsize::new(0));
    let id = InterfaceId::from_channel_tag(
        crate::interfaces::InterfaceKind::BluetoothAuto,
        b"same-id-replacement",
    );

    let exercise = async {
        for _ in 0..64 {
            let live = live.clone();
            let overlaps = overlaps.clone();
            let started_tx = started_tx.clone();
            msg_tx
                .send(DriverMsg::Add {
                    id,
                    supervisor: None,
                    build: Box::new(move || {
                        let guard = LiveRun::new(live, overlaps);
                        let _ = started_tx.send(());
                        Box::pin(async move {
                            let _guard = guard;
                            core::future::pending().await
                        })
                    }),
                })
                .expect("the driver is listening");
            started_rx.recv().await.expect("the run starts");
            msg_tx
                .send(DriverMsg::Stop { id })
                .expect("the driver is listening");
        }
        drop(msg_tx);
    };

    tokio::join!(
        drive_interfaces(
            std::vec![],
            msg_rx,
            cmd_tx,
            Arc::new(Mutex::new(HashMap::new())),
        ),
        exercise,
    );

    assert_eq!(overlaps.load(Ordering::SeqCst), 0);
    assert_eq!(live.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn panicking_interfaces_are_deregistered_without_stopping_the_driver() {
    let (msg_tx, msg_rx) = mpsc::unbounded_channel::<DriverMsg>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<HostCommand>();
    let build_id = InterfaceId::from_channel_tag(
        crate::interfaces::InterfaceKind::LocalClient,
        b"panicking-build",
    );
    let run_id = InterfaceId::from_channel_tag(
        crate::interfaces::InterfaceKind::LocalClient,
        b"panicking-run",
    );
    let healthy_id = InterfaceId::from_channel_tag(
        crate::interfaces::InterfaceKind::LocalClient,
        b"healthy-run",
    );
    msg_tx
        .send(DriverMsg::Add {
            id: build_id,
            supervisor: None,
            build: Box::new(|| std::panic::panic_any("interface build")),
        })
        .expect("the driver is listening");
    msg_tx
        .send(DriverMsg::Add {
            id: run_id,
            supervisor: None,
            build: Box::new(|| Box::pin(async { std::panic::panic_any("interface run") })),
        })
        .expect("the driver is listening");
    msg_tx
        .send(DriverMsg::Add {
            id: healthy_id,
            supervisor: None,
            build: Box::new(|| Box::pin(async {})),
        })
        .expect("the driver is listening");
    drop(msg_tx);

    drive_interfaces(
        std::vec![],
        msg_rx,
        cmd_tx,
        Arc::new(Mutex::new(HashMap::new())),
    )
    .await;

    let mut removed = std::vec::Vec::new();
    while let Ok(command) = cmd_rx.try_recv() {
        if let HostCommand::RemoveInterface { id, .. } = command {
            removed.push(id);
        }
    }
    removed.sort_unstable();
    let mut expected = std::vec![build_id, run_id, healthy_id];
    expected.sort_unstable();
    assert_eq!(removed, expected);
}

#[tokio::test]
async fn a_panicking_supervisor_stops_its_members() {
    let (msg_tx, msg_rx) = mpsc::unbounded_channel::<DriverMsg>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (panic_tx, panic_rx) = oneshot::channel();
    let (member_ready_tx, member_ready_rx) = oneshot::channel();
    let supervisor_id = InterfaceId::from_channel_tag(
        crate::interfaces::InterfaceKind::AutoWifi,
        b"panicking-supervisor",
    );
    let member_id = InterfaceId::from_channel_tag(
        crate::interfaces::InterfaceKind::WifiPeer,
        b"supervised-member",
    );
    msg_tx
        .send(DriverMsg::Add {
            id: supervisor_id,
            supervisor: None,
            build: Box::new(|| {
                Box::pin(async move {
                    let _ = panic_rx.await;
                    std::panic::panic_any("supervisor run");
                })
            }),
        })
        .expect("the driver is listening");
    msg_tx
        .send(DriverMsg::Add {
            id: member_id,
            supervisor: Some(supervisor_id),
            build: Box::new(|| {
                let _ = member_ready_tx.send(());
                Box::pin(std::future::pending())
            }),
        })
        .expect("the driver is listening");
    drop(msg_tx);

    tokio::join!(
        drive_interfaces(
            std::vec![],
            msg_rx,
            cmd_tx,
            Arc::new(Mutex::new(HashMap::new())),
        ),
        async {
            let _ = member_ready_rx.await;
            let _ = panic_tx.send(());
        },
    );

    let mut removed = std::vec::Vec::new();
    while let Ok(command) = cmd_rx.try_recv() {
        if let HostCommand::RemoveInterface { id, .. } = command {
            removed.push(id);
        }
    }
    removed.sort_unstable();
    let mut expected = std::vec![supervisor_id, member_id];
    expected.sort_unstable();
    assert_eq!(removed, expected);
}
