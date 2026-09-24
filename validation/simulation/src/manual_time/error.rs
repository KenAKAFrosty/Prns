use std::{fmt, time::Duration};

use crate::ble::BleAdvanceError;
use crate::{AdvanceError, SimulationTick};

#[derive(Debug)]
pub enum ManualTimeError {
    RuntimeBuild(std::io::Error),
    InvalidTickDuration {
        requested: Duration,
    },
    WakeSteppingRequiresMillisecondTicks,
    InsideRuntime,
    SpawnedTasks {
        count: usize,
    },
    ReadyTasks {
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
    MediaDisagree {
        frames: SimulationTick,
        ble: SimulationTick,
    },
    BeforeCurrent {
        current: SimulationTick,
        requested: SimulationTick,
    },
    ClockRange {
        tick: SimulationTick,
    },
    Frames(AdvanceError),
    HaLow(AdvanceError),
    Ble(BleAdvanceError),
}

impl fmt::Display for ManualTimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeBuild(error) => write!(formatter, "cannot build manual runtime: {error}"),
            Self::InvalidTickDuration { requested } => write!(formatter, "tick duration {requested:?} must be a nonzero whole number of milliseconds fitting u64"),
            Self::WakeSteppingRequiresMillisecondTicks => formatter.write_str("automatic wake stepping requires one-millisecond ticks"),
            Self::InsideRuntime => formatter.write_str("manual time driving requires a synchronous caller outside Tokio"),
            Self::SpawnedTasks { count } => write!(formatter, "manual time driving does not support {count} live spawned tasks"),
            Self::ReadyTasks { count } => write!(formatter, "settle {count} ready manual tasks before advancing time"),
            Self::ClockDrift { expected, observed } => write!(formatter, "runtime clock changed outside the driver: expected {expected:?}, observed {observed:?}"),
            Self::MediumDrift { expected, observed } => write!(formatter, "medium clock changed outside the driver: expected {}, observed {}", expected.get(), observed.get()),
            Self::MediaDisagree { frames, ble } => write!(formatter, "frame and BLE clocks disagree: {} versus {}", frames.get(), ble.get()),
            Self::BeforeCurrent { current, requested } => write!(formatter, "cannot move manual time backward from {} to {}", current.get(), requested.get()),
            Self::ClockRange { tick } => write!(formatter, "tick {} exceeds the runtime clock's representable range", tick.get()),
            Self::Frames(error) => write!(formatter, "frame advance refused: {error}"),
            Self::HaLow(error) => write!(formatter, "HaLoW advance refused: {error}"),
            Self::Ble(error) => write!(formatter, "BLE advance refused: {error}"),
        }
    }
}

impl std::error::Error for ManualTimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RuntimeBuild(error) => Some(error),
            Self::Frames(error) => Some(error),
            Self::HaLow(error) => Some(error),
            Self::Ble(error) => Some(error),
            Self::InvalidTickDuration { .. }
            | Self::WakeSteppingRequiresMillisecondTicks
            | Self::InsideRuntime
            | Self::SpawnedTasks { .. }
            | Self::ReadyTasks { .. }
            | Self::ClockDrift { .. }
            | Self::MediumDrift { .. }
            | Self::MediaDisagree { .. }
            | Self::BeforeCurrent { .. }
            | Self::ClockRange { .. } => None,
        }
    }
}
