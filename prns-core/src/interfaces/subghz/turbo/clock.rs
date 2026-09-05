use super::super::MonotonicMicros;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScheduleMicros(u64);

impl ScheduleMicros {
    pub const fn new(micros: u64) -> Self {
        Self(micros)
    }

    pub const fn micros(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedTimeSource {
    Gnss,
    AuthenticatedHost,
    AuthenticatedNetworkTime,
    SeededRtc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtcTimescale {
    PosixUnsmeared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedScheduleClock {
    observed_at: MonotonicMicros,
    schedule_at_observation: ScheduleMicros,
    uncertainty_us: u64,
    maximum_drift_ppm: u32,
    source: TrustedTimeSource,
    timescale: UtcTimescale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquiredReceivePhase {
    observed_at: MonotonicMicros,
    schedule_at_observation: ScheduleMicros,
    uncertainty_us: u64,
    maximum_drift_ppm: u32,
    observations: u8,
    distinct_channels: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockError {
    EmptyUncertainty,
    EmptyDriftBound,
    MonotonicTimeWentBackward,
    TimeRangeOverflow,
    TransmitUncertaintyExceeded { actual_us: u64, maximum_us: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockWindow {
    center_schedule_us: u64,
    uncertainty_us: u64,
}

impl ClockWindow {
    pub(crate) const fn from_center(center_schedule_us: u64, uncertainty_us: u64) -> Self {
        Self {
            center_schedule_us,
            uncertainty_us,
        }
    }

    pub const fn center_schedule_us(self) -> u64 {
        self.center_schedule_us
    }

    pub const fn uncertainty_us(self) -> u64 {
        self.uncertainty_us
    }

    pub const fn earliest_schedule_us(self) -> u64 {
        self.center_schedule_us.saturating_sub(self.uncertainty_us)
    }

    pub const fn latest_schedule_us(self) -> u64 {
        self.center_schedule_us.saturating_add(self.uncertainty_us)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustedClockUpdate {
    Continuous {
        clock: TrustedScheduleClock,
    },
    Discontinuous {
        clock: TrustedScheduleClock,
        predicted_schedule_us: u64,
        supplied_schedule_us: u64,
        tolerated_error_us: u64,
    },
}

impl TrustedClockUpdate {
    pub(crate) const fn clock(self) -> TrustedScheduleClock {
        match self {
            Self::Continuous { clock } | Self::Discontinuous { clock, .. } => clock,
        }
    }
}

impl TrustedScheduleClock {
    pub const fn new(
        observed_at: MonotonicMicros,
        utc_at_observation: ScheduleMicros,
        uncertainty_us: u64,
        maximum_drift_ppm: u32,
        source: TrustedTimeSource,
        timescale: UtcTimescale,
    ) -> Result<Self, ClockError> {
        if uncertainty_us == 0 {
            return Err(ClockError::EmptyUncertainty);
        }
        if maximum_drift_ppm == 0 {
            return Err(ClockError::EmptyDriftBound);
        }
        Ok(Self {
            observed_at,
            schedule_at_observation: utc_at_observation,
            uncertainty_us,
            maximum_drift_ppm,
            source,
            timescale,
        })
    }

    pub const fn source(self) -> TrustedTimeSource {
        self.source
    }

    pub const fn timescale(self) -> UtcTimescale {
        self.timescale
    }

    pub fn window_at(self, now: MonotonicMicros) -> Result<ClockWindow, ClockError> {
        window_at(
            self.observed_at,
            self.schedule_at_observation,
            self.uncertainty_us,
            self.maximum_drift_ppm,
            now,
        )
    }

    pub fn transmit_window_at(
        self,
        now: MonotonicMicros,
        maximum_uncertainty_us: u64,
    ) -> Result<ClockWindow, ClockError> {
        let window = self.window_at(now)?;
        if window.uncertainty_us() > maximum_uncertainty_us {
            return Err(ClockError::TransmitUncertaintyExceeded {
                actual_us: window.uncertainty_us(),
                maximum_us: maximum_uncertainty_us,
            });
        }
        Ok(window)
    }

    pub(crate) fn assess_update(
        self,
        observed_at: MonotonicMicros,
        supplied_schedule: ScheduleMicros,
        uncertainty_us: u64,
        maximum_drift_ppm: u32,
        source: TrustedTimeSource,
        timescale: UtcTimescale,
    ) -> Result<TrustedClockUpdate, ClockError> {
        let replacement = Self::new(
            observed_at,
            supplied_schedule,
            uncertainty_us,
            maximum_drift_ppm,
            source,
            timescale,
        )?;
        let predicted = self.window_at(observed_at)?;
        let error_us = predicted
            .center_schedule_us()
            .abs_diff(supplied_schedule.micros());
        let tolerated_error_us = predicted.uncertainty_us().saturating_add(uncertainty_us);
        if error_us > tolerated_error_us || self.timescale != timescale {
            return Ok(TrustedClockUpdate::Discontinuous {
                clock: replacement,
                predicted_schedule_us: predicted.center_schedule_us(),
                supplied_schedule_us: supplied_schedule.micros(),
                tolerated_error_us,
            });
        }
        Ok(TrustedClockUpdate::Continuous { clock: replacement })
    }
}

impl AcquiredReceivePhase {
    pub(crate) const fn new(
        observed_at: MonotonicMicros,
        schedule_at_observation: ScheduleMicros,
        uncertainty_us: u64,
        maximum_drift_ppm: u32,
        observations: u8,
        distinct_channels: u8,
    ) -> Result<Self, ClockError> {
        if uncertainty_us == 0 {
            return Err(ClockError::EmptyUncertainty);
        }
        if maximum_drift_ppm == 0 {
            return Err(ClockError::EmptyDriftBound);
        }
        Ok(Self {
            observed_at,
            schedule_at_observation,
            uncertainty_us,
            maximum_drift_ppm,
            observations,
            distinct_channels,
        })
    }

    pub const fn observed_at(self) -> MonotonicMicros {
        self.observed_at
    }

    pub const fn observations(self) -> u8 {
        self.observations
    }

    pub const fn distinct_channels(self) -> u8 {
        self.distinct_channels
    }

    pub fn window_at(self, now: MonotonicMicros) -> Result<ClockWindow, ClockError> {
        window_at(
            self.observed_at,
            self.schedule_at_observation,
            self.uncertainty_us,
            self.maximum_drift_ppm,
            now,
        )
    }
}

fn window_at(
    observed_at: MonotonicMicros,
    schedule_at_observation: ScheduleMicros,
    uncertainty_us: u64,
    maximum_drift_ppm: u32,
    now: MonotonicMicros,
) -> Result<ClockWindow, ClockError> {
    let Some(elapsed_us) = now.micros().checked_sub(observed_at.micros()) else {
        return Err(ClockError::MonotonicTimeWentBackward);
    };
    let Some(center_schedule_us) = schedule_at_observation.micros().checked_add(elapsed_us) else {
        return Err(ClockError::TimeRangeOverflow);
    };
    let drift_us = elapsed_us
        .saturating_mul(maximum_drift_ppm as u64)
        .div_ceil(1_000_000);
    let uncertainty_us = uncertainty_us.saturating_add(drift_us);
    if center_schedule_us.checked_add(uncertainty_us).is_none() {
        return Err(ClockError::TimeRangeOverflow);
    }
    Ok(ClockWindow::from_center(center_schedule_us, uncertainty_us))
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn clock_uncertainty_never_shrinks_during_holdover() {
        let uncertainty_us: u16 = kani::any();
        let maximum_drift_ppm: u16 = kani::any();
        let elapsed_us: u32 = kani::any();
        kani::assume(uncertainty_us > 0);
        kani::assume(maximum_drift_ppm > 0);
        let clock = TrustedScheduleClock::new(
            MonotonicMicros::new(0),
            ScheduleMicros::new(0),
            uncertainty_us as u64,
            maximum_drift_ppm as u32,
            TrustedTimeSource::Gnss,
            UtcTimescale::PosixUnsmeared,
        )
        .unwrap();
        let window = clock
            .window_at(MonotonicMicros::new(elapsed_us as u64))
            .unwrap();
        assert!(window.uncertainty_us() >= uncertainty_us as u64);
    }
}
