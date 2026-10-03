#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::tests::usb_observation::{USB, with_observation};
use hopspot_hub_core::{AdvanceUsbPairingDiscoveryError, SelectUsbPairingCandidateOutcome};
use personal_rns::runtime::Diagnostic;
use personal_rns::units::InstantMillis;

#[test]
fn verified_events_and_cloned_handles_share_the_native_clock_and_selection() {
    let clock = TokioClock::start_at(InstantMillis(500));
    let handle = UsbDiscoveryHandle::<2>::new(USB, clock.clone());
    let clone = handle.clone();
    let update = with_observation(1, USB, 500, 60_000, |observation| {
        let NativeUsbDiscoveryEvent::Discovery(result) = handle.receive(PrnsEvent::Message(
            Message::RemoteControlPairingAvailable(observation),
        )) else {
            panic!("availability was not routed")
        };
        result.unwrap()
    });
    let candidate = *update.snapshot.candidates.first().unwrap();
    assert!(update.snapshot.as_of >= InstantMillis(500));
    assert!(update.snapshot.as_of <= clock.now());
    assert_eq!(update.snapshot.interface, USB);
    let selected = std::thread::spawn(move || {
        clone
            .submit(UsbDiscoveryIntent::Select {
                endpoint: candidate.endpoint(),
            })
            .unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(
        selected.event,
        crate::UsbDiscoveryEvent::Selection(SelectUsbPairingCandidateOutcome::Selected {
            candidate
        })
    );
    assert!(selected.snapshot.as_of >= update.snapshot.as_of);
    let clone = handle.clone();
    let cleared = std::thread::spawn(move || clone.submit(UsbDiscoveryIntent::Clear).unwrap())
        .join()
        .unwrap();
    assert!(cleared.snapshot.candidates.is_empty());
    assert!(
        handle
            .submit(UsbDiscoveryIntent::Inspect)
            .unwrap()
            .snapshot
            .candidates
            .is_empty()
    );
}

#[test]
fn unrelated_native_messages_and_diagnostics_remain_available_to_the_caller() {
    let handle = UsbDiscoveryHandle::<1>::new(USB, TokioClock::new());
    let attempt_id = personal_rns::remote_control::RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([1; 32]);
    assert!(
        matches!(handle.receive(PrnsEvent::Message(Message::RemoteControlControllerPairingAuthorizationPersisted { attempt_id })),
        NativeUsbDiscoveryEvent::Prns(PrnsEvent::Message(Message::RemoteControlControllerPairingAuthorizationPersisted { attempt_id: received })) if received == attempt_id)
    );
    let event = PrnsEvent::Diagnostic(Diagnostic::PersistenceRestored {
        routes: 3,
        destination_identities: 4,
        tunnels: 5,
        ratchets: 6,
        refused: 1,
        dropped: 2,
    });
    assert!(matches!(
        handle.receive(event),
        NativeUsbDiscoveryEvent::Prns(PrnsEvent::Diagnostic(Diagnostic::PersistenceRestored {
            routes: 3,
            destination_identities: 4,
            tunnels: 5,
            ratchets: 6,
            refused: 1,
            dropped: 2
        }))
    ));
}

#[test]
fn poison_and_clock_failures_are_reported_without_recovering_uncertain_state() {
    let handle = UsbDiscoveryHandle::<1>::new(USB, TokioClock::new());
    let clone = handle.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = clone.shared.switchboard.lock().unwrap();
            panic!("poisoned state fixture");
        })
        .join()
        .is_err()
    );
    assert_eq!(
        handle.submit(UsbDiscoveryIntent::Inspect),
        Err(UsbDiscoveryFailure::Poisoned)
    );
    with_observation(1, USB, 0, 60_000, |observation| {
        assert!(matches!(
            handle.receive(PrnsEvent::Message(Message::RemoteControlPairingAvailable(
                observation
            ))),
            NativeUsbDiscoveryEvent::Discovery(Err(UsbDiscoveryFailure::Poisoned))
        ));
    });
    let handle = UsbDiscoveryHandle::<1>::new(USB, TokioClock::new());
    *handle.shared.switchboard.lock().unwrap() =
        UsbDiscoverySwitchboard::new(USB, InstantMillis(u64::MAX));
    assert_eq!(
        handle.submit(UsbDiscoveryIntent::Clear),
        Err(UsbDiscoveryFailure::Routing(
            UsbDiscoveryRoutingError::Clock(AdvanceUsbPairingDiscoveryError::TimeWentBackwards {
                current: InstantMillis(u64::MAX)
            })
        ))
    );
}
