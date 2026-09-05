use super::super::MonotonicMicros;
use super::schedule::{TURBO_CHANNEL_COUNT, TURBO_OCCUPANCY_LIMIT_US};

const OCCUPANCY_WINDOW_US: u64 = 10_000_000;
const UNUSED_VISIT: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChannelVisit {
    global_slot: u64,
    reserved_airtime_us: u64,
    keyed_airtime_us: u64,
    last_rf_end: MonotonicMicros,
    has_keyed_rf: bool,
}

const EMPTY_VISIT: ChannelVisit = ChannelVisit {
    global_slot: UNUSED_VISIT,
    reserved_airtime_us: 0,
    keyed_airtime_us: 0,
    last_rf_end: MonotonicMicros::new(0),
    has_keyed_rf: false,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct OccupancyReservation {
    channel_index: usize,
    global_slot: u64,
    airtime_us: u64,
}

impl OccupancyReservation {
    pub(crate) const fn channel_index(&self) -> usize {
        self.channel_index
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccupancyError {
    ChannelOutsideHopSet {
        channel_index: usize,
    },
    EmptyAirtime,
    PriorVisitInsideObservationWindow {
        transmit_not_before_us: u64,
    },
    PriorReservationPending,
    ChannelOccupancyExceeded {
        projected_us: u64,
        maximum_us: u64,
    },
    UnequalChannelUse {
        channel_index: usize,
        projected_us: u64,
        permitted_us: u64,
    },
    ReservationMismatch,
    ActualAirtimeExceededReservation {
        reserved_us: u64,
        actual_us: u64,
    },
}

pub(crate) struct TurboOccupancyLedger {
    visits: [ChannelVisit; TURBO_CHANNEL_COUNT],
    cumulative_use_us: [u64; TURBO_CHANNEL_COUNT],
}

impl TurboOccupancyLedger {
    pub(crate) const fn new() -> Self {
        Self {
            visits: [EMPTY_VISIT; TURBO_CHANNEL_COUNT],
            cumulative_use_us: [0; TURBO_CHANNEL_COUNT],
        }
    }

    pub(crate) fn reserve(
        &mut self,
        channel_index: usize,
        global_slot: u64,
        now: MonotonicMicros,
        airtime_us: u64,
    ) -> Result<OccupancyReservation, OccupancyError> {
        if channel_index >= TURBO_CHANNEL_COUNT {
            return Err(OccupancyError::ChannelOutsideHopSet { channel_index });
        }
        if airtime_us == 0 {
            return Err(OccupancyError::EmptyAirtime);
        }
        let visit = &mut self.visits[channel_index];
        if visit.global_slot != global_slot {
            if visit.reserved_airtime_us != 0 {
                return Err(OccupancyError::PriorReservationPending);
            }
            if visit.has_keyed_rf {
                let transmit_not_before_us = visit
                    .last_rf_end
                    .micros()
                    .saturating_add(OCCUPANCY_WINDOW_US);
                if now.micros() < transmit_not_before_us {
                    return Err(OccupancyError::PriorVisitInsideObservationWindow {
                        transmit_not_before_us,
                    });
                }
            }
            *visit = ChannelVisit {
                global_slot,
                ..EMPTY_VISIT
            };
        }
        let projected_occupancy_us = visit
            .keyed_airtime_us
            .saturating_add(visit.reserved_airtime_us)
            .saturating_add(airtime_us);
        if projected_occupancy_us > TURBO_OCCUPANCY_LIMIT_US {
            return Err(OccupancyError::ChannelOccupancyExceeded {
                projected_us: projected_occupancy_us,
                maximum_us: TURBO_OCCUPANCY_LIMIT_US,
            });
        }
        let minimum_use_us = self.cumulative_use_us.iter().copied().min().unwrap_or(0);
        let permitted_us = minimum_use_us.saturating_add(TURBO_OCCUPANCY_LIMIT_US);
        let projected_use_us = self.cumulative_use_us[channel_index].saturating_add(airtime_us);
        if projected_use_us > permitted_us {
            return Err(OccupancyError::UnequalChannelUse {
                channel_index,
                projected_us: projected_use_us,
                permitted_us,
            });
        }
        visit.reserved_airtime_us = visit.reserved_airtime_us.saturating_add(airtime_us);
        self.cumulative_use_us[channel_index] = projected_use_us;
        Ok(OccupancyReservation {
            channel_index,
            global_slot,
            airtime_us,
        })
    }

    pub(crate) fn abort(
        &mut self,
        reservation: OccupancyReservation,
    ) -> Result<(), OccupancyError> {
        let visit = self
            .visits
            .get_mut(reservation.channel_index)
            .ok_or(OccupancyError::ReservationMismatch)?;
        if visit.global_slot != reservation.global_slot
            || visit.reserved_airtime_us < reservation.airtime_us
            || self.cumulative_use_us[reservation.channel_index] < reservation.airtime_us
        {
            return Err(OccupancyError::ReservationMismatch);
        }
        visit.reserved_airtime_us -= reservation.airtime_us;
        self.cumulative_use_us[reservation.channel_index] -= reservation.airtime_us;
        Ok(())
    }

    pub(crate) fn complete(
        &mut self,
        reservation: OccupancyReservation,
        rf_ended_at: MonotonicMicros,
        actual_airtime_us: u64,
    ) -> Result<(), OccupancyError> {
        let visit = self
            .visits
            .get_mut(reservation.channel_index)
            .ok_or(OccupancyError::ReservationMismatch)?;
        if visit.global_slot != reservation.global_slot
            || visit.reserved_airtime_us < reservation.airtime_us
        {
            return Err(OccupancyError::ReservationMismatch);
        }
        visit.reserved_airtime_us -= reservation.airtime_us;
        visit.keyed_airtime_us = visit.keyed_airtime_us.saturating_add(actual_airtime_us);
        visit.last_rf_end = rf_ended_at;
        visit.has_keyed_rf = true;
        if actual_airtime_us < reservation.airtime_us {
            self.cumulative_use_us[reservation.channel_index] -=
                reservation.airtime_us - actual_airtime_us;
        } else {
            self.cumulative_use_us[reservation.channel_index] = self.cumulative_use_us
                [reservation.channel_index]
                .saturating_add(actual_airtime_us - reservation.airtime_us);
        }
        let keyed_airtime_us = visit.keyed_airtime_us;
        self.normalize_use_totals();
        if actual_airtime_us > reservation.airtime_us {
            return Err(OccupancyError::ActualAirtimeExceededReservation {
                reserved_us: reservation.airtime_us,
                actual_us: actual_airtime_us,
            });
        }
        if keyed_airtime_us > TURBO_OCCUPANCY_LIMIT_US {
            return Err(OccupancyError::ChannelOccupancyExceeded {
                projected_us: keyed_airtime_us,
                maximum_us: TURBO_OCCUPANCY_LIMIT_US,
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn channel_keyed_airtime_us(&self, channel_index: usize) -> u64 {
        self.visits[channel_index].keyed_airtime_us
    }

    #[cfg(test)]
    pub(crate) fn channel_use_airtime_us(&self, channel_index: usize) -> u64 {
        self.cumulative_use_us[channel_index]
    }

    fn normalize_use_totals(&mut self) {
        let minimum_use_us = self.cumulative_use_us.iter().copied().min().unwrap_or(0);
        if minimum_use_us == 0 {
            return;
        }
        for use_us in &mut self.cumulative_use_us {
            *use_us -= minimum_use_us;
        }
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn accepted_reservation_never_exceeds_occupancy_or_balance_limit() {
        let channel: u8 = kani::any();
        let airtime: u32 = kani::any();
        kani::assume(channel < TURBO_CHANNEL_COUNT as u8);
        kani::assume(airtime > 0);
        let mut ledger = TurboOccupancyLedger::new();
        if ledger
            .reserve(channel as usize, 0, MonotonicMicros::new(0), airtime as u64)
            .is_ok()
        {
            assert!(airtime as u64 <= TURBO_OCCUPANCY_LIMIT_US);
            assert!(ledger.cumulative_use_us[channel as usize] <= TURBO_OCCUPANCY_LIMIT_US);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::subghz::frequency_hopping::{
        audit_frequency_occupancy, ChannelOccupancyLimit, FrequencyOccupancyError, HopTransmission,
        MeasuredTwentyDbBandwidth, Us915HopSet, Us915HoppingModel,
    };
    use proptest::prelude::*;

    fn hop_set() -> Us915HopSet<TURBO_CHANNEL_COUNT> {
        let model = Us915HoppingModel::new(
            MeasuredTwentyDbBandwidth::new(500_000).unwrap(),
            ChannelOccupancyLimit::new(TURBO_OCCUPANCY_LIMIT_US).unwrap(),
        )
        .unwrap();
        Us915HopSet::new(model, super::super::schedule::US915_TURBO_CHANNELS).unwrap()
    }

    #[test]
    fn more_than_sixty_four_transmissions_need_no_history_capacity() {
        let mut ledger = TurboOccupancyLedger::new();
        for transmission in 0..200u64 {
            let channel = transmission as usize % TURBO_CHANNEL_COUNT;
            let reservation = ledger
                .reserve(
                    channel,
                    channel as u64,
                    MonotonicMicros::new(transmission * 2_000),
                    1_000,
                )
                .unwrap();
            ledger
                .complete(
                    reservation,
                    MonotonicMicros::new(transmission * 2_000 + 1_000),
                    1_000,
                )
                .unwrap();
        }
    }

    #[test]
    fn cumulative_balance_blocks_a_periodic_single_channel_producer() {
        let mut ledger = TurboOccupancyLedger::new();
        let reservation = ledger
            .reserve(0, 0, MonotonicMicros::new(0), TURBO_OCCUPANCY_LIMIT_US)
            .unwrap();
        ledger
            .complete(
                reservation,
                MonotonicMicros::new(TURBO_OCCUPANCY_LIMIT_US),
                TURBO_OCCUPANCY_LIMIT_US,
            )
            .unwrap();
        assert!(matches!(
            ledger.reserve(0, 100, MonotonicMicros::new(20_000_000), 1),
            Err(OccupancyError::UnequalChannelUse { .. })
        ));
    }

    #[test]
    fn completing_every_channel_renormalizes_equal_use_to_zero() {
        let mut ledger = TurboOccupancyLedger::new();
        for channel in 0..TURBO_CHANNEL_COUNT {
            let reservation = ledger
                .reserve(
                    channel,
                    channel as u64,
                    MonotonicMicros::new(channel as u64 * 1_000),
                    100,
                )
                .unwrap();
            ledger
                .complete(
                    reservation,
                    MonotonicMicros::new(channel as u64 * 1_000 + 100),
                    100,
                )
                .unwrap();
        }
        assert_eq!(ledger.cumulative_use_us, [0; TURBO_CHANNEL_COUNT]);
    }

    #[test]
    fn unresolved_reservation_cannot_be_replaced_by_another_visit() {
        let mut ledger = TurboOccupancyLedger::new();
        let reservation = ledger
            .reserve(0, 0, MonotonicMicros::new(0), 1_000)
            .unwrap();
        assert_eq!(
            ledger.reserve(0, 1, MonotonicMicros::new(20_000_000), 1_000),
            Err(OccupancyError::PriorReservationPending)
        );
        ledger.abort(reservation).unwrap();
        assert!(ledger
            .reserve(0, 1, MonotonicMicros::new(20_000_000), 1_000)
            .is_ok());
    }

    proptest! {
        #[test]
        fn runtime_admission_matches_the_offline_oracle(
            airtimes in prop::collection::vec(1u32..=25_000, 1..40)
        ) {
            let hop_set = hop_set();
            let mut ledger = TurboOccupancyLedger::new();
            let mut transmissions = Vec::new();
            let mut started_at_us = 0u64;
            for airtime_us in airtimes.into_iter().map(u64::from) {
                let prospective = HopTransmission::new(
                    0,
                    MonotonicMicros::new(started_at_us),
                    airtime_us,
                )
                .unwrap();
                transmissions.push(prospective);
                let oracle = audit_frequency_occupancy(&hop_set, &transmissions);
                let runtime = ledger.reserve(
                    0,
                    0,
                    MonotonicMicros::new(started_at_us),
                    airtime_us,
                );
                match oracle {
                    Ok(_) => {
                        let reservation = runtime.unwrap();
                        ledger
                            .complete(
                                reservation,
                                MonotonicMicros::new(started_at_us + airtime_us),
                                airtime_us,
                            )
                            .unwrap();
                    }
                    Err(FrequencyOccupancyError::ChannelOccupancyExceeded { .. }) => {
                        let runtime_exceeded = matches!(
                            runtime,
                            Err(OccupancyError::ChannelOccupancyExceeded { .. })
                        );
                        prop_assert!(runtime_exceeded);
                        break;
                    }
                    Err(error) => prop_assert!(false, "unexpected oracle error: {error:?}"),
                }
                started_at_us += airtime_us;
            }
        }
    }
}
