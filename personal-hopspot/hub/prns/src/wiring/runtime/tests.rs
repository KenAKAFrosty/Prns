#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::ControllerInstallationError;
use alloc::rc::Rc;
use core::cell::RefCell;
use core::time::Duration;
use personal_rns::identity::vault::IdentityOrigin;
use personal_rns::interfaces::ConnectionState;
use personal_rns::remote_control::RemoteControlControllerAuthority;
use personal_rns::usb_auto::DEFAULT_USB_AUTO_ID;
use tokio::sync::{mpsc, oneshot};

async fn no_port(
    _: personal_rns::usb_auto::UsbAutoCandidate,
) -> std::io::Result<tokio::io::DuplexStream> {
    panic!("an empty scanner cannot open a port")
}

fn attach_controlled_usb(
    handle: &PrnsNodeHandle,
    rescan: Arc<Notify>,
    scans: mpsc::UnboundedSender<()>,
) -> AttachedInterface {
    handle.add_interface(UsbAutoHost::new(
        DEFAULT_USB_AUTO_ID,
        move || {
            let _sent = scans.send(());
            alloc::vec::Vec::new()
        },
        no_port,
        rescan,
    ))
}

fn assert_locked(path: &std::path::Path) {
    assert!(matches!(
        ControllerInstallation::open(path),
        Err(ControllerInstallationError::Lock(
            std::fs::TryLockError::WouldBlock
        ))
    ));
}

#[tokio::test]
async fn native_preparation_attaches_usb_without_running_and_owns_the_lock() {
    let directory = tempfile::tempdir().unwrap();
    let installation = ControllerInstallation::open(directory.path()).unwrap();
    let expected = installation.identity.secrets().identities();
    let runtime = prepare_native_hub(
        installation,
        |_, _| panic!("unpolled runtime emitted an event"),
        core::future::pending(),
    );
    assert_eq!(runtime.identities, expected);
    assert_eq!(
        runtime.identity_origins.controller(),
        IdentityOrigin::Generated
    );
    assert_eq!(runtime.identity_origins.target(), IdentityOrigin::Generated);
    assert_eq!(runtime.usb_interface.id(), DEFAULT_USB_AUTO_ID);
    let interfaces = runtime.handle.interfaces();
    assert_eq!(interfaces.len(), 1);
    assert_eq!(interfaces.first().unwrap().id, DEFAULT_USB_AUTO_ID);
    assert_eq!(
        interfaces.first().unwrap().connection,
        ConnectionState::Initializing
    );
    assert_locked(directory.path());
    drop(runtime.run);
    assert_locked(directory.path());
    drop(runtime.devices);
    let reopened = ControllerInstallation::open(directory.path()).unwrap();
    assert_eq!(reopened.identity.secrets().identities(), expected);
    let runtime = prepare_native_hub(reopened, |_, _| {}, core::future::pending());
    drop(runtime.devices);
    assert_locked(directory.path());
    drop(runtime.run);
    assert!(ControllerInstallation::open(directory.path()).is_ok());
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Restored,
    Flushed,
    Failed,
}

