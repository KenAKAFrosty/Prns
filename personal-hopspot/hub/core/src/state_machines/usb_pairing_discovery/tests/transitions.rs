use super::*;

#[test]
fn verified_usb_availability_is_listed_and_selection_preserves_its_provenance() {
    let mut discovery = UsbPairingDiscovery::<2>::new(USB, InstantMillis(100));
    assert_eq!(discovery.step(ReadUsbPairingCandidates), snapshot(100, &[]));
    let first = candidate(1, 99, 100);
    let second = candidate(2, 100, 200);
    assert_eq!(first.source_interface(), USB);
    assert_eq!(first.observed_at(), InstantMillis(99));
    assert_eq!(first.expires_at(), InstantMillis(199));
    assert_ne!(first.endpoint(), second.endpoint());
    assert_eq!(
        discovery.step(SelectUsbPairingCandidate {
            endpoint: first.endpoint()
        }),
        SelectUsbPairingCandidateOutcome::Unavailable
    );
    assert_eq!(
        observe(&mut discovery, 1, USB, 99, 100),
        Ok(ObserveUsbPairingAvailabilityOutcome::Added { candidate: first })
    );
    assert_eq!(
        observe(&mut discovery, 2, USB, 100, 200),
        Ok(ObserveUsbPairingAvailabilityOutcome::Added { candidate: second })
    );
    let before = discovery.step(ReadUsbPairingCandidates);
    assert_eq!(before, snapshot(100, &[first, second]));
    for selected in [second, first, second] {
        assert_eq!(
            discovery.step(SelectUsbPairingCandidate {
                endpoint: selected.endpoint()
            }),
            SelectUsbPairingCandidateOutcome::Selected {
                candidate: selected
            }
        );
        assert_eq!(discovery.step(ReadUsbPairingCandidates), before);
    }
    assert_eq!(
        discovery.step(ClearUsbPairingCandidates),
        ClearUsbPairingCandidatesOutcome { removed: 2 }
    );
    assert_eq!(before, snapshot(100, &[first, second]));
    assert_eq!(discovery.step(ReadUsbPairingCandidates), snapshot(100, &[]));
    assert_eq!(
        discovery.step(SelectUsbPairingCandidate {
            endpoint: second.endpoint()
        }),
        SelectUsbPairingCandidateOutcome::Unavailable
    );
    assert_eq!(
        discovery.step(ClearUsbPairingCandidates),
        ClearUsbPairingCandidatesOutcome { removed: 0 }
    );
    assert_eq!(
        observe(&mut discovery, 1, USB, 100, 100),
        Ok(ObserveUsbPairingAvailabilityOutcome::Added {
            candidate: candidate(1, 100, 100)
        })
    );
}

#[test]
fn other_interfaces_future_and_expired_observations_cannot_change_the_list() {
    let mut discovery = UsbPairingDiscovery::<2>::new(USB, InstantMillis(100));
    let kept = candidate(1, 95, 100);
    assert_eq!(
        observe(&mut discovery, 1, USB, 95, 100),
        Ok(ObserveUsbPairingAvailabilityOutcome::Added { candidate: kept })
    );
    for (seed, source, observed, lifetime, expected) in [
        (
            2,
            OTHER,
            100,
            100,
            Ok(ObserveUsbPairingAvailabilityOutcome::WrongInterface),
        ),
        (
            1,
            USB,
            101,
            100,
            Err(ObserveUsbPairingAvailabilityError::ObservedInFuture),
        ),
        (
            1,
            USB,
            90,
            10,
            Ok(ObserveUsbPairingAvailabilityOutcome::Expired),
        ),
        (
            2,
            USB,
            80,
            10,
            Ok(ObserveUsbPairingAvailabilityOutcome::Expired),
        ),
    ] {
        assert_eq!(
            observe(&mut discovery, seed, source, observed, lifetime),
            expected
        );
        assert_eq!(
            discovery.step(ReadUsbPairingCandidates),
            snapshot(100, &[kept])
        );
    }
}

