#![forbid(unsafe_code)]

//! Host-native, deterministic hardware seams for exercising the production runtime.

mod config;
mod fault;
mod interface;
#[cfg(feature = "controlled-time")]
mod manual_time;
mod medium;
mod seeded;
mod stepping;
mod time;
mod topology;
mod trace;

pub mod ble;

pub use config::{CapacityField, VirtualMediumConfig, VirtualMediumConfigError};
pub use fault::{
    FaultPlan, FaultPlanError, TransmissionAction, TransmissionOrdinal, TransmissionRule,
};
pub use interface::VirtualInterface;
#[cfg(feature = "controlled-time")]
pub use manual_time::{
    ManualAdvance, ManualMedium, ManualTaskAdmissionError, ManualTaskId, ManualTaskPoll,
    ManualTaskRunner, ManualTimeDriver, ManualTimeError, ManualTimeSnapshot,
};
pub use medium::{AttachError, EndpointId, VirtualMedium};
pub use seeded::{
    RatePerMillion, RatePerMillionError, SeededFaultProfile, SeededFaultProfileError,
    SeededFaultRecipe, SimulationSeed, SEEDED_FAULT_ALGORITHM_VERSION,
};
pub use stepping::MediumSchedule;
pub use time::{AdvanceError, AdvanceReport, SimulationDurationInTicks, SimulationTick};
pub use topology::{Reachability, TopologyConfig, TopologyError, TopologyMutation};
pub use trace::{DeliveryCopy, MediumEvent, ReceptionDropReason, TraceSnapshot};

#[cfg(test)]
mod tests;