#[tokio::test]
async fn usb_rescans_and_persisted_authorization_survive_a_graceful_restart() {
    let directory = tempfile::tempdir().unwrap();
    let target = crate::tests::target(72);
    let target_hash = target.identity_hash();
    let permitted = RemoteControlRequestSet::only(RemoteControlRequestKind::InventoryInterfaces);
    let events = Rc::new(RefCell::new(alloc::vec::Vec::new()));
    let mut original_controller = None;
    for restart in [false, true] {
        let installation = ControllerInstallation::open(directory.path()).unwrap();
        let (scan_tx, mut scans) = mpsc::unbounded_channel();
        let rescan = Arc::new(Notify::new());
        let usb_rescan = rescan.clone();
        let observed = events.clone();
        let path = directory.path();
        let (stop, stopped) = oneshot::channel();
        let mut runtime = prepare_hub(
            installation,
            move |handle| attach_controlled_usb(handle, usb_rescan, scan_tx),
            rescan.clone(),
            move |event, _| {
                assert_locked(path);
                match event {
                    PrnsEvent::Diagnostic(Diagnostic::PersistenceRestored {
                        refused,
                        dropped,
                        ..
                    }) => {
                        assert_eq!((refused, dropped), (0, 0));
                        observed.borrow_mut().push(Event::Restored);
                    }
                    PrnsEvent::Diagnostic(Diagnostic::PersistenceFlushed { .. }) => {
                        observed.borrow_mut().push(Event::Flushed)
                    }
                    PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
                }
            },
            async {
                stopped.await.unwrap();
            },
        );
        assert!(Arc::ptr_eq(&runtime.usb_rescan, &rescan));
        assert_eq!(runtime.usb_interface.id(), DEFAULT_USB_AUTO_ID);
        let controller = *runtime.identities.controller();
        if let Some(original) = original_controller {
            assert_eq!(controller, original);
            assert_eq!(
                runtime.identity_origins.controller(),
                IdentityOrigin::Loaded
            );
        } else {
            original_controller = Some(controller);
        }
        let exercise = async {
            scans.recv().await.unwrap();
            runtime.usb_rescan.notify_one();
            scans.recv().await.unwrap();
            assert!(
                runtime
                    .handle
                    .interfaces()
                    .iter()
                    .any(|interface| interface.id == DEFAULT_USB_AUTO_ID)
            );
            if !restart {
                assert!(
                    runtime
                        .handle
                        .remote_control_target_inventory()
                        .await
                        .unwrap()
                        .is_empty()
                );
                assert_eq!(
                    runtime
                        .handle
                        .set_remote_control_target_access(
                            RemoteControlTargetAccess::new(
                                crate::tests::target(72),
                                RemoteControlControllerAuthority::Operator,
                                permitted,
                            )
                            .unwrap()
                        )
                        .await,
                    Ok(SetRemoteControlTargetAccessOutcome::Added)
                );
            }
            use hopspot_hub_core::{
                BeginConnection, BeginConnectionOutcome, DeviceRegistry, ListDevices,
                ListDevicesOutcome,
            };
            use pipecircuit::StateMachine;
            let mut records = if restart {
                let crate::LoadDevicesOutcome::Loaded { registry } =
                    runtime.devices.load(core::num::NonZeroU32::MIN).unwrap()
                else {
                    panic!("remembered device missing after restart")
                };
                registry
            } else {
                let mut records = DeviceRegistry::try_new(core::num::NonZeroU32::MIN).unwrap();
                crate::tests::paired(&mut records, crate::tests::target(72));
                assert_eq!(
                    runtime.devices.save::<1>(&mut records).unwrap(),
                    crate::SaveDevicesOutcome::Saved
                );
                records
            };
            let ListDevicesOutcome::Listed { devices } = records.step(ListDevices::<1>) else {
                panic!("record listing refused")
            };
            let BeginConnectionOutcome::Connect { connection } = records
                .step(BeginConnection {
                    device: *devices.first().unwrap(),
                })
                .unwrap()
            else {
                panic!("remembered device cannot connect")
            };
            assert_eq!(connection.target().identity_hash(), target_hash);
            let resolved = runtime
                .handle
                .resolve_remote_control_target(target_hash)
                .await
                .unwrap();
            assert_eq!(resolved.controller(), &controller);
            assert_eq!(resolved.target(), target_hash);
            assert_eq!(resolved.endpoint(), target.endpoint());
            assert_eq!(resolved.permitted_requests(), &permitted);
            assert_eq!(
                runtime
                    .handle
                    .remote_control_target_inventory()
                    .await
                    .unwrap()
                    .len(),
                1
            );
            assert_locked(directory.path());
            stop.send(()).unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(runtime.run, exercise)
        })
        .await
        .unwrap();
        assert_eq!(result, Ok(()));
        assert_locked(directory.path());
        drop(runtime.devices);
        assert_eq!(events.borrow().first(), Some(&Event::Restored));
        assert!(
            events
                .borrow()
                .iter()
                .filter(|event| **event == Event::Flushed)
                .count()
                >= 2
        );
        events.borrow_mut().clear();
    }
    assert!(ControllerInstallation::open(directory.path()).is_ok());
}

#[tokio::test]
async fn terminal_persistence_failure_is_reported_before_releasing_the_lock() {
    let directory = tempfile::tempdir().unwrap();
    let installation = ControllerInstallation::open(directory.path()).unwrap();
    let (scan_tx, _scans) = mpsc::unbounded_channel();
    let rescan = Arc::new(Notify::new());
    let events = Rc::new(RefCell::new(alloc::vec::Vec::new()));
    let observed = events.clone();
    let path = directory.path();
    let runtime = prepare_hub(
        installation,
        |handle| attach_controlled_usb(handle, rescan.clone(), scan_tx),
        rescan.clone(),
        move |event, _| {
            assert_locked(path);
            if matches!(
                event,
                PrnsEvent::Diagnostic(Diagnostic::PersistenceFlushFailed { .. })
            ) {
                observed.borrow_mut().push(Event::Failed);
            }
        },
        async {},
    );
    std::fs::remove_dir_all(directory.path().join("retained")).unwrap();
    std::fs::write(directory.path().join("retained"), b"blocked").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), runtime.run)
        .await
        .unwrap();
    assert_eq!(result, Err(NodeRunError::PersistenceFailed));
    assert_locked(directory.path());
    drop(runtime.devices);
    assert!(events.borrow().contains(&Event::Failed));
    std::fs::remove_file(directory.path().join("retained")).unwrap();
    assert!(ControllerInstallation::open(directory.path()).is_ok());
}
