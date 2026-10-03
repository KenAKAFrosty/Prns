#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::tests::usb_observation::{OTHER, USB, with_observation};
use proptest::prelude::*;

fn intent<const N: usize>(
    board: &mut UsbDiscoverySwitchboard<N>,
    now: u64,
    intent: UsbDiscoveryIntent,
) -> Result<UsbDiscoveryUpdate<N>, UsbDiscoveryRoutingError> {
    board.route(UsbDiscoveryInput {
        now: InstantMillis(now),
        message: UsbDiscoveryMessage::Intent(intent),
    })
}

fn observe<const N: usize>(
    board: &mut UsbDiscoverySwitchboard<N>,
    now: u64,
    seed: u8,
    source: InterfaceId,
    observed: u64,
    lifetime: u64,
) -> Result<UsbDiscoveryUpdate<N>, UsbDiscoveryRoutingError> {
    with_observation(seed, source, observed, lifetime, |observation| {
        board.route(UsbDiscoveryInput {
            now: InstantMillis(now),
            message: UsbDiscoveryMessage::Available(observation),
        })
    })
}

#[test]
fn routes_verified_availability_and_fresh_selection_with_owned_snapshots() {
    let mut board = UsbDiscoverySwitchboard::<2>::new(USB, InstantMillis(100));
    let first = observe(&mut board, 101, 1, USB, 100, 10).unwrap();
    let candidate = *first.snapshot.candidates.first().unwrap();
    assert_eq!(first.expired, 0);
    assert_eq!(first.snapshot.interface, USB);
    assert_eq!(first.snapshot.as_of, InstantMillis(101));
    assert_eq!(
        first.event,
        UsbDiscoveryEvent::Availability(ObserveUsbPairingAvailabilityOutcome::Added { candidate })
    );
    let second = observe(&mut board, 102, 2, USB, 102, 20).unwrap();
    assert_eq!(second.snapshot.candidates.len(), 2);
    let selected = intent(
        &mut board,
        103,
        UsbDiscoveryIntent::Select {
            endpoint: candidate.endpoint(),
        },
    )
    .unwrap();
    assert_eq!(
        selected.event,
        UsbDiscoveryEvent::Selection(SelectUsbPairingCandidateOutcome::Selected { candidate })
    );
    assert_eq!(selected.snapshot.candidates, second.snapshot.candidates);
    let expired = intent(
        &mut board,
        110,
        UsbDiscoveryIntent::Select {
            endpoint: candidate.endpoint(),
        },
    )
    .unwrap();
    assert_eq!(expired.expired, 1);
    assert_eq!(
        expired.event,
        UsbDiscoveryEvent::Selection(SelectUsbPairingCandidateOutcome::Unavailable)
    );
    assert_eq!(expired.snapshot.candidates.len(), 1);
    assert_eq!(expired.snapshot.as_of, InstantMillis(110));
    assert_eq!(first.snapshot.candidates.as_slice(), &[candidate]);
    let cleared = intent(&mut board, 120, UsbDiscoveryIntent::Clear).unwrap();
    assert_eq!(
        cleared.event,
        UsbDiscoveryEvent::Cleared(ClearUsbPairingCandidatesOutcome { removed: 1 })
    );
    assert_eq!(cleared.expired, 0);
    assert!(cleared.snapshot.candidates.is_empty());
    let empty = intent(&mut board, 121, UsbDiscoveryIntent::Inspect).unwrap();
    assert_eq!(empty.event, UsbDiscoveryEvent::Inspected);
    assert_eq!(empty.snapshot.as_of, InstantMillis(121));
    assert!(empty.snapshot.candidates.is_empty());
}

