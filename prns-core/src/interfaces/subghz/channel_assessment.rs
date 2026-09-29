#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelAssessmentPolicy {
    noise_percentile: u8,
    interference_margin_db: i16,
    threshold_ceiling_dbm: i16,
    persistent_soft_busy_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAssessmentPolicyError {
    InvalidNoisePercentile { percentile: u8 },
    EmptyPersistentSoftBusyPeriod,
}

impl ChannelAssessmentPolicy {
    pub const fn new(
        noise_percentile: u8,
        interference_margin_db: i16,
        threshold_ceiling_dbm: i16,
        persistent_soft_busy_us: u64,
    ) -> Result<Self, ChannelAssessmentPolicyError> {
        if noise_percentile > 100 {
            return Err(ChannelAssessmentPolicyError::InvalidNoisePercentile {
                percentile: noise_percentile,
            });
        }
        if persistent_soft_busy_us == 0 {
            return Err(ChannelAssessmentPolicyError::EmptyPersistentSoftBusyPeriod);
        }
        Ok(Self {
            noise_percentile,
            interference_margin_db,
            threshold_ceiling_dbm,
            persistent_soft_busy_us,
        })
    }

    pub const fn turbo() -> Self {
        Self {
            noise_percentile: 20,
            interference_margin_db: 11,
            threshold_ceiling_dbm: -83,
            persistent_soft_busy_us: 2_500_000,
        }
    }

    pub const fn noise_percentile(self) -> u8 {
        self.noise_percentile
    }

    pub const fn interference_margin_db(self) -> i16 {
        self.interference_margin_db
    }

    pub const fn threshold_ceiling_dbm(self) -> i16 {
        self.threshold_ceiling_dbm
    }

