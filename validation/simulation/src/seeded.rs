use std::fmt;

use crate::fault::{FaultPlan, TransmissionOrdinal, TransmissionRule};
use crate::time::SimulationDurationInTicks;

pub const SEEDED_FAULT_ALGORITHM_VERSION: u8 = 1;

const RATE_SCALE: u32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SimulationSeed(u64);

impl SimulationSeed {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RatePerMillion(u32);

impl RatePerMillion {
    pub const NEVER: Self = Self(0);
    pub const ALWAYS: Self = Self(RATE_SCALE);

    pub const fn new(value: u32) -> Result<Self, RatePerMillionError> {
        if value > RATE_SCALE {
            return Err(RatePerMillionError {
                value_was_too_large: value,
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatePerMillionError {
    pub value_was_too_large: u32,
}

impl fmt::Display for RatePerMillionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "rate {} exceeds the per-million maximum {}",
            self.value_was_too_large, RATE_SCALE,
        )
    }
}

impl std::error::Error for RatePerMillionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeededFaultProfile {
    drop_rate: RatePerMillion,
    duplicate_rate: RatePerMillion,
    delay_rate: RatePerMillion,
    maximum_delay: SimulationDurationInTicks,
}

impl SeededFaultProfile {
    pub fn new(
        drop_rate: RatePerMillion,
        duplicate_rate: RatePerMillion,
        delay_rate: RatePerMillion,
        maximum_delay: SimulationDurationInTicks,
    ) -> Result<Self, SeededFaultProfileError> {
        let total = u64::from(drop_rate.get())
            + u64::from(duplicate_rate.get())
            + u64::from(delay_rate.get());
        if total > u64::from(RATE_SCALE) {
            return Err(SeededFaultProfileError::ActionRatesExceedScale { total });
        }
        if delay_rate != RatePerMillion::NEVER && maximum_delay == SimulationDurationInTicks::ZERO {
            return Err(SeededFaultProfileError::DelayRequiresNonZeroMaximum);
        }
        Ok(Self {
            drop_rate,
            duplicate_rate,
            delay_rate,
            maximum_delay,
        })
    }

    #[must_use]
    pub const fn drop_rate(self) -> RatePerMillion {
        self.drop_rate
    }

    #[must_use]
    pub const fn duplicate_rate(self) -> RatePerMillion {
        self.duplicate_rate
    }

    #[must_use]
    pub const fn delay_rate(self) -> RatePerMillion {
        self.delay_rate
    }

    #[must_use]
    pub const fn maximum_delay(self) -> SimulationDurationInTicks {
        self.maximum_delay
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeededFaultProfileError {
    ActionRatesExceedScale { total: u64 },
    DelayRequiresNonZeroMaximum,
}

impl fmt::Display for SeededFaultProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActionRatesExceedScale { total } => write!(
                formatter,
                "mutually exclusive action rates total {total}; maximum is {RATE_SCALE}",
            ),
            Self::DelayRequiresNonZeroMaximum => {
                formatter.write_str("a nonzero delay rate requires a nonzero maximum delay")
            }
        }
    }
}

impl std::error::Error for SeededFaultProfileError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeededFaultRecipe {
    algorithm_version: u8,
    seed: SimulationSeed,
    profile: SeededFaultProfile,
    transmission_count: u32,
}

impl SeededFaultRecipe {
    #[must_use]
    pub const fn new(
        seed: SimulationSeed,
        profile: SeededFaultProfile,
        transmission_count: u32,
    ) -> Self {
        Self {
            algorithm_version: SEEDED_FAULT_ALGORITHM_VERSION,
            seed,
            profile,
            transmission_count,
        }
    }

    #[must_use]
    pub const fn algorithm_version(self) -> u8 {
        self.algorithm_version
    }

    #[must_use]
    pub const fn seed(self) -> SimulationSeed {
        self.seed
    }

    #[must_use]
    pub const fn profile(self) -> SeededFaultProfile {
        self.profile
    }

    #[must_use]
    pub const fn transmission_count(self) -> u32 {
        self.transmission_count
    }

    #[must_use]
    pub fn materialize(self) -> FaultPlan {
        let mut entropy = StableEntropy::new(self.seed);
        let mut rules = Vec::new();
        for index in 0..self.transmission_count {
            let ordinal = TransmissionOrdinal::new(u64::from(index));
            let roll = entropy.below(u64::from(RATE_SCALE)) as u32;
            let drop_end = self.profile.drop_rate.get();
            let duplicate_end = drop_end + self.profile.duplicate_rate.get();
            let delay_end = duplicate_end + self.profile.delay_rate.get();
            let rule = if roll < drop_end {
                Some(TransmissionRule::drop(ordinal))
            } else if roll < duplicate_end {
                let mut first = entropy.up_to_inclusive(self.profile.maximum_delay.get());
                let mut second = entropy.up_to_inclusive(self.profile.maximum_delay.get());
                if first > second {
                    core::mem::swap(&mut first, &mut second);
                }
                Some(TransmissionRule::duplicate(
                    ordinal,
                    SimulationDurationInTicks::from_ticks(first),
                    SimulationDurationInTicks::from_ticks(second),
                ))
            } else if roll < delay_end {
                Some(TransmissionRule::delay(
                    ordinal,
                    SimulationDurationInTicks::from_ticks(
                        entropy.nonzero_up_to(self.profile.maximum_delay.get()),
                    ),
                ))
            } else {
                None
            };
            if let Some(rule) = rule {
                rules.push(rule);
            }
        }
        FaultPlan::from_canonical_rules(rules)
    }
}

/// SplitMix64 is fixed here as a file-format-like replay contract, not as cryptography.
struct StableEntropy {
    state: u64,
}

impl StableEntropy {
    const INCREMENT: u64 = 0x9E37_79B9_7F4A_7C15;

