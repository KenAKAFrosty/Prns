use super::acquisition::ValidatedAcquiredSchedule;
use super::clock::{
    AcquisitionCandidate, ClockError, ClockWindow, ScheduleMicros, TrustedScheduleClock,
};
use super::schedule::{TurboChannelIndex, TurboGlobalSlot};
use super::spec::US915_TURBO_SPEC;
use crate::interfaces::subghz::{Frequency, MonotonicMicros};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboRadioDwell {
    begins_at: MonotonicMicros,
    ends_before: MonotonicMicros,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboRadioDwellError {
    EmptyOrReversed {
        begins_at: MonotonicMicros,
        ends_before: MonotonicMicros,
    },
}

impl TurboRadioDwell {
    pub const fn new(
        begins_at: MonotonicMicros,
        ends_before: MonotonicMicros,
    ) -> Result<Self, TurboRadioDwellError> {
        if begins_at.micros() >= ends_before.micros() {
            return Err(TurboRadioDwellError::EmptyOrReversed {
                begins_at,
                ends_before,
            });
        }
        Ok(Self {
            begins_at,
            ends_before,
        })
    }

    pub const fn begins_at(self) -> MonotonicMicros {
        self.begins_at
    }

    pub const fn ends_before(self) -> MonotonicMicros {
        self.ends_before
    }

    pub const fn duration_us(self) -> u64 {
        self.ends_before.micros() - self.begins_at.micros()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboReceiveLease {
    dwell: TurboRadioDwell,
    global_slot: TurboGlobalSlot,
    channel_index: TurboChannelIndex,
    frequency: Frequency,
}

impl TurboReceiveLease {
    pub const fn dwell(self) -> TurboRadioDwell {
        self.dwell
    }

    pub const fn global_slot(self) -> TurboGlobalSlot {
        self.global_slot
    }

    pub const fn channel_index(self) -> TurboChannelIndex {
        self.channel_index
    }

    pub const fn frequency(self) -> Frequency {
        self.frequency
    }

    pub const fn is_active_at(self, now: MonotonicMicros) -> bool {
        self.dwell.begins_at().micros() <= now.micros()
            && now.micros() < self.dwell.ends_before().micros()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboReceiveLeaseError {
    ClockAtBeginning(ClockError),
    ClockBeforeEnd(ClockError),
    ClockEnvelopeCrossesSlotBoundary {
        earliest_slot: TurboGlobalSlot,
        latest_slot: TurboGlobalSlot,
    },
}

impl TrustedScheduleClock {
    pub fn authorize_receive(
        self,
        dwell: TurboRadioDwell,
    ) -> Result<TurboReceiveLease, TurboReceiveLeaseError> {
        let beginning = self
            .window_at(dwell.begins_at())
            .map_err(TurboReceiveLeaseError::ClockAtBeginning)?;
        let final_microsecond = MonotonicMicros::new(dwell.ends_before().micros() - 1);
        let before_end = self
            .window_at(final_microsecond)
            .map_err(TurboReceiveLeaseError::ClockBeforeEnd)?;
        authorize_receive_envelope(dwell, beginning, before_end)
    }
}

impl AcquisitionCandidate {
    pub fn authorize_receive(
        self,
        dwell: TurboRadioDwell,
    ) -> Result<TurboReceiveLease, TurboReceiveLeaseError> {
        let beginning = self
            .window_at(dwell.begins_at())
            .map_err(TurboReceiveLeaseError::ClockAtBeginning)?;
        let final_microsecond = MonotonicMicros::new(dwell.ends_before().micros() - 1);
        let before_end = self
            .window_at(final_microsecond)
            .map_err(TurboReceiveLeaseError::ClockBeforeEnd)?;
        authorize_receive_envelope(dwell, beginning, before_end)
    }
}

impl ValidatedAcquiredSchedule {
    pub fn authorize_receive(
        self,
        dwell: TurboRadioDwell,
    ) -> Result<TurboReceiveLease, TurboReceiveLeaseError> {
        let beginning = self
            .window_at(dwell.begins_at())
            .map_err(TurboReceiveLeaseError::ClockAtBeginning)?;
        let final_microsecond = MonotonicMicros::new(dwell.ends_before().micros() - 1);
        let before_end = self
            .window_at(final_microsecond)
            .map_err(TurboReceiveLeaseError::ClockBeforeEnd)?;
        authorize_receive_envelope(dwell, beginning, before_end)
    }
}

fn authorize_receive_envelope(
    dwell: TurboRadioDwell,
    beginning: ClockWindow,
    before_end: ClockWindow,
) -> Result<TurboReceiveLease, TurboReceiveLeaseError> {
    let earliest_schedule_us = beginning
        .earliest_schedule_us()
        .min(before_end.earliest_schedule_us());
    let latest_schedule_us = beginning
        .latest_schedule_us()
        .max(before_end.latest_schedule_us());
    let earliest = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(earliest_schedule_us));
    let latest = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(latest_schedule_us));
    if earliest.global_slot() != latest.global_slot() {
        return Err(TurboReceiveLeaseError::ClockEnvelopeCrossesSlotBoundary {
            earliest_slot: earliest.global_slot(),
            latest_slot: latest.global_slot(),
        });
    }
    Ok(TurboReceiveLease {
        dwell,
        global_slot: earliest.global_slot(),
        channel_index: earliest.channel_index(),
        frequency: earliest.frequency(),
    })
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;
    use crate::interfaces::subghz::regions::us915::turbo::{TrustedTimeSource, UtcTimescale};

    #[kani::proof]
    #[kani::unwind(9)]
    fn granted_receive_lease_keeps_every_instant_on_one_slot() {
        let schedule_at_observation: u16 = kani::any();
        let begins_at: u8 = kani::any();
        let duration_us = u64::from(kani::any::<u8>() % 8 + 1);
        let uncertainty_us = u64::from(kani::any::<u8>()) + 1;
        let maximum_drift_ppm = u32::from(kani::any::<u8>()) + 1;
        let Ok(clock) = TrustedScheduleClock::new(
            MonotonicMicros::new(0),
            ScheduleMicros::new(u64::from(schedule_at_observation)),
            uncertainty_us,
            maximum_drift_ppm,
            TrustedTimeSource::Gnss,
            UtcTimescale::PosixUnsmeared,
        ) else {
            return;
        };
        let begins_at = MonotonicMicros::new(u64::from(begins_at));
        let ends_before = MonotonicMicros::new(begins_at.micros() + duration_us);
        let Ok(dwell) = TurboRadioDwell::new(begins_at, ends_before) else {
            return;
        };
        let Ok(lease) = clock.authorize_receive(dwell) else {
            return;
        };
        for offset_us in 0..8 {
            if offset_us >= duration_us {
                continue;
            }
            let now = MonotonicMicros::new(begins_at.micros() + offset_us);
            let Ok(window) = clock.window_at(now) else {
                assert!(false);
                return;
            };
            let earliest = US915_TURBO_SPEC
                .slot_at(ScheduleMicros::new(window.earliest_schedule_us()))
                .global_slot();
            let latest = US915_TURBO_SPEC
                .slot_at(ScheduleMicros::new(window.latest_schedule_us()))
                .global_slot();
            assert_eq!(earliest, lease.global_slot());
            assert_eq!(latest, lease.global_slot());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::subghz::regions::us915::turbo::{
        AcquisitionTracker, AcquisitionTrackerConfigurationError, MaximumTransmitUncertainty,
        TrustedTimeSource, UtcTimescale,
    };
    use proptest::prelude::*;

    fn clock(
        observed_at: u64,
        schedule_at_observation: u64,
        uncertainty_us: u64,
        maximum_drift_ppm: u32,
    ) -> TrustedScheduleClock {
        TrustedScheduleClock::new(
            MonotonicMicros::new(observed_at),
            ScheduleMicros::new(schedule_at_observation),
            uncertainty_us,
            maximum_drift_ppm,
            TrustedTimeSource::Gnss,
            UtcTimescale::PosixUnsmeared,
        )
        .unwrap()
    }

    #[test]
    fn stable_clock_envelope_grants_one_exact_channel_dwell() {
        let clock = clock(100, 800_100, 50, 1);
        let dwell =
            TurboRadioDwell::new(MonotonicMicros::new(100), MonotonicMicros::new(200)).unwrap();
        let lease = clock.authorize_receive(dwell).unwrap();
        let scheduled = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(800_100));
        assert_eq!(lease.dwell(), dwell);
        assert_eq!(lease.global_slot(), scheduled.global_slot());
        assert_eq!(lease.channel_index(), scheduled.channel_index());
        assert_eq!(lease.frequency(), scheduled.frequency());
        assert!(lease.is_active_at(MonotonicMicros::new(100)));
        assert!(lease.is_active_at(MonotonicMicros::new(199)));
        assert!(!lease.is_active_at(MonotonicMicros::new(200)));
    }

    #[test]
    fn exclusive_end_preserves_the_last_safe_microsecond() {
        let clock = clock(399_800, 399_800, 100, 1);
        let safe =
            TurboRadioDwell::new(MonotonicMicros::new(399_800), MonotonicMicros::new(399_899))
                .unwrap();
        assert!(clock.authorize_receive(safe).is_ok());

        let ambiguous =
            TurboRadioDwell::new(MonotonicMicros::new(399_800), MonotonicMicros::new(399_900))
                .unwrap();
        assert_eq!(
            clock.authorize_receive(ambiguous),
            Err(TurboReceiveLeaseError::ClockEnvelopeCrossesSlotBoundary {
                earliest_slot: TurboGlobalSlot::new(0).unwrap(),
                latest_slot: TurboGlobalSlot::new(1).unwrap(),
            })
        );
    }

    #[test]
    fn acquisition_candidate_can_authorize_receive_during_graduation() {
        let candidate = AcquisitionCandidate::new(
            MonotonicMicros::new(100),
            ScheduleMicros::new(800_100),
            50,
            1,
            3,
            3,
        )
        .unwrap();
        let dwell =
            TurboRadioDwell::new(MonotonicMicros::new(100), MonotonicMicros::new(200)).unwrap();
        let lease = candidate.authorize_receive(dwell).unwrap();
        assert_eq!(
            lease.global_slot(),
            US915_TURBO_SPEC
                .slot_at(ScheduleMicros::new(800_100))
                .global_slot()
        );
    }

    #[test]
    fn invalid_dwells_and_unusable_clock_ranges_are_typed() {
        assert_eq!(
            TurboRadioDwell::new(MonotonicMicros::new(7), MonotonicMicros::new(7)),
            Err(TurboRadioDwellError::EmptyOrReversed {
                begins_at: MonotonicMicros::new(7),
                ends_before: MonotonicMicros::new(7),
            })
        );
        let clock = clock(10, 100, 1, 1);
        let dwell =
            TurboRadioDwell::new(MonotonicMicros::new(9), MonotonicMicros::new(10)).unwrap();
        assert_eq!(
            clock.authorize_receive(dwell),
            Err(TurboReceiveLeaseError::ClockAtBeginning(
                ClockError::MonotonicTimeWentBackward
            ))
        );
    }

    #[test]
    fn invalid_clock_and_acquisition_bounds_are_rejected() {
        assert_eq!(
            TrustedScheduleClock::new(
                MonotonicMicros::new(0),
                ScheduleMicros::new(0),
                1,
                1_000_001,
                TrustedTimeSource::Gnss,
                UtcTimescale::PosixUnsmeared,
            ),
            Err(ClockError::DriftBoundOutsideRange {
                ppm: 1_000_001,
                maximum_ppm: 1_000_000,
            })
        );
        assert_eq!(
            AcquisitionTracker::new(1_000_001, MaximumTransmitUncertainty::new(1_000).unwrap())
                .err(),
            Some(AcquisitionTrackerConfigurationError::Clock(
                ClockError::DriftBoundOutsideRange {
                    ppm: 1_000_001,
                    maximum_ppm: 1_000_000,
                }
            ))
        );
        assert_eq!(
            AcquisitionTracker::new(1, MaximumTransmitUncertainty::new(249).unwrap()).err(),
            Some(
                AcquisitionTrackerConfigurationError::TransmitUncertaintyBelowAcquisitionFloor {
                    actual_us: 249,
                    minimum_us: 250,
                }
            )
        );
    }

    proptest! {
        #[test]
        fn every_sample_in_a_granted_dwell_matches_its_lease(
            schedule_at_observation in 0u64..10_000_000,
            begins_at in 0u64..1_000,
            duration_us in 1u64..500,
            uncertainty_us in 1u64..200,
            maximum_drift_ppm in 1u32..100,
        ) {
            let clock = clock(0, schedule_at_observation, uncertainty_us, maximum_drift_ppm);
            let dwell = TurboRadioDwell::new(
                MonotonicMicros::new(begins_at),
                MonotonicMicros::new(begins_at + duration_us),
            ).unwrap();
            let Ok(lease) = clock.authorize_receive(dwell) else {
                return Ok(());
            };
            for now in [begins_at, begins_at + duration_us / 2, begins_at + duration_us - 1] {
                let window = clock.window_at(MonotonicMicros::new(now)).unwrap();
                let earliest = US915_TURBO_SPEC
                    .slot_at(ScheduleMicros::new(window.earliest_schedule_us()))
                    .global_slot();
                let latest = US915_TURBO_SPEC
                    .slot_at(ScheduleMicros::new(window.latest_schedule_us()))
                    .global_slot();
                prop_assert_eq!(earliest, lease.global_slot());
                prop_assert_eq!(latest, lease.global_slot());
            }
        }
    }
}
