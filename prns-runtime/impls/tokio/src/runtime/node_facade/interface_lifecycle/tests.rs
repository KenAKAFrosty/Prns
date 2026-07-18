use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::Departure;
use crate::interfaces::ifac::{IfacContext, IfacSize};
use crate::interfaces::{
    ConnectionView, InterfaceId, InterfaceKind, InterfaceOriginKind, InterfaceSnapshot,
    InterfaceStatus, InterfaceVitals, Membership, ReportsStatus, StatusView,
};
use crate::node_introspection::{InterfaceIfacSnapshot, InterfaceInventoryEntry};
use crate::reactor::driver::{HostCommand, TokioInterfaceStatus};
use crate::reactor::interface_seam::Interface;

use super::super::PrnsNodeHandle;
use super::{drive_interfaces, DriverMsg, Fleet, RuntimeIfac};

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
            mode: crate::interfaces::InterfaceMode::Full,
            bitrate: crate::interfaces::BitrateBps::guess(1_000_000),
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: crate::interfaces::AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
            common: crate::interfaces::InterfaceCommonPolicy::RNS_DEFAULT,
        }
    }

    async fn run<S: crate::reactor::interface_seam::InterfaceSeam>(self, _seam: S) {}
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