    pub const fn persistent_soft_busy_us(self) -> u64 {
        self.persistent_soft_busy_us
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelSample {
    DemodulatorBusy,
    Rssi { dbm: i16 },
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyEvidence {
    Demodulator,
    Rssi { dbm: i16, threshold_dbm: i16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAssessment {
    Learning {
        samples_collected: usize,
        samples_required: usize,
    },
    Clear {
        noise_floor_dbm: i16,
        threshold_dbm: i16,
    },
    Busy {
        evidence: BusyEvidence,
    },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAssessmentError {
    EmptyChannelSet,
    EmptySampleWindow,
    ChannelOutsideSet {
        channel_index: usize,
        channel_count: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SoftBusyState {
    Absent,
    ObservedAt { monotonic_us: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelNoiseFloorBank<const N: usize, const S: usize> {
    policy: ChannelAssessmentPolicy,
    samples: [[i16; S]; N],
    lengths: [usize; N],
    cursors: [usize; N],
    soft_busy: [SoftBusyState; N],
}

impl<const N: usize, const S: usize> ChannelNoiseFloorBank<N, S> {
    pub const fn new(policy: ChannelAssessmentPolicy) -> Result<Self, ChannelAssessmentError> {
        if N == 0 {
            return Err(ChannelAssessmentError::EmptyChannelSet);
        }
        if S == 0 {
            return Err(ChannelAssessmentError::EmptySampleWindow);
        }
        Ok(Self {
            policy,
            samples: [[0; S]; N],
            lengths: [0; N],
            cursors: [0; N],
            soft_busy: [SoftBusyState::Absent; N],
        })
    }

    pub fn observe(
        &mut self,
        channel_index: usize,
        monotonic_us: u64,
        sample: ChannelSample,
    ) -> Result<ChannelAssessment, ChannelAssessmentError> {
        if channel_index >= N {
            return Err(ChannelAssessmentError::ChannelOutsideSet {
                channel_index,
                channel_count: N,
            });
        }
        match sample {
            ChannelSample::DemodulatorBusy => {
                self.soft_busy[channel_index] = SoftBusyState::Absent;
                Ok(ChannelAssessment::Busy {
                    evidence: BusyEvidence::Demodulator,
                })
            }
            ChannelSample::Unavailable => Ok(ChannelAssessment::Unknown),
            ChannelSample::Rssi { dbm } => self.observe_rssi(channel_index, monotonic_us, dbm),
        }
    }

    pub fn noise_floor_dbm(
        &self,
        channel_index: usize,
    ) -> Result<ChannelNoiseFloor, ChannelAssessmentError> {
        if channel_index >= N {
            return Err(ChannelAssessmentError::ChannelOutsideSet {
                channel_index,
                channel_count: N,
            });
        }
        if self.lengths[channel_index] < S {
            return Ok(ChannelNoiseFloor::Learning {
                samples_collected: self.lengths[channel_index],
                samples_required: S,
            });
        }
        let floor_dbm = self.calibrated_floor_dbm(channel_index);
        Ok(ChannelNoiseFloor::Calibrated {
            floor_dbm,
            threshold_dbm: self.threshold_dbm(floor_dbm),
        })
    }

    fn observe_rssi(
        &mut self,
        channel_index: usize,
        monotonic_us: u64,
        dbm: i16,
    ) -> Result<ChannelAssessment, ChannelAssessmentError> {
        let completes_calibration = self.lengths[channel_index] < S;
        if completes_calibration {
            self.push(channel_index, dbm);
            if self.lengths[channel_index] < S {
                return Ok(ChannelAssessment::Learning {
                    samples_collected: self.lengths[channel_index],
                    samples_required: S,
                });
            }
        }
        let floor_dbm = self.calibrated_floor_dbm(channel_index);
        let threshold_dbm = self.threshold_dbm(floor_dbm);
        if dbm < threshold_dbm {
            self.soft_busy[channel_index] = SoftBusyState::Absent;
            if !completes_calibration {
                self.push(channel_index, dbm);
            }
            return Ok(ChannelAssessment::Clear {
                noise_floor_dbm: floor_dbm,
                threshold_dbm,
            });
        }
        if dbm < self.policy.threshold_ceiling_dbm {
            let first_observed_us = match self.soft_busy[channel_index] {
                SoftBusyState::Absent => {
                    self.soft_busy[channel_index] = SoftBusyState::ObservedAt { monotonic_us };
                    monotonic_us
                }
                SoftBusyState::ObservedAt { monotonic_us } => monotonic_us,
            };
            if monotonic_us.saturating_sub(first_observed_us) >= self.policy.persistent_soft_busy_us
            {
                self.reset(channel_index);
                self.push(channel_index, dbm);
                return Ok(ChannelAssessment::Learning {
                    samples_collected: 1,
                    samples_required: S,
                });
            }
        } else {
            self.soft_busy[channel_index] = SoftBusyState::Absent;
        }
        Ok(ChannelAssessment::Busy {
            evidence: BusyEvidence::Rssi { dbm, threshold_dbm },
        })
    }

    fn calibrated_floor_dbm(&self, channel_index: usize) -> i16 {
        let mut ordered = self.samples[channel_index];
        ordered.sort_unstable();
        let index = (S - 1) * self.policy.noise_percentile as usize / 100;
        ordered[index]
    }

    const fn threshold_dbm(&self, floor_dbm: i16) -> i16 {
        let threshold_dbm = floor_dbm.saturating_add(self.policy.interference_margin_db);
        if threshold_dbm < self.policy.threshold_ceiling_dbm {
            threshold_dbm
        } else {
            self.policy.threshold_ceiling_dbm
        }
    }

    fn push(&mut self, channel_index: usize, dbm: i16) {
        self.samples[channel_index][self.cursors[channel_index]] = dbm;
        self.cursors[channel_index] = (self.cursors[channel_index] + 1) % S;
        self.lengths[channel_index] = self.lengths[channel_index].saturating_add(1).min(S);
    }

    fn reset(&mut self, channel_index: usize) {
        self.lengths[channel_index] = 0;
        self.cursors[channel_index] = 0;
        self.soft_busy[channel_index] = SoftBusyState::Absent;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelNoiseFloor {
    Learning {
        samples_collected: usize,
        samples_required: usize,
    },
    Calibrated {
        floor_dbm: i16,
        threshold_dbm: i16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hopping_channels_learn_independent_noise_floors() {
        let mut bank =
            ChannelNoiseFloorBank::<2, 4>::new(ChannelAssessmentPolicy::turbo()).unwrap();
        for sample in [-110, -109, -108, -107] {
            bank.observe(0, 0, ChannelSample::Rssi { dbm: sample })
                .unwrap();
        }
        assert_eq!(
            bank.noise_floor_dbm(0),
            Ok(ChannelNoiseFloor::Calibrated {
                floor_dbm: -110,
                threshold_dbm: -99,
            })
        );
        assert_eq!(
            bank.noise_floor_dbm(1),
            Ok(ChannelNoiseFloor::Learning {
                samples_collected: 0,
                samples_required: 4,
            })
        );
    }

    #[test]
    fn unavailable_and_uncalibrated_channels_fail_closed() {
        let mut bank =
            ChannelNoiseFloorBank::<1, 2>::new(ChannelAssessmentPolicy::turbo()).unwrap();
        assert_eq!(
            bank.observe(0, 0, ChannelSample::Unavailable),
            Ok(ChannelAssessment::Unknown)
        );
        assert_eq!(
            bank.observe(0, 1, ChannelSample::Rssi { dbm: -110 }),
            Ok(ChannelAssessment::Learning {
                samples_collected: 1,
                samples_required: 2,
            })
        );
    }

    #[test]
    fn persistent_soft_busy_relearns_only_its_channel() {
        let policy = ChannelAssessmentPolicy::new(20, 11, -83, 100).unwrap();
        let mut bank = ChannelNoiseFloorBank::<2, 2>::new(policy).unwrap();
        for channel_index in 0..2 {
            bank.observe(channel_index, 0, ChannelSample::Rssi { dbm: -110 })
                .unwrap();
            bank.observe(channel_index, 1, ChannelSample::Rssi { dbm: -109 })
                .unwrap();
        }
        assert!(matches!(
            bank.observe(0, 10, ChannelSample::Rssi { dbm: -95 }),
            Ok(ChannelAssessment::Busy { .. })
        ));
        assert_eq!(
            bank.observe(0, 110, ChannelSample::Rssi { dbm: -95 }),
            Ok(ChannelAssessment::Learning {
                samples_collected: 1,
                samples_required: 2,
            })
        );
        assert!(matches!(
            bank.noise_floor_dbm(1),
            Ok(ChannelNoiseFloor::Calibrated { .. })
        ));
    }
}
