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
fn native_tcp_nodes_authenticate_report_inventory_and_execute_controls() {
    within_runtime(async {
        let target_secrets = secrets(0xD0, 0xD1);
        let target_identity =
            RemoteControlTargetIdentity::new(*target_secrets.identities().target().public_keys());
        let target_destination = target_identity.endpoint().destination_hash();
        let controller_secrets = secrets(0xD2, 0xD3);
        let controller_identity = *controller_secrets.identities().controller();
        let permitted = control_permissions();
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
                .with_request(RemoteControlRequestKind::InventoryInterfaces)
                .with_request(RemoteControlRequestKind::SetDisplayVisibility)
                .with_request(RemoteControlRequestKind::SetGnssPower),
        );
        let controller_service = RemoteControlService::new(
            controller_secrets,
            RemoteControlInitialControllerGrants::Nobody,
            RemoteControlSelfAnnouncement::Unavailable,
        );
        let server = TcpServer::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap().to_string();
        let commands = Arc::new(Mutex::new(alloc::vec::Vec::new()));
        let recorded = commands.clone();
        let target_node = PrnsNode::new_with_handle(|handle| PrnsNodeRecipe {
            transport_identity: None,
            remote_control: RemoteControlNodeSetup::new(target_service).with_controls(
                InventoryHost {
                    handle,
                    commands: recorded,
                },
            ),
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
            for command in [
                DeviceControlCommand::DisplayVisibility(RemoteControlDisplayVisibility::Hidden),
                DeviceControlCommand::DisplayVisibility(RemoteControlDisplayVisibility::Visible),
                DeviceControlCommand::GnssPower(RemoteControlGnssPower::On),
                DeviceControlCommand::GnssPower(RemoteControlGnssPower::Off),
            ] {
                assert_eq!(
                    session_handle
                        .submit(DeviceSessionIntent::Control(RequestDeviceControl {
                            command
                        }))
                        .unwrap(),
                    DeviceSessionSubmission::Submitted
                );
                loop {
                    let update = observed.recv().await.unwrap();
                    if let DeviceSessionEvent::ControlSettled { settlement, output } = update.event
                    {
                        let PrnsDeviceOut::ControlAcknowledged {
                            request, outcome, ..
                        } = output
                        else {
                            panic!("control not acknowledged: {output:?}")
                        };
                        assert_eq!(request.command(), command);
                        assert_eq!(outcome, RemoteControlApplyOutcome::Applied);
                        assert_eq!(settlement, SettleDeviceControlOutcome::Settled { request });
                        assert_eq!(commands.lock().unwrap().last(), Some(&command));
                        assert_eq!(update.snapshot.controls.pending, None);
                        break;
                    }
                }
            }
            assert_eq!(commands.lock().unwrap().len(), 4);
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

struct InventoryHost {
    handle: PrnsNodeHandle,
    commands: Arc<Mutex<alloc::vec::Vec<DeviceControlCommand>>>,
}

impl RemoteControlHostControls for InventoryHost {
    fn supported_requests(&self) -> RemoteControlRequestSet {
        control_permissions()
    }

    async fn execute_remote_control(
        &self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
        match command {
            RemoteControlHostCommand::InventoryInterfaces { page } => {
                personal_hopspot_core::remote_control_inventory_from_snapshots(
                    &self.handle.interfaces(),
                    page,
                )
                .map(RemoteControlHostResponse::InventoryInterfaces)
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            RemoteControlHostCommand::SetDisplayVisibility { visibility } => {
                self.commands
                    .lock()
                    .unwrap()
                    .push(DeviceControlCommand::DisplayVisibility(visibility));
                Ok(RemoteControlHostResponse::SetDisplayVisibility(
                    RemoteControlApplyOutcome::Applied,
                ))
            }
            RemoteControlHostCommand::SetGnssPower { power } => {
                self.commands
                    .lock()
                    .unwrap()
                    .push(DeviceControlCommand::GnssPower(power));
                Ok(RemoteControlHostResponse::SetGnssPower(
                    RemoteControlApplyOutcome::Applied,
                ))
            }
            RemoteControlHostCommand::SetInterfacePower { .. }
            | RemoteControlHostCommand::SetInterfaceMode { .. }
            | RemoteControlHostCommand::SetInterfaceGroup { .. }
            | RemoteControlHostCommand::InventoryInterfaceDiscoveryGroups { .. }
            | RemoteControlHostCommand::ReplaceInterfaceDiscoveryGroups { .. }
            | RemoteControlHostCommand::InventoryInterfacePeers { .. }
            | RemoteControlHostCommand::InventoryInterfaceConfig { .. }
            | RemoteControlHostCommand::SetInterfaceLoRaProfile { .. }
            | RemoteControlHostCommand::DescribeBuild
            | RemoteControlHostCommand::DescribePower
            | RemoteControlHostCommand::SleepRadios
            | RemoteControlHostCommand::WakeRadios
            | RemoteControlHostCommand::SetSystemPower { .. }
            | RemoteControlHostCommand::SetDisplayAutoOff { .. }
            | RemoteControlHostCommand::SetEspRadioMode { .. } => {
                Err(RemoteControlHostCommandError::Unsupported)
            }
        }
    }
}

fn control_permissions() -> RemoteControlRequestSet {
    let mut permitted =
        RemoteControlRequestSet::only(RemoteControlRequestKind::InventoryInterfaces);
    assert!(permitted.insert(RemoteControlRequestKind::SetDisplayVisibility));
    assert!(permitted.insert(RemoteControlRequestKind::SetGnssPower));
    permitted
}
