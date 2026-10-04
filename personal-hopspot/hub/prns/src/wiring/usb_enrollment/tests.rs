#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::tests::within_runtime;
use crate::{
    DeviceSessionIntent, DeviceSessionSubmission, LoadDevicesOutcome, prepare_device_session,
};
use core::{num::NonZeroU32, time::Duration};
use personal_rns::prelude::*;
use personal_rns::remote_control::*;
use personal_rns::units::DurationMillis;

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

#[test]
fn direct_enrollment_persists_both_authorizations_and_hands_off_to_live_interface_inventory() {
    within_runtime(async {
        let target_dir = tempfile::tempdir().unwrap();
        let hub_dir = tempfile::tempdir().unwrap();
        let target_secrets = RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(
                personal_rns::identity::vault::IdentitySecretKey::new([70; 64]),
            ),
            RemoteControlTargetIdentitySecret::from(
                personal_rns::identity::vault::IdentitySecretKey::new([71; 64]),
            ),
        )
        .unwrap();
        let target_identity =
            RemoteControlTargetIdentity::new(*target_secrets.identities().target().public_keys());
        let service = RemoteControlService::with_capabilities(
            target_secrets,
            RemoteControlInitialControllerGrants::Nobody,
            RemoteControlSelfAnnouncement::Unavailable,
            RemoteControlCapabilities::describe_only()
                .with_request(RemoteControlRequestKind::InventoryInterfaces),
        );

        let (persisted, mut persistences) = tokio::sync::mpsc::channel(1);
        let (confirm, mut confirmations) = tokio::sync::mpsc::channel(1);
        let (target_stop, target_stopped) = tokio::sync::oneshot::channel();
        let target = PrnsNode::new_with_handle(|handle| PrnsNodeRecipe {
            transport_identity: None,
            remote_control: RemoteControlNodeSetup::new(service)
                .with_controls(InventoryHost(handle)),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: NoRemoteControlHostControls,
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: move |event, _| match event {
                PrnsEvent::Message(Message::RemoteControlTargetPairingConfirmationRequired(
                    confirmation,
                )) => confirm.try_send(confirmation).unwrap(),
                PrnsEvent::Message(Message::RemoteControlTargetPairingAuthorizationPersisted {
                    attempt_id,
                }) => persisted.try_send(attempt_id).unwrap(),
                PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
            },
            interfaces: ManuallyAttached,
            persistence: NodePersistence::custom_dir(target_dir.path()).unwrap(),
        });
        let target_handle = target.handle();
        let server = TcpServer::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap().to_string();
        let _server = target_handle.supervise(server);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let (discover, mut discovered) = tokio::sync::mpsc::channel(4);
        let (announce, _announced) = tokio::sync::mpsc::channel(4);
        let prepared = prepare_enrollment::<4>(
            ControllerInstallation::open(hub_dir.path()).unwrap(),
            move |handle| handle.attach(TcpClientInterface::new(address)),
            alloc::sync::Arc::new(tokio::sync::Notify::new()),
            move |event| match event {
                NativeUsbDiscoveryEvent::Discovery(update) => {
                    discover.try_send(update.unwrap()).unwrap()
                }
                NativeUsbDiscoveryEvent::Prns(PrnsEvent::Diagnostic(
                    Diagnostic::AnnounceHeard { destination, .. },
                )) => {
                    announce.try_send(destination).unwrap();
                }
                NativeUsbDiscoveryEvent::Prns(_) => {}
            },
            async {
                let _stopped = stopped.await;
            },
        );
        let native = prepared.discovery.native;
        let controller = *native.identities.controller();
        let exercise = async {
            while !target_handle.interfaces().iter().any(|interface| {
                interface.connection.is_online()
                    && interface.id.kind()
                        == Some(personal_rns::interfaces::InterfaceKind::TcpServerPeer)
            }) || !native
                .handle
                .interfaces()
                .iter()
                .any(|interface| interface.connection.is_online())
            {
                tokio::task::yield_now().await;
            }
            let interface = target_handle
                .interfaces()
                .into_iter()
                .find(|interface| {
                    interface.connection.is_online()
                        && interface.id.kind()
                            == Some(personal_rns::interfaces::InterfaceKind::TcpServerPeer)
                })
                .unwrap()
                .id;
            let opened = target_handle
                .open_remote_control_pairing(personal_rns::engine::OpenRemoteControlPairing {
                    admission: RemoteControlPairingAdmissionMode::DirectPhysical,
                    target: personal_rns::engine::AnnounceTarget::Interface(interface),
                    expires_after: RemoteControlPairingExpiresAfter::try_from(DurationMillis(
                        60_000,
                    ))
                    .unwrap(),
                    attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(DurationMillis(
                        30_000,
                    ))
                    .unwrap(),
                    permissions: RemoteControlPairingPermissions::new(
                        RemoteControlControllerAuthority::Administrator,
                        RemoteControlRequestSet::only(
                            RemoteControlRequestKind::InventoryInterfaces,
                        ),
                    )
                    .unwrap(),
                    public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
                        b"Test T-Beam".as_slice(),
                    )
                    .unwrap(),
                })
                .await
                .unwrap();
            assert_eq!(opened.invitation_code, None);
            let update = discovered.recv().await.unwrap();
            let candidate = *update.snapshot.candidates.first().unwrap();
            assert_eq!(
                candidate.kind(),
                RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable
            );
            let mut registry = DeviceRegistry::try_new(NonZeroU32::new(4).unwrap()).unwrap();
            let CreateDeviceOutcome::Created { device } = registry
                .step(CreateDevice {
                    label: DeviceLabel::new("T-Beam").unwrap(),
                })
                .unwrap()
            else {
                panic!("create");
            };
            let work = UsbEnrollmentWork {
                registry,
                store: native.devices,
                events: prepared.events,
                candidate,
                device,
                controller,
            };
            let target_approve = async {
                let confirmation = confirmations.recv().await.unwrap();
                assert_eq!(
                    confirmation.confirmation().context().endpoint(),
                    opened.endpoint
                );
                let approval = target_handle
                    .approve_remote_control_target_pairing(confirmation.approval())
                    .await
                    .unwrap();
                assert_eq!(
                    persistences.recv().await.unwrap(),
                    confirmation.confirmation().attempt_id()
                );
                approval
            };
            let (exit, _approved) = tokio::join!(
                work.run::<4>(native.handle.clone(), native.clock.clone()),
                target_approve
            );
            let mut exit = exit.unwrap();
            assert_eq!(
                exit.outcome.unwrap(),
                UsbEnrollmentOutcome::Ready { device }
            );
            let LoadDevicesOutcome::Loaded {
                registry: mut restored,
            } = exit.store.load(NonZeroU32::new(4).unwrap()).unwrap()
            else {
                panic!("missing archive");
            };
            assert_eq!(
                restored.step(ReadDevice { device }),
                exit.registry.step(ReadDevice { device })
            );
            assert_eq!(
                exit.coordinator.as_mut().unwrap().inspect().status,
                EnrollmentSettlementStatus::Recorded
            );
            let target_record = exit.registry.step(ReadDevice { device });
            let ReadDeviceOutcome::Found { device: snapshot } = target_record else {
                panic!("missing record");
            };
            assert_eq!(
                snapshot.enrollment,
                EnrollmentState::Paired {
                    target: *target_identity.public_keys()
                }
            );
            let (updates, mut updated) = tokio::sync::mpsc::unbounded_channel();
            let session = prepare_device_session::<16>(
                exit.registry,
                device,
                native.handle.clone(),
                move |update| {
                    updates.send(update).unwrap();
                },
            )
            .unwrap();
            assert_eq!(
                session.handle.submit(DeviceSessionIntent::Connect).unwrap(),
                DeviceSessionSubmission::Submitted
            );
            let inspect = async {
                loop {
                    let update = updated.recv().await.unwrap();
                    if let ReadDeviceInterfacesOutcome::Found { inventory, .. } =
                        update.snapshot.interfaces
                        && inventory.status == InterfaceInventoryStatus::Ready
                    {
                        assert!(!inventory.interfaces.unwrap().is_empty());
                        session.handle.shutdown().unwrap();
                        break;
                    }
                }
            };
            let (session_exit, ()) = tokio::join!(session.run, inspect);
            assert!(session_exit.unwrap().settlement.is_ok());
            stop.send(()).unwrap();
            target_stop.send(()).unwrap();
        };
        let all = async {
            let (hub_result, (), ()) = tokio::join!(
                native.run,
                async {
                    target
                        .run_until(async {
                            let _stop = target_stopped.await;
                        })
                        .await
                        .unwrap();
                },
                exercise
            );
            hub_result.unwrap();
        };
        tokio::time::timeout(Duration::from_secs(15), all)
            .await
            .unwrap();
    });
}

mod hardware;

mod failures;