#[test]
fn expiry_precedes_admission_and_clear_and_domain_refusals_remain_flat() {
    let mut board = UsbDiscoverySwitchboard::<1>::new(USB, InstantMillis(10));
    let first = observe(&mut board, 10, 1, USB, 10, 10).unwrap();
    assert_eq!(
        observe(&mut board, 11, 2, USB, 11, 20).unwrap().event,
        UsbDiscoveryEvent::Availability(ObserveUsbPairingAvailabilityOutcome::CapacityExceeded {
            maximum: 1
        })
    );
    assert_eq!(
        observe(&mut board, 12, 1, USB, 10, 30).unwrap().event,
        UsbDiscoveryEvent::Availability(ObserveUsbPairingAvailabilityOutcome::StaleObservation)
    );
    assert_eq!(
        observe(&mut board, 13, 2, OTHER, 13, 20).unwrap().event,
        UsbDiscoveryEvent::Availability(ObserveUsbPairingAvailabilityOutcome::WrongInterface)
    );
    assert_eq!(
        observe(&mut board, 14, 2, USB, 10, 4).unwrap().event,
        UsbDiscoveryEvent::Availability(ObserveUsbPairingAvailabilityOutcome::Expired)
    );
    let replacement = observe(&mut board, 20, 2, USB, 20, 10).unwrap();
    assert_eq!(replacement.expired, 1);
    assert_ne!(replacement.snapshot.candidates, first.snapshot.candidates);
    assert!(matches!(
        replacement.event,
        UsbDiscoveryEvent::Availability(ObserveUsbPairingAvailabilityOutcome::Added { .. })
    ));
    let refreshed = observe(&mut board, 21, 2, USB, 21, 10).unwrap();
    assert_eq!(
        refreshed.event,
        UsbDiscoveryEvent::Availability(ObserveUsbPairingAvailabilityOutcome::Updated {
            candidate: *refreshed.snapshot.candidates.first().unwrap()
        })
    );
    let cleared = intent(&mut board, 31, UsbDiscoveryIntent::Clear).unwrap();
    assert_eq!(cleared.expired, 1);
    assert_eq!(
        cleared.event,
        UsbDiscoveryEvent::Cleared(ClearUsbPairingCandidatesOutcome { removed: 0 })
    );
}

#[test]
fn clock_invariants_preserve_the_list_and_do_not_hide_observation_failures() {
    let mut board = UsbDiscoverySwitchboard::<1>::new(USB, InstantMillis(10));
    let initial = observe(&mut board, 10, 1, USB, 10, 50).unwrap();
    assert_eq!(
        intent(&mut board, 9, UsbDiscoveryIntent::Clear),
        Err(UsbDiscoveryRoutingError::Clock(
            AdvanceUsbPairingDiscoveryError::TimeWentBackwards {
                current: InstantMillis(10)
            }
        ))
    );
    assert_eq!(
        observe(&mut board, 20, 2, USB, 21, 10),
        Err(UsbDiscoveryRoutingError::Observation(
            ObserveUsbPairingAvailabilityError::ObservedInFuture
        ))
    );
    let inspected = intent(&mut board, 20, UsbDiscoveryIntent::Inspect).unwrap();
    assert_eq!(inspected.snapshot.candidates, initial.snapshot.candidates);
    assert_eq!(inspected.snapshot.as_of, InstantMillis(20));
    assert_eq!(inspected.expired, 0);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn arbitrary_inspection_selection_and_clear_histories_expire_before_routing(
        actions in prop::collection::vec((0u8..3, 0u64..25), 1..30),
    ) {
        let mut board = UsbDiscoverySwitchboard::<1>::new(USB, InstantMillis(0));
        let added = observe(&mut board, 0, 1, USB, 0, 100).unwrap();
        let candidate = *added.snapshot.candidates.first().unwrap();
        let mut present = true;
        let mut now = 0u64;
        for (action, elapsed) in actions {
            now = now.saturating_add(elapsed);
            let expired = usize::from(present && now >= 100);
            present &= now < 100;
            let (command, event) = match action {
                0 => (UsbDiscoveryIntent::Inspect, UsbDiscoveryEvent::Inspected),
                1 => (UsbDiscoveryIntent::Select { endpoint: candidate.endpoint() },
                    UsbDiscoveryEvent::Selection(if present { SelectUsbPairingCandidateOutcome::Selected { candidate } } else { SelectUsbPairingCandidateOutcome::Unavailable })),
                _ => {
                    let removed = usize::from(present);
                    present = false;
                    (UsbDiscoveryIntent::Clear, UsbDiscoveryEvent::Cleared(ClearUsbPairingCandidatesOutcome { removed }))
                }
            };
            let update = intent(&mut board, now, command).unwrap();
            prop_assert_eq!(update.expired, expired);
            prop_assert_eq!(update.event, event);
            prop_assert_eq!(update.snapshot.as_of, InstantMillis(now));
            prop_assert_eq!(update.snapshot.candidates.as_slice(), if present { core::slice::from_ref(&candidate) } else { &[] });
        }
    }
}
