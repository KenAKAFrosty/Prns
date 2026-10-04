#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::tests::usb_observation::availability_wire;
use crate::{UsbDiscoveryEvent, UsbDiscoveryIntent};
use hopspot_hub_core::{ObserveUsbPairingAvailabilityOutcome, SelectUsbPairingCandidateOutcome};
use personal_rns::interfaces::rns_serial_framing;
use personal_rns::units::InstantMillis;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

#[test]
fn native_preparation_initializes_discovery_from_the_attached_usb_and_persisted_clock() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = prepare_native_usb_discovery::<2>(
        ControllerInstallation::open(directory.path()).unwrap(),
        |_| panic!("preparation cannot emit events"),
        core::future::pending(),
    );
    let inspected = runtime
        .discovery
        .submit(UsbDiscoveryIntent::Inspect)
        .unwrap();
    assert_eq!(
        inspected.snapshot.interface,
        runtime.native.usb_interface.id()
    );
    assert_eq!(inspected.event, UsbDiscoveryEvent::Inspected);
    assert!(inspected.snapshot.candidates.is_empty());
    assert!(inspected.snapshot.as_of > InstantMillis(1_000_000));
    assert!(inspected.snapshot.as_of <= runtime.native.clock.now());
    drop(runtime.native.run);
    drop(runtime.native.devices);
    assert!(ControllerInstallation::open(directory.path()).is_ok());
}

#[test]
fn signed_native_ingress_routes_discovery_and_callbacks_can_inspect_without_deadlock() {
    crate::tests::within_runtime(async {
        let directory = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let rescan = Arc::new(Notify::new());
        let (updates, mut observed) = mpsc::unbounded_channel();
        let (stop, stopped) = oneshot::channel();
        let reentrant = Arc::new(OnceLock::<UsbDiscoveryHandle<2>>::new());
        let callback_handle = reentrant.clone();
        let runtime = prepare_discovery::<2>(
            ControllerInstallation::open(directory.path()).unwrap(),
            move |handle| handle.attach(TcpClientInterface::new(address)),
            rescan.clone(),
            move |event| match event {
                NativeUsbDiscoveryEvent::Discovery(result) => {
                    let update = result.unwrap();
                    let inspect = callback_handle
                        .get()
                        .unwrap()
                        .submit(UsbDiscoveryIntent::Inspect)
                        .unwrap();
                    assert_eq!(inspect.snapshot.candidates, update.snapshot.candidates);
                    updates.send(update).unwrap();
                }
                NativeUsbDiscoveryEvent::Prns(_) => {}
            },
            async {
                stopped.await.unwrap();
            },
        );
        reentrant.get_or_init(|| runtime.discovery.clone());
        assert!(Arc::ptr_eq(&runtime.native.usb_rescan, &rescan));
        let interface = runtime.native.usb_interface.id();
        let native_clock = runtime.native.clock.clone();
        let exercise = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            for seed in [1, 2] {
                let wire = availability_wire(seed, native_clock.now().0, 60_000);
                let mut framed = alloc::vec![0; rns_serial_framing::max_encoded_len(wire.len())];
                let length = rns_serial_framing::encode(&wire, &mut framed).unwrap();
                stream
                    .write_all(framed.get(..length).unwrap())
                    .await
                    .unwrap();
                let update = observed.recv().await.unwrap();
                assert_eq!(update.expired, 0);
                let UsbDiscoveryEvent::Availability(ObserveUsbPairingAvailabilityOutcome::Added {
                    candidate,
                }) = update.event
                else {
                    panic!("verified packet was not added")
                };
                assert_eq!(candidate.source_interface(), interface);
                assert_eq!(update.snapshot.interface, interface);
                assert_eq!(update.snapshot.candidates.len(), usize::from(seed));
                assert!(candidate.observed_at() <= update.snapshot.as_of);
                assert!(update.snapshot.as_of <= native_clock.now());
                assert_eq!(
                    candidate
                        .expires_at()
                        .0
                        .saturating_sub(candidate.observed_at().0),
                    60_000
                );
                let selection = runtime
                    .discovery
                    .submit(UsbDiscoveryIntent::Select {
                        endpoint: candidate.endpoint(),
                    })
                    .unwrap();
                assert_eq!(
                    selection.event,
                    UsbDiscoveryEvent::Selection(SelectUsbPairingCandidateOutcome::Selected {
                        candidate
                    })
                );
            }
            stop.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(runtime.native.run, exercise);
        assert_eq!(result, Ok(()));
        drop(runtime.native.devices);
        assert!(ControllerInstallation::open(directory.path()).is_ok());
    });
}
