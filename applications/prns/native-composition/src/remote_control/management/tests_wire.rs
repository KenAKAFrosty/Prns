#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::interfaces::{
    ConnectionState, DiscoveryGroupSet, InterfaceId, InterfaceKind, InterfaceMode, INTERFACE_ID_LEN,
};
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::prelude::*;
use personal_rns::runtime::{
    NodePersistence, RemoteControlHostCommand, RemoteControlHostCommandError,
    RemoteControlHostControls, RemoteControlHostResponse,
};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

struct Controls {
    writes: Arc<AtomicUsize>,
    hold: Arc<AtomicBool>,
    held_read: Arc<AtomicBool>,
}

fn interface(byte: u8) -> InterfaceId {
    InterfaceId::new([byte; INTERFACE_ID_LEN])
}

impl RemoteControlHostControls for Controls {
    async fn execute_remote_control(
        &self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
        Ok(match command {
            RemoteControlHostCommand::DescribeBuild => RemoteControlHostResponse::DescribeBuild(
                core::RemoteControlBuildVersion::from_text("test-build").expect("build label"),
            ),
            RemoteControlHostCommand::DescribePower => RemoteControlHostResponse::DescribePower(
                prns_core::capabilities::power::PowerSnapshot::UNKNOWN,
            ),
            RemoteControlHostCommand::InventoryInterfaces { page } => {
                let mut inventory = core::RemoteControlInterfaceInventory::empty();
                let (first, last) = match page {
                    core::RemoteControlInterfacePage::First => (1, 4),
                    core::RemoteControlInterfacePage::After(cursor)
                        if cursor.id() == interface(4) =>
                    {
                        (5, 5)
                    }
                    _ => return Err(RemoteControlHostCommandError::ApplyFailed),
                };
                for byte in first..=last {
                    inventory
                        .push(core::RemoteControlInterfaceEntry {
                            id: interface(byte),
                            kind: InterfaceKind::BluetoothAuto,
                            mode: InterfaceMode::Full,
                            connection: ConnectionState::Connected,
                            enabled: true,
                            tx_bytes: u64::MAX,
                            rx_bytes: 10,
                            links: 1,
                            rate_bytes_per_sec: 20,
                        })
                        .expect("entry fits");
                }
                if last == 4 {
                    inventory
                        .set_continuation(core::RemoteControlInterfaceContinuation::More(
                            core::RemoteControlInterfaceCursor::after(interface(4)),
                        ))
                        .expect("continuation");
                }
                RemoteControlHostResponse::InventoryInterfaces(inventory)
            }
            RemoteControlHostCommand::InventoryInterfaceConfig { id } => {
                assert_eq!(id, interface(1));
                if self.hold.load(Ordering::SeqCst) {
                    self.held_read.store(true, Ordering::SeqCst);
                    std::future::pending::<()>().await;
                }
                let mut card = core::RemoteControlInterfaceCard::empty();
                card.set_name("Bluetooth").expect("name fits");
                RemoteControlHostResponse::InventoryInterfaceConfig(
                    core::RemoteControlInterfaceConfigOutcome::Card(card),
                )
            }
            RemoteControlHostCommand::InventoryInterfaceDiscoveryGroups { id } => {
                assert_eq!(id, interface(1));
                RemoteControlHostResponse::InventoryInterfaceDiscoveryGroups(
                    core::RemoteControlDiscoveryGroupsInventoryOutcome::Groups(
                        core::RemoteControlDiscoveryGroups::new(DiscoveryGroupSet::reticulum()),
                    ),
                )
            }
            RemoteControlHostCommand::InventoryInterfacePeers { id, .. } => {
                RemoteControlHostResponse::InventoryInterfacePeers(
                    core::RemoteControlInterfacePeersOutcome::Page(
                        core::RemoteControlInterfacePeerPage::empty(id),
                    ),
                )
            }
            RemoteControlHostCommand::SetInterfacePower { id, power } => {
                assert_eq!(id, interface(1));
                assert_eq!(power, core::RemoteControlInterfacePower::On);
                self.writes.fetch_add(1, Ordering::SeqCst);
                if self.hold.load(Ordering::SeqCst) {
                    std::future::pending::<()>().await;
                }
                RemoteControlHostResponse::SetInterfacePower(
                    core::RemoteControlPowerOutcome::Applied,
                )
            }
            _ => return Err(RemoteControlHostCommandError::Unsupported),
        })
    }
}

