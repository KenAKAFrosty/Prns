use super::*;
use crate::{
    DeviceSessionEvent, DeviceSessionInput, DeviceSessionMessage, DeviceSessionSwitchboard,
};
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::prelude::*;
use pipecircuit::Switchboard;

fn secrets(controller: u8, target: u8) -> RemoteControlNodeIdentitySecrets {
    RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
            [controller; IDENTITY_SECRET_KEY_LEN],
        )),
        RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
            [target; IDENTITY_SECRET_KEY_LEN],
        )),
    )
    .unwrap()
}

#[tokio::test]
async fn native_nodes_authenticate_and_publish_interface_status_through_the_worker() {
    let target_secrets = secrets(0xD0, 0xD1);
    let target_identity =
        RemoteControlTargetIdentity::new(*target_secrets.identities().target().public_keys());
    let target_destination = target_identity.endpoint().destination_hash();
    let controller_secrets = secrets(0xD2, 0xD3);
    let controller_identity = *controller_secrets.identities().controller();
    let permitted = RemoteControlRequestSet::only(RemoteControlRequestKind::InventoryInterfaces);
    let grants = [RemoteControlControllerGrant::new(
        controller_identity,
        RemoteControlControllerAuthority::Operator,
        permitted,
    )
    .unwrap()];
    let target_service = RemoteControlService::with_capabilities(
        target_secrets,
        RemoteControlInitialControllerGrants::Grants(
            RemoteControlControllerGrants::try_from(grants.as_slice()).unwrap(),
        ),
        RemoteControlSelfAnnouncement::Unavailable,
        RemoteControlCapabilities::describe_only()
            .with_request(RemoteControlRequestKind::InventoryInterfaces),
    );
    let controller_service = RemoteControlService::new(
        controller_secrets,
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    );
    let server = TcpServer::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap().to_string();
    let target_node = PrnsNode::new_with_handle(|handle| PrnsNodeRecipe {
        transport_identity: None,
        remote_control: RemoteControlNodeSetup::new(target_service)
            .with_controls(InventoryHost(handle)),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let target_handle = target_node.handle();
    let _server = target_handle.supervise(server);
    let (heard, mut announcements) = tokio::sync::mpsc::unbounded_channel();
    let controller_node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: controller_service.into(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _sent = heard.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(TcpClientInterface::new(address));
        },
        persistence: NoPersistence,
    });
    let handle = controller_node.handle();
    let announce = async {
        loop {
            target_handle
                .announce_now(AnnounceNow {
                    destination: target_destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                })
                .await
                .unwrap();
            tokio::time::sleep(core::time::Duration::from_millis(100)).await;
        }
    };
    let exchange = async {
        loop {
            if announcements.recv().await.unwrap() == target_destination {
                break;
            }
        }
        handle
            .set_remote_control_target_access(
                RemoteControlTargetAccess::new(
                    RemoteControlTargetIdentity::new(*target_identity.public_keys()),
                    RemoteControlControllerAuthority::Operator,
                    permitted,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let mut registry = DeviceRegistry::try_new(NonZeroU32::new(1).unwrap()).unwrap();
        let device = paired(&mut registry, target_identity);
        let mut board = DeviceSessionSwitchboard::<16>::new(device);
        let (mut fitting, run) = crate::PrnsDeviceWorker::try_new(
            device,
            handle.clone(),
            core::num::NonZeroUsize::new(2).unwrap(),
        )
        .unwrap();
        let worker = tokio::spawn(run);
        let mut routed = board
            .route(DeviceSessionInput {
                registry: &mut registry,
                message: DeviceSessionMessage::Connect,
            })
            .unwrap();
        let mut commands = alloc::collections::VecDeque::new();
        loop {
            commands.extend(routed.commands.iter_mut().filter_map(Option::take));
            let Some(command) = commands.pop_front() else {
                break;
            };
            let output = perform_worker(&mut fitting, command).await;
            routed = board
                .route(DeviceSessionInput {
                    registry: &mut registry,
                    message: DeviceSessionMessage::Prns(output),
                })
                .unwrap();
        }
        let DeviceSessionEvent::InterfacesReceived {
            outcome: ReceiveInterfacePageOutcome::Complete { count },
            ..
        } = routed.event
        else {
            panic!("inventory did not complete")
        };
        assert!(count > 0);
        let ReadDeviceInterfacesOutcome::Found { inventory, .. } = routed.snapshot.interfaces
        else {
            panic!("inventory missing")
        };
        assert_eq!(inventory.status, InterfaceInventoryStatus::Ready);
        assert!(
            inventory
                .interfaces
                .unwrap()
                .iter()
                .any(|entry| entry.enabled
                    && entry.connection == personal_rns::interfaces::ConnectionState::Connected)
        );
        let disconnected = board
            .route(DeviceSessionInput {
                registry: &mut registry,
                message: DeviceSessionMessage::Disconnect,
            })
            .unwrap();
        let [Some(command), None] = disconnected.commands else {
            panic!("close not dispatched")
        };
        let output = perform_worker(&mut fitting, command).await;
        assert!(matches!(
            output,
            PrnsDeviceOut::Closed {
                settlement: CloseRemoteControlTargetOutcome::Queued,
                ..
            }
        ));
        let closed = board
            .route(DeviceSessionInput {
                registry: &mut registry,
                message: DeviceSessionMessage::Prns(output),
            })
            .unwrap();
        assert_eq!(closed.commands, [None, None]);
        assert_eq!(
            closed.snapshot.interfaces,
            ReadDeviceInterfacesOutcome::Unavailable { device }
        );
        drop(fitting);
        worker.await.unwrap();
    };
    tokio::select! {
        result = tokio::time::timeout(core::time::Duration::from_secs(15), exchange) => result.unwrap(),
        result = target_node.run() => panic!("target stopped: {result:?}"),
        result = controller_node.run() => panic!("controller stopped: {result:?}"),
        () = announce => panic!("announcer stopped"),
    }
}

struct InventoryHost(PrnsNodeHandle);

impl RemoteControlHostControls for InventoryHost {
    fn supported_requests(&self) -> RemoteControlRequestSet {
        RemoteControlRequestSet::only(RemoteControlRequestKind::InventoryInterfaces)
    }

    async fn execute_remote_control(
        &self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
        let RemoteControlHostCommand::InventoryInterfaces { page } = command else {
            return Err(RemoteControlHostCommandError::Unsupported);
        };
        personal_hopspot_core::remote_control_inventory_from_snapshots(&self.0.interfaces(), page)
            .map(RemoteControlHostResponse::InventoryInterfaces)
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
    }
}

async fn perform_worker(
    worker: &mut crate::PrnsDeviceWorker,
    input: PrnsDeviceIn,
) -> PrnsDeviceOut {
    let (mut incoming, mut outgoing) = <_ as DuplexFitting<PrnsDevice>>::split(worker);
    let crate::PrnsDeviceSubmission::Submitted { completion } = outgoing.send(input) else {
        panic!("worker rejected command")
    };
    let ReceiveFromOutcome::Received { output } =
        incoming.receive_from(completion.complete().await)
    else {
        panic!("worker failed")
    };
    output
}
