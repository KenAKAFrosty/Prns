use super::*;

#[test]
#[ignore = "requires the explicitly selected T-Beam Supreme over USB"]
fn attached_tbeam_enrolls_and_reports_live_interfaces() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../local-state");
        let installation = ControllerInstallation::open(&directory).unwrap();
        eprintln!(
            "USB candidates: {:?}",
            personal_rns::usb_auto::scan_native_usb_auto_targets()
        );
        let mut registry = match installation
            .devices
            .load(NonZeroU32::new(8).unwrap())
            .unwrap()
        {
            LoadDevicesOutcome::Missing => {
                DeviceRegistry::try_new(NonZeroU32::new(8).unwrap()).unwrap()
            }
            LoadDevicesOutcome::Loaded { registry } => registry,
        };
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let (discovery, mut discoveries) = tokio::sync::mpsc::channel(8);
        let (announce, _announcements) = tokio::sync::mpsc::channel(16);
        let prepared = prepare_native_usb_enrollment::<8>(
            installation,
            move |event| match event {
                NativeUsbDiscoveryEvent::Discovery(update) => {
                    discovery.try_send(update.unwrap()).unwrap();
                }
                NativeUsbDiscoveryEvent::Prns(PrnsEvent::Diagnostic(
                    Diagnostic::AnnounceHeard { destination, .. },
                )) => {
                    announce.try_send(destination).unwrap();
                }
                NativeUsbDiscoveryEvent::Prns(PrnsEvent::Diagnostic(diagnostic)) => {
                    eprintln!("USB diagnostic: {diagnostic:?}");
                }
                NativeUsbDiscoveryEvent::Prns(PrnsEvent::Message(_)) => {}
            },
            async {
                let _stopped = stopped.await;
            },
        );
        let native = prepared.discovery.native;
        let exercise = async {
            let ListDevicesOutcome::Listed { devices } = registry.step(ListDevices::<8>) else {
                panic!("capacity");
            };
            let mut existing = None;
            for device in devices {
                let ReadDeviceOutcome::Found { device: snapshot } =
                    registry.step(ReadDevice { device })
                else {
                    panic!("missing");
                };
                if matches!(snapshot.enrollment, EnrollmentState::Paired { .. }) {
                    assert!(
                        existing.is_none(),
                        "the physical test requires exactly one remembered device"
                    );
                    existing = Some(device);
                }
            }
            let (mut registry, store, device) = if let Some(device) = existing {
                (registry, native.devices, device)
            } else {
                eprintln!("Waiting for signed USB first-owner availability");
                let candidate = loop {
                    let update = tokio::select! {
                        update = discoveries.recv() => update.unwrap(),
                        () = tokio::time::sleep(Duration::from_secs(5)) => {
                            eprintln!("USB interfaces: {:?}", native.handle.interfaces());
                            continue;
                        }
                    };
                    let direct = update
                        .snapshot
                        .candidates
                        .iter()
                        .filter(|candidate| {
                            candidate.kind()
                                == RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable
                        })
                        .copied()
                        .collect::<alloc::vec::Vec<_>>();
                    assert!(
                        direct.len() <= 1,
                        "multiple direct enrollment devices require explicit selection"
                    );
                    if let Some(candidate) = direct.first() {
                        break *candidate;
                    }
                };
                eprintln!("Selected USB endpoint {:?}", candidate.endpoint());
                let CreateDeviceOutcome::Created { device } = registry
                    .step(CreateDevice {
                        label: DeviceLabel::new("T-Beam Supreme").unwrap(),
                    })
                    .unwrap()
                else {
                    panic!("capacity");
                };
                let work = UsbEnrollmentWork {
                    registry,
                    store: native.devices,
                    events: prepared.events,
                    candidate,
                    device,
                    controller: *native.identities.controller(),
                };
                let exit = work
                    .run::<8>(native.handle.clone(), native.clock.clone())
                    .await
                    .unwrap();
                assert_eq!(
                    exit.outcome.unwrap(),
                    UsbEnrollmentOutcome::Ready { device }
                );
                eprintln!("USB enrollment durable at {}", directory.display());
                (exit.registry, exit.store, device)
            };
            let ReadDeviceOutcome::Found { device: snapshot } =
                registry.step(ReadDevice { device })
            else {
                panic!("missing");
            };
            let EnrollmentState::Paired { .. } = snapshot.enrollment else {
                panic!("not paired");
            };
            tokio::time::sleep(Duration::from_secs(5)).await;
            let (updates, mut received) = tokio::sync::mpsc::unbounded_channel();
            let session = prepare_device_session::<16>(
                registry,
                device,
                native.handle.clone(),
                move |update| {
                    let _sent = updates.send(update);
                },
            )
            .unwrap();
            assert_eq!(
                session.handle.submit(DeviceSessionIntent::Connect).unwrap(),
                DeviceSessionSubmission::Submitted
            );
            let inspect = async {
                while let Some(update) = received.recv().await {
                    eprintln!("Session event: {:?}", update.event);
                    if let ReadDeviceInterfacesOutcome::Found { inventory, .. } =
                        update.snapshot.interfaces
                        && inventory.status == InterfaceInventoryStatus::Ready
                    {
                        let interfaces = inventory.interfaces.unwrap();
                        eprintln!("Live T-Beam interfaces: {interfaces:#?}");
                        assert!(!interfaces.is_empty());
                        session.handle.shutdown().unwrap();
                        return;
                    }
                }
                panic!("session stopped before inventory");
            };
            let (exit, ()) = tokio::join!(session.run, inspect);
            assert!(exit.unwrap().settlement.is_ok());
            drop(store);
            stop.send(()).unwrap();
        };
        let run = async {
            let (node, ()) = tokio::join!(native.run, exercise);
            node.unwrap();
        };
        tokio::time::timeout(Duration::from_secs(120), run)
            .await
            .unwrap();
    });
}
