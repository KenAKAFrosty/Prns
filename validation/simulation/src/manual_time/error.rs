use std::{fmt, time::Duration};

use crate::ble::BleAdvanceError;
use crate::{AdvanceError, SimulationTick};

#[derive(Debug)]
pub enum ManualTimeError {
    RuntimeBuild(std::io::Error),
    InvalidTickDuration {
        requested: Duration,
    },
    InsideRuntime,
    SpawnedTasks {
        count: usize,
    },
    ClockDrift {
        expected: Duration,
        observed: Duration,
    },
    MediumDrift {
        expected: SimulationTick,
        observed: SimulationTick,
    },
    BeforeCurrent {
        current: SimulationTick,
        requested: SimulationTick,
    },
    ClockRange {
        tick: SimulationTick,
    },
    Frames(AdvanceError),
    Ble(BleAdvanceError),
}

impl fmt::Display for ManualTimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeBuild(error) => write!(formatter, "cannot build manual runtime: {error}"),
            Self::InvalidTickDuration { requested } => write!(formatter, "tick duration {requested:?} must be a nonzero whole number of milliseconds fitting u64"),
            Self::InsideRuntime => formatter.write_str("manual time driving requires a synchronous caller outside Tokio"),
            Self::SpawnedTasks { count } => write!(formatter, "manual time driving does not support {count} live spawned tasks"),
            Self::ClockDrift { expected, observed } => write!(formatter, "runtime clock changed outside the driver: expected {expected:?}, observed {observed:?}"),
            Self::MediumDrift { expected, observed } => write!(formatter, "medium clock changed outside the driver: expected {}, observed {}", expected.get(), observed.get()),
            Self::BeforeCurrent { current, requested } => write!(formatter, "cannot move manual time backward from {} to {}", current.get(), requested.get()),
            Self::ClockRange { tick } => write!(formatter, "tick {} exceeds the runtime clock's representable range", tick.get()),
            Self::Frames(error) => write!(formatter, "frame advance refused: {error}"),
            Self::Ble(error) => write!(formatter, "BLE advance refused: {error}"),
        }
    }
}

impl std::error::Error for ManualTimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RuntimeBuild(error) => Some(error),
            Self::Frames(error) => Some(error),
            Self::Ble(error) => Some(error),
            Self::InvalidTickDuration { .. }
            | Self::InsideRuntime
            | Self::SpawnedTasks { .. }
            | Self::ClockDrift { .. }
            | Self::MediumDrift { .. }
            | Self::BeforeCurrent { .. }
            | Self::ClockRange { .. } => None,
        }
    }
}
