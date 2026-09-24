use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimulationTick(u64);

impl SimulationTick {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn from_ticks(ticks: u64) -> Self {
        Self(ticks)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn checked_add(self, duration: SimulationDurationInTicks) -> Option<Self> {
        match self.0.checked_add(duration.0) {
            Some(tick) => Some(Self(tick)),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimulationDurationInTicks(u64);

impl SimulationDurationInTicks {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn from_ticks(ticks: u64) -> Self {
        Self(ticks)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvanceError {
    BeforeCurrent {
        current: SimulationTick,
        requested: SimulationTick,
    },
    Overflow {
        current: SimulationTick,
        by: SimulationDurationInTicks,
    },
}

impl fmt::Display for AdvanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeCurrent { current, requested } => write!(
                formatter,
                "cannot move simulation time backward from {} to {}",
                current.get(),
                requested.get(),
            ),
            Self::Overflow { current, by } => write!(
                formatter,
                "advancing simulation time {} by {} ticks overflows",
                current.get(),
                by.get(),
            ),
        }
    }
}

impl std::error::Error for AdvanceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvanceReport {
    pub from: SimulationTick,
    pub to: SimulationTick,
    pub receptions_queued: usize,
    pub receptions_dropped: usize,
}

impl AdvanceReport {
    #[must_use]
    pub const fn settled(self) -> usize {
        self.receptions_queued + self.receptions_dropped
    }
}