    const fn new(seed: SimulationSeed) -> Self {
        Self { state: seed.0 }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(Self::INCREMENT);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    fn below(&mut self, exclusive_upper: u64) -> u64 {
        debug_assert!(exclusive_upper > 0, "bounded entropy needs a nonzero range");
        let rejection_floor = exclusive_upper.wrapping_neg() % exclusive_upper;
        loop {
            let value = self.next();
            if value >= rejection_floor {
                return value % exclusive_upper;
            }
        }
    }

    fn up_to_inclusive(&mut self, maximum: u64) -> u64 {
        match maximum.checked_add(1) {
            Some(exclusive_upper) => self.below(exclusive_upper),
            None => self.next(),
        }
    }

    fn nonzero_up_to(&mut self, maximum: u64) -> u64 {
        debug_assert!(maximum > 0, "nonzero entropy needs a nonzero maximum");
        if maximum == u64::MAX {
            loop {
                let value = self.next();
                if value != 0 {
                    return value;
                }
            }
        }
        self.below(maximum) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fault::TransmissionAction;

    fn rate(value: u32) -> RatePerMillion {
        match RatePerMillion::new(value) {
            Ok(rate) => rate,
            Err(error) => unreachable!("test rate is valid: {error}"),
        }
    }

    fn profile(drop: u32, duplicate: u32, delay: u32, maximum_delay: u64) -> SeededFaultProfile {
        match SeededFaultProfile::new(
            rate(drop),
            rate(duplicate),
            rate(delay),
            SimulationDurationInTicks::from_ticks(maximum_delay),
        ) {
            Ok(profile) => profile,
            Err(error) => unreachable!("test profile is valid: {error}"),
        }
    }

    #[test]
    fn splitmix64_sequence_is_a_stable_replay_contract() {
        let mut entropy = StableEntropy::new(SimulationSeed::new(0));
        assert_eq!(entropy.next(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(entropy.next(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(entropy.next(), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn rates_and_profiles_reject_invalid_whole_values() {
        assert_eq!(
            RatePerMillion::new(RATE_SCALE + 1),
            Err(RatePerMillionError {
                value_was_too_large: RATE_SCALE + 1,
            }),
        );
        assert!(matches!(
            SeededFaultProfile::new(
                rate(500_000),
                rate(500_000),
                rate(1),
                SimulationDurationInTicks::from_ticks(1),
            ),
            Err(SeededFaultProfileError::ActionRatesExceedScale { .. }),
        ));
        assert_eq!(
            SeededFaultProfile::new(
                RatePerMillion::NEVER,
                RatePerMillion::NEVER,
                rate(1),
                SimulationDurationInTicks::ZERO,
            ),
            Err(SeededFaultProfileError::DelayRequiresNonZeroMaximum),
        );
    }

    #[test]
    fn the_same_recipe_materializes_the_same_canonical_plan() {
        let recipe = SeededFaultRecipe::new(
            SimulationSeed::new(0xC0FFEE),
            profile(200_000, 300_000, 400_000, 7),
            64,
        );
        assert_eq!(recipe.algorithm_version(), SEEDED_FAULT_ALGORITHM_VERSION);
        let first = recipe.materialize();
        let second = recipe.materialize();
        assert_eq!(first, second);
        assert!(first
            .rules()
            .windows(2)
            .all(|pair| pair[0].ordinal() < pair[1].ordinal()));
    }

    #[test]
    fn certain_actions_materialize_without_probability_ambiguity() {
        let dropped =
            SeededFaultRecipe::new(SimulationSeed::new(1), profile(RATE_SCALE, 0, 0, 0), 3)
                .materialize();
        assert!(dropped
            .rules()
            .iter()
            .all(|rule| rule.action() == TransmissionAction::Drop));

        let delayed =
            SeededFaultRecipe::new(SimulationSeed::new(2), profile(0, 0, RATE_SCALE, 1), 3)
                .materialize();
        assert!(delayed.rules().iter().all(|rule| {
            rule.action()
                == TransmissionAction::Delay {
                    by: SimulationDurationInTicks::from_ticks(1),
                }
        }));
    }
}
