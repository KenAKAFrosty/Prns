use super::*;
use crate::{
    DeviceSessionEvent, DeviceSessionIntent, DeviceSessionSubmission, SessionStopReason,
    prepare_device_session, supervise_device_session,
};
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::prelude::*;

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

#[test]
fn native_nodes_authenticate_and_publish_interface_status_through_supervised_execution() {
    within_runtime(async {
        let target_secrets = secrets(0xD0, 0xD1);
        let target_identity =
            RemoteControlTargetIdentity::new(*target_secrets.identities().target().public_keys());
        let target_destination = target_identity.endpoint().destination_hash();
        let controller_secrets = secrets(0xD2, 0xD3);
        let controller_identity = *controller_secrets.identities().controller();
        let permitted =
            RemoteControlRequestSet::only(RemoteControlRequestKind::InventoryInterfaces);
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
                if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event
                {
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
        let mut registry = DeviceRegistry::try_new(NonZeroU32::new(1).unwrap()).unwrap();
        let device = paired(
            &mut registry,
            RemoteControlTargetIdentity::new(*target_identity.public_keys()),
        );
        let (updates, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let session =
            prepare_device_session::<16>(registry, device, handle.clone(), move |update| {
                updates.send(update).unwrap();
            })
            .unwrap();
        let session_handle = session.handle.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let exercise = async {
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
            assert_eq!(
                session_handle.submit(DeviceSessionIntent::Connect).unwrap(),
                DeviceSessionSubmission::Submitted
            );
            loop {
                let update = observed.recv().await.unwrap();
                if let DeviceSessionEvent::InterfacesReceived {
                    outcome: ReceiveInterfacePageOutcome::Complete { count },
                    ..
                } = update.event
                {
                    assert!(count > 0);
                    let ReadDeviceInterfacesOutcome::Found { inventory, .. } =
                        update.snapshot.interfaces
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
                                && entry.connection
                                    == personal_rns::interfaces::ConnectionState::Connected)
                    );
                    break;
                }
            }
        };
        let supervised = supervise_device_session(
            session,
            controller_node.run_until(async {
                stopped.await.unwrap();
            }),
            move || {
                stop.send(()).unwrap();
            },
            exercise,
        );
        tokio::select! {
            exit = supervised => {
                assert_eq!(exit.reason, SessionStopReason::Requested);
                assert_eq!(exit.node, Ok(()));
                assert!(exit.shutdown_wake.is_ok());
                let mut session = exit.session.unwrap();
                assert!(session.settlement.is_ok());
                assert!(matches!(session.registry.step(ReadDevice { device }), ReadDeviceOutcome::Found { device } if matches!(device.connection, hopspot_hub_core::ConnectionState::Disconnected { .. })));
            },
            result = target_node.run() => panic!("target stopped: {result:?}"),
            () = announce => panic!("announcer stopped"),
        }
    });
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
