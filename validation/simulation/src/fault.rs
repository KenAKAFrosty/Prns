use std::fmt;

use crate::time::SimulationDurationInTicks;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransmissionOrdinal(pub(crate) u64);

impl TransmissionOrdinal {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransmissionAction {
    Drop,
    Delay {
        by: SimulationDurationInTicks,
    },
    Duplicate {
        first_after: SimulationDurationInTicks,
        second_after: SimulationDurationInTicks,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransmissionRule {
    ordinal: TransmissionOrdinal,
    action: TransmissionAction,
}

impl TransmissionRule {
    #[must_use]
    pub const fn drop(ordinal: TransmissionOrdinal) -> Self {
        Self {
            ordinal,
            action: TransmissionAction::Drop,
        }
    }

    #[must_use]
    pub const fn delay(ordinal: TransmissionOrdinal, by: SimulationDurationInTicks) -> Self {
        Self {
            ordinal,
            action: TransmissionAction::Delay { by },
        }
    }

    #[must_use]
    pub const fn duplicate(
        ordinal: TransmissionOrdinal,
        first_after: SimulationDurationInTicks,
        second_after: SimulationDurationInTicks,
    ) -> Self {
        Self {
            ordinal,
            action: TransmissionAction::Duplicate {
                first_after,
                second_after,
            },
        }
    }

    #[must_use]
    pub const fn ordinal(self) -> TransmissionOrdinal {
        self.ordinal
    }

    #[must_use]
    pub const fn action(self) -> TransmissionAction {
        self.action
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultPlan {
    rules: Vec<TransmissionRule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPlanError {
    OrdinalsNotStrictlyIncreasing {
        previous: TransmissionOrdinal,
        next: TransmissionOrdinal,
    },
    DuplicateDeliveriesNotCanonical {
        ordinal: TransmissionOrdinal,
        first_after: SimulationDurationInTicks,
        second_after: SimulationDurationInTicks,
    },
}

impl fmt::Display for FaultPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OrdinalsNotStrictlyIncreasing { previous, next } => write!(
                formatter,
                "fault ordinals must be strictly increasing; {previous:?} was followed by {next:?}",
            ),
            Self::DuplicateDeliveriesNotCanonical {
                ordinal,
                first_after,
                second_after,
            } => write!(
                formatter,
                "duplicate rule {ordinal:?} delivers its second copy at {} before its first at {}",
                second_after.get(),
                first_after.get(),
            ),
        }
    }
}

impl std::error::Error for FaultPlanError {}

impl FaultPlan {
    #[must_use]
    pub const fn none() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn new(rules: Vec<TransmissionRule>) -> Result<Self, FaultPlanError> {
        for pair in rules.windows(2) {
            if pair[0].ordinal >= pair[1].ordinal {
                return Err(FaultPlanError::OrdinalsNotStrictlyIncreasing {
                    previous: pair[0].ordinal,
                    next: pair[1].ordinal,
                });
            }
        }
        for rule in &rules {
            if let TransmissionAction::Duplicate {
                first_after,
                second_after,
            } = rule.action
            {
                if first_after > second_after {
                    return Err(FaultPlanError::DuplicateDeliveriesNotCanonical {
                        ordinal: rule.ordinal,
                        first_after,
                        second_after,
                    });
                }
            }
        }
        Ok(Self { rules })
    }

    pub fn drop_transmissions(
        dropped_transmissions: Vec<TransmissionOrdinal>,
    ) -> Result<Self, FaultPlanError> {
        Self::new(
            dropped_transmissions
                .into_iter()
                .map(TransmissionRule::drop)
                .collect(),
        )
    }

    pub(crate) fn action_for(&self, ordinal: TransmissionOrdinal) -> Option<TransmissionAction> {
        self.rules
            .binary_search_by_key(&ordinal, |rule| rule.ordinal)
            .ok()
            .map(|index| self.rules[index].action)
    }
}
