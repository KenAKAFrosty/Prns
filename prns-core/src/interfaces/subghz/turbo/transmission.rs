use super::super::frequency_hopping::{
    ChannelOccupancyLimit, ChannelOccupancyLimitError, ConductedPowerDbm, HopSetError,
    MeasuredTwentyDbBandwidth, Us915HopSet, Us915HoppingModel, Us915HoppingModelError,
    Us915PowerBudget, Us915PowerBudgetError, Us915PowerInputs,
};
use super::super::{Frequency, MonotonicMicros};
use super::channel_access::FinalClearGrant;
use super::clock::{
    ClockError, ScheduleMicros, TrustedScheduleClock, TrustedTimeSource, UtcTimescale,
};
use super::frame::{encode_datagram, DatagramId, EncodedDatagram, TurboFrameError};
use super::occupancy::{OccupancyError, OccupancyReservation, TurboOccupancyLedger};
use super::profile::{TurboHardwareSupport, TurboProfileError, US915_TURBO_PHY};
use super::schedule::{
    channel_index_at, opportunity_for, supercycle_cycle_at, OpportunityRejection,
    TransmissionTimingBudget, TurboOpportunity, TURBO_BOOT_QUARANTINE_US, TURBO_CHANNEL_COUNT,
    TURBO_OCCUPANCY_LIMIT_US, US915_TURBO_CHANNELS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaximumTransmitUncertainty(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaximumTransmitUncertaintyError {
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboTransmitterInstanceId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboTransmitterInstanceIdError {
    Zero,
}

impl TurboTransmitterInstanceId {
    pub const fn new(value: u64) -> Result<Self, TurboTransmitterInstanceIdError> {
        if value == 0 {
            return Err(TurboTransmitterInstanceIdError::Zero);
        }
        Ok(Self(value))
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl MaximumTransmitUncertainty {
    pub const fn new(micros: u64) -> Result<Self, MaximumTransmitUncertaintyError> {
        if micros == 0 {
            return Err(MaximumTransmitUncertaintyError::Empty);
        }
        Ok(Self(micros))
    }

    pub const fn micros(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Us915TurboConfiguration {
    measured_bandwidth: MeasuredTwentyDbBandwidth,
    timing: TransmissionTimingBudget,
    maximum_transmit_uncertainty: MaximumTransmitUncertainty,
    power_inputs: Us915PowerInputs,
    hardware_support: TurboHardwareSupport,
    instance_id: TurboTransmitterInstanceId,
}

impl Us915TurboConfiguration {
    pub const fn new(
        measured_bandwidth: MeasuredTwentyDbBandwidth,
        timing: TransmissionTimingBudget,
        maximum_transmit_uncertainty: MaximumTransmitUncertainty,
        power_inputs: Us915PowerInputs,
        hardware_support: TurboHardwareSupport,
        instance_id: TurboTransmitterInstanceId,
    ) -> Self {
        Self {
            measured_bandwidth,
            timing,
            maximum_transmit_uncertainty,
            power_inputs,
            hardware_support,
            instance_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboTransmitterStatus {
    Quarantined { transmit_not_before_us: u64 },
    Idle,
    Reserved,
    Transmitting,
    Faulted { reason: TurboFault },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboFault {
    ClockChangedDuringRfOperation,
    ActualAirtimeExceededReservation,
    TransmissionCrossedSlotBoundary,
    CompletionBeforeStart,
    ClockUnavailableAtCompletion,
    OccupancyInvariant,
}

#[derive(Debug, PartialEq, Eq)]
enum TransmitterState {
    Idle,
    Reserved { generation: u64 },
    Transmitting { generation: u64 },
    Faulted { reason: TurboFault },
}

#[derive(Debug, PartialEq, Eq)]
pub struct PreparedTurboTransmission {
    instance_id: TurboTransmitterInstanceId,
    generation: u64,
    opportunity: TurboOpportunity,
    power: ConductedPowerDbm,
    datagram: EncodedDatagram,
    reservation: OccupancyReservation,
    prepared_at: MonotonicMicros,
    keyed_airtime_us: u64,
    required_elapsed_us: u64,
    final_clear_expires_at: MonotonicMicros,
}

impl PreparedTurboTransmission {
    pub const fn frequency(&self) -> Frequency {
        self.opportunity.frequency()
    }

    pub const fn channel_index(&self) -> usize {
        self.opportunity.channel_index()
    }

    pub const fn power(&self) -> ConductedPowerDbm {
        self.power
    }

    pub const fn keyed_airtime_us(&self) -> u64 {
        self.keyed_airtime_us
    }

    pub fn datagram(&self) -> &EncodedDatagram {
        &self.datagram
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ActiveTurboTransmission {
    instance_id: TurboTransmitterInstanceId,
    generation: u64,
    reservation: OccupancyReservation,
    started_at: MonotonicMicros,
    transmit_must_end_by_schedule_us: u64,
    reserved_airtime_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboTransmissionReport {
    channel_index: usize,
    actual_airtime_us: u64,
    completed_at: MonotonicMicros,
}

impl TurboTransmissionReport {
    pub const fn channel_index(self) -> usize {
        self.channel_index
    }

    pub const fn actual_airtime_us(self) -> u64 {
        self.actual_airtime_us
    }

    pub const fn completed_at(self) -> MonotonicMicros {
        self.completed_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockUpdateDisposition {
    Continuous,
    Discontinuous { transmit_not_before_us: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboTransmissionError {
    OccupancyLimit(ChannelOccupancyLimitError),
    RegulatoryModel(Us915HoppingModelError),
    HopSet(HopSetError),
    Profile(TurboProfileError),
    Power(Us915PowerBudgetError),
    BandwidthBelowTurboMinimum { measured_hz: u32, minimum_hz: u32 },
    Clock(ClockError),
    BootQuarantine { transmit_not_before_us: u64 },
    Opportunity(OpportunityRejection),
    Frame(TurboFrameError),
    Occupancy(OccupancyError),
    FinalClearFromWrongChannel { expected: usize, actual: usize },
    FinalClearExpired { age_us: u64, maximum_age_us: u64 },
    FinalClearFromFuture,
    TransmitterBusy,
    InvalidPreparedToken,
    InvalidActiveToken,
    RfStartBeforePreparation,
    RfStartedOnWrongChannel { expected: usize, actual: usize },
    EmptyActualAirtime,
    ActualAirtimeExceedsElapsed { actual_us: u64, elapsed_us: u64 },
    Faulted { reason: TurboFault },
}

pub struct Us915TurboTransmitter {
    state: TransmitterState,
    clock: TrustedScheduleClock,
    timing: TransmissionTimingBudget,
    maximum_transmit_uncertainty: MaximumTransmitUncertainty,
    power: Us915PowerBudget,
    occupancy: TurboOccupancyLedger,
    quarantine_until: MonotonicMicros,
    instance_id: TurboTransmitterInstanceId,
    next_generation: u64,
}

impl Us915TurboTransmitter {
    pub fn new(
        booted_at: MonotonicMicros,
        clock: TrustedScheduleClock,
        configuration: Us915TurboConfiguration,
    ) -> Result<Self, TurboTransmissionError> {
        US915_TURBO_PHY
            .validate()
            .map_err(TurboTransmissionError::Profile)?;
        US915_TURBO_PHY
            .verify_hardware(configuration.hardware_support)
            .map_err(TurboTransmissionError::Profile)?;
        if configuration.measured_bandwidth.hz() < 250_000 {
            return Err(TurboTransmissionError::BandwidthBelowTurboMinimum {
                measured_hz: configuration.measured_bandwidth.hz(),
                minimum_hz: 250_000,
            });
        }
        let occupancy_limit = ChannelOccupancyLimit::new(TURBO_OCCUPANCY_LIMIT_US)
            .map_err(TurboTransmissionError::OccupancyLimit)?;
        let model = Us915HoppingModel::new(configuration.measured_bandwidth, occupancy_limit)
            .map_err(TurboTransmissionError::RegulatoryModel)?;
        Us915HopSet::new(model, US915_TURBO_CHANNELS).map_err(TurboTransmissionError::HopSet)?;
        let power =
            Us915PowerBudget::for_hop_count(TURBO_CHANNEL_COUNT, configuration.power_inputs)
                .map_err(TurboTransmissionError::Power)?;
        Ok(Self {
            state: TransmitterState::Idle,
            clock,
            timing: configuration.timing,
            maximum_transmit_uncertainty: configuration.maximum_transmit_uncertainty,
            power,
            occupancy: TurboOccupancyLedger::new(),
            quarantine_until: MonotonicMicros::new(
                booted_at.micros().saturating_add(TURBO_BOOT_QUARANTINE_US),
            ),
            instance_id: configuration.instance_id,
            next_generation: 0,
        })
    }

    pub fn status(&self, now: MonotonicMicros) -> TurboTransmitterStatus {
        match self.state {
            TransmitterState::Idle if now < self.quarantine_until => {
                TurboTransmitterStatus::Quarantined {
                    transmit_not_before_us: self.quarantine_until.micros(),
                }
            }
            TransmitterState::Idle => TurboTransmitterStatus::Idle,
            TransmitterState::Reserved { .. } => TurboTransmitterStatus::Reserved,
            TransmitterState::Transmitting { .. } => TurboTransmitterStatus::Transmitting,
            TransmitterState::Faulted { reason } => TurboTransmitterStatus::Faulted { reason },
        }
    }

    pub const fn selected_power(&self) -> ConductedPowerDbm {
        self.power.selected()
    }

    pub fn update_clock(
        &mut self,
        observed_at: MonotonicMicros,
        supplied_schedule: ScheduleMicros,
        uncertainty_us: u64,
        maximum_drift_ppm: u32,
        source: TrustedTimeSource,
        timescale: UtcTimescale,
    ) -> Result<ClockUpdateDisposition, TurboTransmissionError> {
        let update = self
            .clock
            .assess_update(
                observed_at,
                supplied_schedule,
                uncertainty_us,
                maximum_drift_ppm,
                source,
                timescale,
            )
            .map_err(TurboTransmissionError::Clock)?;
        let discontinuous = matches!(
            update,
            super::clock::TrustedClockUpdate::Discontinuous { .. }
        );
        self.clock = update.clock();
        if !discontinuous {
            return Ok(ClockUpdateDisposition::Continuous);
        }
        self.quarantine_until = MonotonicMicros::new(
            observed_at
                .micros()
                .saturating_add(TURBO_BOOT_QUARANTINE_US),
        );
        if !matches!(self.state, TransmitterState::Idle) {
            self.state = TransmitterState::Faulted {
                reason: TurboFault::ClockChangedDuringRfOperation,
            };
        }
        Ok(ClockUpdateDisposition::Discontinuous {
            transmit_not_before_us: self.quarantine_until.micros(),
        })
    }

    pub fn prepare_after_final_clear(
        &mut self,
        grant: FinalClearGrant,
        now: MonotonicMicros,
        datagram_id: DatagramId,
        payload: &[u8],
    ) -> Result<PreparedTurboTransmission, TurboTransmissionError> {
        self.require_idle(now)?;
        let Some(grant_age_us) = now.micros().checked_sub(grant.issued_at().micros()) else {
            return Err(TurboTransmissionError::FinalClearFromFuture);
        };
        if grant_age_us > self.timing.final_clear_validity_us() {
            return Err(TurboTransmissionError::FinalClearExpired {
                age_us: grant_age_us,
                maximum_age_us: self.timing.final_clear_validity_us(),
            });
        }
        let window = self
            .clock
            .transmit_window_at(now, self.maximum_transmit_uncertainty.micros())
            .map_err(TurboTransmissionError::Clock)?;
        let cycle = supercycle_cycle_at(window.center_schedule_us());
        let datagram =
            encode_datagram(cycle, datagram_id, payload).map_err(TurboTransmissionError::Frame)?;
        let opportunity = opportunity_for(window, US915_TURBO_PHY, &datagram, self.timing)
            .map_err(TurboTransmissionError::Opportunity)?;
        if grant.channel_index() != opportunity.channel_index() {
            return Err(TurboTransmissionError::FinalClearFromWrongChannel {
                expected: opportunity.channel_index(),
                actual: grant.channel_index(),
            });
        }
        let keyed_airtime_us = datagram.keyed_airtime_us(US915_TURBO_PHY);
        let required_elapsed_us = keyed_airtime_us.saturating_add(if datagram.frame_count() == 2 {
            self.timing.interframe_us()
        } else {
            0
        });
        let reservation = self
            .occupancy
            .reserve(
                opportunity.channel_index(),
                opportunity.global_slot(),
                now,
                keyed_airtime_us,
            )
            .map_err(TurboTransmissionError::Occupancy)?;
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        self.state = TransmitterState::Reserved { generation };
        Ok(PreparedTurboTransmission {
            instance_id: self.instance_id,
            generation,
            opportunity,
            power: self.power.selected(),
            datagram,
            reservation,
            prepared_at: now,
            keyed_airtime_us,
            required_elapsed_us,
            final_clear_expires_at: MonotonicMicros::new(
                grant
                    .issued_at()
                    .micros()
                    .saturating_add(self.timing.final_clear_validity_us()),
            ),
        })
    }

    pub fn mark_rf_started(
        &mut self,
        prepared: PreparedTurboTransmission,
        actual_start: MonotonicMicros,
    ) -> Result<ActiveTurboTransmission, TurboTransmissionError> {
        if self.state
            != (TransmitterState::Reserved {
                generation: prepared.generation,
            })
            || prepared.instance_id != self.instance_id
        {
            return Err(TurboTransmissionError::InvalidPreparedToken);
        }
        if actual_start < prepared.prepared_at {
            self.release_invalid_preparation(prepared.reservation)?;
            return Err(TurboTransmissionError::RfStartBeforePreparation);
        }
        if actual_start > prepared.final_clear_expires_at {
            let age_us = actual_start.micros().saturating_sub(
                prepared
                    .final_clear_expires_at
                    .micros()
                    .saturating_sub(self.timing.final_clear_validity_us()),
            );
            self.release_invalid_preparation(prepared.reservation)?;
            return Err(TurboTransmissionError::FinalClearExpired {
                age_us,
                maximum_age_us: self.timing.final_clear_validity_us(),
            });
        }
        let window = match self
            .clock
            .transmit_window_at(actual_start, self.maximum_transmit_uncertainty.micros())
        {
            Ok(window) => window,
            Err(error) => {
                self.release_invalid_preparation(prepared.reservation)?;
                return Err(TurboTransmissionError::Clock(error));
            }
        };
        let actual_channel = channel_index_at(window.center_schedule_us());
        if actual_channel != prepared.opportunity.channel_index() {
            let expected = prepared.opportunity.channel_index();
            self.release_invalid_preparation(prepared.reservation)?;
            return Err(TurboTransmissionError::RfStartedOnWrongChannel {
                expected,
                actual: actual_channel,
            });
        }
        if window
            .latest_schedule_us()
            .saturating_add(prepared.required_elapsed_us)
            > prepared.opportunity.transmit_must_end_by_schedule_us()
        {
            self.release_invalid_preparation(prepared.reservation)?;
            return Err(TurboTransmissionError::Opportunity(
                OpportunityRejection::PacketCrossesGuardedBoundary,
            ));
        }
        self.state = TransmitterState::Transmitting {
            generation: prepared.generation,
        };
        Ok(ActiveTurboTransmission {
            instance_id: self.instance_id,
            generation: prepared.generation,
            reservation: prepared.reservation,
            started_at: actual_start,
            transmit_must_end_by_schedule_us: prepared
                .opportunity
                .transmit_must_end_by_schedule_us(),
            reserved_airtime_us: prepared.keyed_airtime_us,
        })
    }

    pub fn abort_before_rf(
        &mut self,
        prepared: PreparedTurboTransmission,
    ) -> Result<(), TurboTransmissionError> {
        if self.state
            != (TransmitterState::Reserved {
                generation: prepared.generation,
            })
            || prepared.instance_id != self.instance_id
        {
            return Err(TurboTransmissionError::InvalidPreparedToken);
        }
        self.occupancy
            .abort(prepared.reservation)
            .map_err(TurboTransmissionError::Occupancy)?;
        self.state = TransmitterState::Idle;
        Ok(())
    }

    pub fn complete(
        &mut self,
        active: ActiveTurboTransmission,
        completed_at: MonotonicMicros,
        actual_airtime_us: u64,
    ) -> Result<TurboTransmissionReport, TurboTransmissionError> {
        if self.state
            != (TransmitterState::Transmitting {
                generation: active.generation,
            })
            || active.instance_id != self.instance_id
        {
            return Err(TurboTransmissionError::InvalidActiveToken);
        }
        let Some(elapsed_us) = completed_at
            .micros()
            .checked_sub(active.started_at.micros())
        else {
            self.state = TransmitterState::Faulted {
                reason: TurboFault::CompletionBeforeStart,
            };
            return Err(TurboTransmissionError::Faulted {
                reason: TurboFault::CompletionBeforeStart,
            });
        };
        if actual_airtime_us == 0 {
            self.state = TransmitterState::Faulted {
                reason: TurboFault::OccupancyInvariant,
            };
            return Err(TurboTransmissionError::EmptyActualAirtime);
        }
        if actual_airtime_us > elapsed_us {
            self.state = TransmitterState::Faulted {
                reason: TurboFault::OccupancyInvariant,
            };
            return Err(TurboTransmissionError::ActualAirtimeExceedsElapsed {
                actual_us: actual_airtime_us,
                elapsed_us,
            });
        }
        let channel_index = active.reservation.channel_index();
        let occupancy_result =
            self.occupancy
                .complete(active.reservation, completed_at, actual_airtime_us);
        if actual_airtime_us > active.reserved_airtime_us {
            self.state = TransmitterState::Faulted {
                reason: TurboFault::ActualAirtimeExceededReservation,
            };
            return Err(TurboTransmissionError::Faulted {
                reason: TurboFault::ActualAirtimeExceededReservation,
            });
        }
        if occupancy_result.is_err() {
            self.state = TransmitterState::Faulted {
                reason: TurboFault::OccupancyInvariant,
            };
            return Err(TurboTransmissionError::Faulted {
                reason: TurboFault::OccupancyInvariant,
            });
        }
        let completion_window = match self.clock.window_at(completed_at) {
            Ok(window) => window,
            Err(_) => {
                self.state = TransmitterState::Faulted {
                    reason: TurboFault::ClockUnavailableAtCompletion,
                };
                return Err(TurboTransmissionError::Faulted {
                    reason: TurboFault::ClockUnavailableAtCompletion,
                });
            }
        };
        if completion_window.latest_schedule_us() > active.transmit_must_end_by_schedule_us {
            self.state = TransmitterState::Faulted {
                reason: TurboFault::TransmissionCrossedSlotBoundary,
            };
            return Err(TurboTransmissionError::Faulted {
                reason: TurboFault::TransmissionCrossedSlotBoundary,
            });
        }
        self.state = TransmitterState::Idle;
        Ok(TurboTransmissionReport {
            channel_index,
            actual_airtime_us,
            completed_at,
        })
    }

    fn require_idle(&self, now: MonotonicMicros) -> Result<(), TurboTransmissionError> {
        if let TransmitterState::Faulted { reason } = self.state {
            return Err(TurboTransmissionError::Faulted { reason });
        }
        if !matches!(self.state, TransmitterState::Idle) {
            return Err(TurboTransmissionError::TransmitterBusy);
        }
        if now < self.quarantine_until {
            return Err(TurboTransmissionError::BootQuarantine {
                transmit_not_before_us: self.quarantine_until.micros(),
            });
        }
        Ok(())
    }

    fn release_invalid_preparation(
        &mut self,
        reservation: OccupancyReservation,
    ) -> Result<(), TurboTransmissionError> {
        self.occupancy
            .abort(reservation)
            .map_err(TurboTransmissionError::Occupancy)?;
        self.state = TransmitterState::Idle;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn channel_keyed_airtime_us(&self, channel_index: usize) -> u64 {
        self.occupancy.channel_keyed_airtime_us(channel_index)
    }

    #[cfg(test)]
    pub(crate) fn channel_use_airtime_us(&self, channel_index: usize) -> u64 {
        self.occupancy.channel_use_airtime_us(channel_index)
    }
}
