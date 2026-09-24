#![forbid(unsafe_code)]

//! Host-native, deterministic hardware seams for exercising the production runtime.

mod config;
mod fault;
mod interface;
mod medium;
mod trace;

pub use config::{CapacityField, VirtualMediumConfig, VirtualMediumConfigError};
pub use fault::{FaultPlan, FaultPlanError, TransmissionOrdinal};
pub use interface::VirtualInterface;
pub use medium::{AttachError, EndpointId, VirtualMedium};
pub use trace::{MediumEvent, ReceptionDropReason, TraceSnapshot};

#[cfg(test)]
mod tests;