fn secrets(controller: u8, target: u8) -> core::RemoteControlNodeIdentitySecrets {
    core::RemoteControlNodeIdentitySecrets::new(
        core::RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
            [controller; IDENTITY_SECRET_KEY_LEN],
        )),
        core::RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
            [target; IDENTITY_SECRET_KEY_LEN],
        )),
    )
    .expect("distinct identities")
}

#[tokio::test(flavor = "current_thread")]
async fn management_uses_authenticated_public_operations_and_preserves_ambiguous_writes() {
    tokio::task::LocalSet::new().run_until(async {
        let directory = tempfile::tempdir().expect("fixture persistence");
        let target_secrets = secrets(0x83, 0x84);
        let controller_secrets = secrets(0x85, 0x86);
        let target_identity = core::RemoteControlTargetIdentity::new(*target_secrets.identities().target().public_keys());
        let controller_identity = *controller_secrets.identities().controller();
        let target_endpoint = target_identity.endpoint().destination_hash();
        let mut available = core::RemoteControlRequestSet::only(core::RemoteControlRequestKind::Describe);
        for kind in [core::RemoteControlRequestKind::DescribeBuild, core::RemoteControlRequestKind::DescribePower, core::RemoteControlRequestKind::InventoryInterfaces, core::RemoteControlRequestKind::InventoryInterfaceConfig, core::RemoteControlRequestKind::InventoryInterfacePeers, core::RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups, core::RemoteControlRequestKind::SetInterfacePower] { let _ = available.insert(kind); }
        let grants = [core::RemoteControlControllerGrant::new(controller_identity, core::RemoteControlControllerAuthority::Operator, core::RemoteControlRequestSet::all_operator()).expect("fixture grant")];
        let writes = Arc::new(AtomicUsize::new(0)); let hold = Arc::new(AtomicBool::new(false));
        let held_read = Arc::new(AtomicBool::new(false));
        let target = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: core::RemoteControlService::with_capabilities(target_secrets, core::RemoteControlInitialControllerGrants::Grants(core::RemoteControlControllerGrants::try_from(grants.as_slice()).expect("initial grants")), core::RemoteControlSelfAnnouncement::Unavailable, core::RemoteControlCapabilities::from_requests(available).expect("describe supported")),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: Controls { writes: Arc::clone(&writes), hold: Arc::clone(&hold), held_read: Arc::clone(&held_read) }, storage: GrowableHeap,
            request_endpoints: request_endpoints![], on_event: |_, _| {}, interfaces: ManuallyAttached,
            persistence: NodePersistence::custom_dir(directory.path().join("target")).expect("target persistence"),
        });
        let target_handle = target.handle();
        let server = TcpServer::bind("127.0.0.1:0").await.expect("server bind");
        let address = server.local_addr().expect("server address").to_string();
        let _server = target_handle.supervise(server);
        let controller = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: core::RemoteControlService::new(controller_secrets, core::RemoteControlInitialControllerGrants::Nobody, core::RemoteControlSelfAnnouncement::Unavailable),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: personal_rns::runtime::NoRemoteControlHostControls, storage: GrowableHeap,
            request_endpoints: request_endpoints![], on_event: |_, _| {}, interfaces: ManuallyAttached,
            persistence: NodePersistence::custom_dir(directory.path().join("controller")).expect("controller persistence"),
        });
        let controller_handle = controller.handle();
        let _client = controller_handle.attach(TcpClientInterface::new(address));
        let target_bytes = target_identity.identity_hash().as_bytes().to_vec();
        let scenario = async {
            controller_handle.set_remote_control_target_access(core::RemoteControlTargetAccess::new(target_identity, core::RemoteControlControllerAuthority::Operator, core::RemoteControlRequestSet::all_operator()).expect("fixture target access")).await.expect("access persisted");
            while !controller_handle.interface_timing_inventory().iter().any(|interface| interface.connection.is_online()) || !target_handle.interface_timing_inventory().iter().any(|interface| interface.connection.is_online()) { tokio::task::yield_now().await; }
            target_handle.announce_now(AnnounceNow { destination: target_endpoint, target: AnnounceTarget::AllInterfaces, app_data: AnnounceAppData::Registered }).await.expect("target announces");
            while controller_handle.route(target_endpoint).await.is_none() { tokio::task::yield_now().await; }
            let deadline = || Instant::now() + Duration::from_secs(3);
            let read_query = |query| ReadRemoteNodeInput { target_identity_fingerprint: target_bytes.clone(), query };
            let ReadRemoteNodeOutcome::Read { data: RemoteNodeData::Overview { overview }, available_requests, .. } = read(&controller_handle, read_query(RemoteNodeQuery::Overview), deadline()).await else { panic!("overview failed"); };
            assert_eq!(overview.firmware.as_deref(), Some("test-build"));
            assert_eq!(overview.power.expect("power supported").battery_percent, None);
            assert!(available_requests.contains(&crate::contract::RemoteControlRequestKind::SetInterfacePower));
            let first = overview.interfaces.expect("interfaces supported"); assert_eq!(first.entries.len(), 4); assert_eq!(first.entries[0].tx_bytes, u64::MAX);
            let ReadRemoteNodeOutcome::Read { data: RemoteNodeData::Interfaces { page }, .. } = read(&controller_handle, read_query(RemoteNodeQuery::Interfaces { after: first.next }), deadline()).await else { panic!("second page failed"); };
            assert_eq!(page.entries.len(), 1); assert_eq!(page.next, None);
            let ReadRemoteNodeOutcome::Read { data: RemoteNodeData::Interface { details }, .. } = read(&controller_handle, read_query(RemoteNodeQuery::Interface { interface_id: interface(1).as_bytes().to_vec() }), deadline()).await else { panic!("details failed"); };
            assert!(matches!(details.configuration, RemoteInterfaceConfiguration::Available { .. }));
            assert_eq!(details.discovery_groups, RemoteDiscoveryGroups::Available { groups: vec!["reticulum".to_owned()] });
            hold.store(true, Ordering::SeqCst);
            let mut abandoned = Box::pin(read(&controller_handle, read_query(RemoteNodeQuery::Interface { interface_id: interface(1).as_bytes().to_vec() }), deadline()));
            tokio::select! {
                result = &mut abandoned => panic!("held read unexpectedly completed: {result:?}"),
                () = async { while !held_read.load(Ordering::SeqCst) { tokio::task::yield_now().await; } } => {},
            }
            drop(abandoned);
            tokio::time::timeout(Duration::from_secs(1), async {
                while controller_handle.link_count().await != 0 || target_handle.link_count().await != 0 { tokio::task::yield_now().await; }
            }).await.expect("abandoned management read retires both links");
            hold.store(false, Ordering::SeqCst);
            let dispatched = AtomicBool::new(false);
            let unsupported = ChangeRemoteNodeInput { target_identity_fingerprint: target_bytes.clone(), change: RemoteNodeChange::GnssPower { enabled: true } };
            assert!(matches!(change(&controller_handle, unsupported, deadline(), &dispatched).await, RemoteChangeStatus::Failed { stage: RemoteManagementFailureStage::Unsupported, .. }));
            assert!(!dispatched.load(Ordering::SeqCst)); assert_eq!(writes.load(Ordering::SeqCst), 0);
            let input = ChangeRemoteNodeInput { target_identity_fingerprint: target_bytes.clone(), change: RemoteNodeChange::InterfacePower { interface_id: interface(1).as_bytes().to_vec(), enabled: true } };
            assert_eq!(change(&controller_handle, input.clone(), deadline(), &dispatched).await, RemoteChangeStatus::Applied);
            assert!(dispatched.load(Ordering::SeqCst)); assert_eq!(writes.load(Ordering::SeqCst), 1);
            hold.store(true, Ordering::SeqCst); dispatched.store(false, Ordering::SeqCst);
            assert_eq!(change(&controller_handle, input, Instant::now() + Duration::from_millis(300), &dispatched).await, RemoteChangeStatus::OutcomeUnknown { reason: RemoteControlAnnounceUnknownReason::Timeout });
            assert!(dispatched.load(Ordering::SeqCst)); assert_eq!(writes.load(Ordering::SeqCst), 2, "lost response must not replay the write");
            tokio::time::timeout(Duration::from_secs(1), async {
                while controller_handle.link_count().await != 0 || target_handle.link_count().await != 0 { tokio::task::yield_now().await; }
            }).await.expect("owned links retire after completion and timeout");
        };
        tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(12), scenario) => result.expect("management scenario finishes"),
            result = target.run() => panic!("target stopped: {result:?}"),
            result = controller.run() => panic!("controller stopped: {result:?}"),
        }
    }).await;
}
