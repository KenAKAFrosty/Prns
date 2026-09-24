#![forbid(unsafe_code)]

//! Host-native, deterministic hardware seams for exercising the production runtime.

mod config;
mod fault;
mod interface;
mod medium;
mod time;
mod trace;

pub use config::{CapacityField, VirtualMediumConfig, VirtualMediumConfigError};
pub use fault::{
    FaultPlan, FaultPlanError, TransmissionAction, TransmissionOrdinal, TransmissionRule,
};
pub use interface::VirtualInterface;
pub use medium::{AttachError, EndpointId, VirtualMedium};
pub use time::{AdvanceError, AdvanceReport, SimulationDurationInTicks, SimulationTick};
pub use trace::{DeliveryCopy, MediumEvent, ReceptionDropReason, TraceSnapshot};

#[cfg(test)]
mod tests;