#[test]
fn newer_observations_update_in_place_even_when_full_and_older_ones_cannot_extend_expiry() {
    let mut discovery = UsbPairingDiscovery::<2>::new(USB, InstantMillis(100));
    let first = candidate(1, 90, 100);
    let second = candidate(2, 90, 100);
    for (seed, candidate) in [(1, first), (2, second)] {
        assert_eq!(
            observe(&mut discovery, seed, USB, 90, 100),
            Ok(ObserveUsbPairingAvailabilityOutcome::Added { candidate })
        );
    }
    assert_eq!(
        observe(&mut discovery, 3, USB, 100, 100),
        Ok(ObserveUsbPairingAvailabilityOutcome::CapacityExceeded { maximum: 2 })
    );
    assert_eq!(
        discovery.step(ReadUsbPairingCandidates),
        snapshot(100, &[first, second])
    );
    let updated = candidate(2, 100, 10);
    assert_eq!(
        observe(&mut discovery, 2, USB, 100, 10),
        Ok(ObserveUsbPairingAvailabilityOutcome::Updated { candidate: updated })
    );
    for observed in [99, 100] {
        assert_eq!(
            observe(&mut discovery, 2, USB, observed, 1_000),
            Ok(ObserveUsbPairingAvailabilityOutcome::StaleObservation)
        );
        assert_eq!(
            discovery.step(ReadUsbPairingCandidates),
            snapshot(100, &[first, updated])
        );
    }
    assert_eq!(
        discovery.step(SelectUsbPairingCandidate {
            endpoint: updated.endpoint()
        }),
        SelectUsbPairingCandidateOutcome::Selected { candidate: updated }
    );
}

#[test]
fn advancing_time_expires_at_the_deadline_recovers_capacity_and_refuses_to_rewind() {
    let mut discovery = UsbPairingDiscovery::<2>::new(USB, InstantMillis(100));
    let first = candidate(1, 100, 10);
    let second = candidate(2, 100, 11);
    for (seed, lifetime, candidate) in [(1, 10, first), (2, 11, second)] {
        assert_eq!(
            observe(&mut discovery, seed, USB, 100, lifetime),
            Ok(ObserveUsbPairingAvailabilityOutcome::Added { candidate })
        );
    }
    for now in [100, 109] {
        assert_eq!(
            discovery.step(AdvanceUsbPairingDiscovery {
                now: InstantMillis(now)
            }),
            Ok(AdvanceUsbPairingDiscoveryOutcome::Advanced { expired: 0 })
        );
        assert_eq!(
            discovery.step(ReadUsbPairingCandidates),
            snapshot(now, &[first, second])
        );
    }
    assert_eq!(
        discovery.step(AdvanceUsbPairingDiscovery {
            now: InstantMillis(110)
        }),
        Ok(AdvanceUsbPairingDiscoveryOutcome::Advanced { expired: 1 })
    );
    assert_eq!(
        discovery.step(ReadUsbPairingCandidates),
        snapshot(110, &[second])
    );
    assert_eq!(
        discovery.step(SelectUsbPairingCandidate {
            endpoint: first.endpoint()
        }),
        SelectUsbPairingCandidateOutcome::Unavailable
    );
    assert_eq!(
        discovery.step(AdvanceUsbPairingDiscovery {
            now: InstantMillis(109)
        }),
        Err(AdvanceUsbPairingDiscoveryError::TimeWentBackwards {
            current: InstantMillis(110)
        })
    );
    assert_eq!(
        discovery.step(ReadUsbPairingCandidates),
        snapshot(110, &[second])
    );
    let next = candidate(3, 110, 1);
    assert_eq!(
        observe(&mut discovery, 3, USB, 110, 1),
        Ok(ObserveUsbPairingAvailabilityOutcome::Added { candidate: next })
    );
    assert_eq!(
        discovery.step(ReadUsbPairingCandidates),
        snapshot(110, &[second, next])
    );
    assert_eq!(
        discovery.step(AdvanceUsbPairingDiscovery {
            now: InstantMillis(u64::MAX)
        }),
        Ok(AdvanceUsbPairingDiscoveryOutcome::Advanced { expired: 2 })
    );
    assert_eq!(
        discovery.step(ReadUsbPairingCandidates),
        snapshot(u64::MAX, &[])
    );
}
