use super::*;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn arbitrary_arrival_and_expiry_orders_match_an_independent_bounded_model(
        actions in prop::collection::vec((0u8..4, 1u8..=4, 0u64..20, 1u64..30), 1..24),
    ) {
        let mut discovery = UsbPairingDiscovery::<2>::new(USB, InstantMillis(20));
        let mut now = 20u64;
        let mut model = alloc::vec::Vec::<(u8, PairingCandidate)>::new();
        for (action, seed, offset, lifetime) in actions {
            match action {
                0 => {
                    now = now.saturating_add(offset);
                    let before = model.len();
                    model.retain(|(_, candidate)| candidate.expires_at().0 > now);
                    prop_assert_eq!(discovery.step(AdvanceUsbPairingDiscovery { now: InstantMillis(now) }), Ok(AdvanceUsbPairingDiscoveryOutcome::Advanced { expired: before.saturating_sub(model.len()) }));
                }
                1 | 2 => {
                    let observed = now.saturating_sub(offset);
                    let incoming = candidate(seed, observed, lifetime);
                    let expected = if observed.saturating_add(lifetime) <= now {
                        Ok(ObserveUsbPairingAvailabilityOutcome::Expired)
                    } else if let Some((_, existing)) = model.iter_mut().find(|(id, _)| *id == seed) {
                        if observed > existing.observed_at().0 {
                            *existing = incoming;
                            Ok(ObserveUsbPairingAvailabilityOutcome::Updated { candidate: incoming })
                        } else {
                            Ok(ObserveUsbPairingAvailabilityOutcome::StaleObservation)
                        }
                    } else if model.len() == 2 {
                        Ok(ObserveUsbPairingAvailabilityOutcome::CapacityExceeded { maximum: 2 })
                    } else {
                        model.push((seed, incoming));
                        Ok(ObserveUsbPairingAvailabilityOutcome::Added { candidate: incoming })
                    };
                    prop_assert_eq!(observe(&mut discovery, seed, USB, observed, lifetime), expected);
                }
                _ => {
                    prop_assert_eq!(discovery.step(ClearUsbPairingCandidates), ClearUsbPairingCandidatesOutcome { removed: model.len() });
                    model.clear();
                }
            }
            let expected: alloc::vec::Vec<_> = model.iter().map(|(_, candidate)| *candidate).collect();
            prop_assert_eq!(discovery.step(ReadUsbPairingCandidates), snapshot(now, &expected));
            for seed in 1..=4 {
                let expected = match model.iter().find(|(id, _)| *id == seed) {
                    Some((_, candidate)) => SelectUsbPairingCandidateOutcome::Selected { candidate: *candidate },
                    None => SelectUsbPairingCandidateOutcome::Unavailable,
                };
                prop_assert_eq!(discovery.step(SelectUsbPairingCandidate { endpoint: candidate(seed, 0, 1).endpoint() }), expected);
            }
        }
    }
}
