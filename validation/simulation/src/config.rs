use std::fmt;

use crate::fault::FaultPlan;
use crate::TopologyConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityField {
    Endpoints,
    EndpointReceiveQueue,
    PendingDeliveries,
    Trace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualMediumConfigError {
    ZeroCapacity(CapacityField),
    TooManyEndpoints { requested: usize, maximum: usize },
}

impl fmt::Display for VirtualMediumConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity(field) => write!(formatter, "{field:?} capacity must be nonzero"),
            Self::TooManyEndpoints { requested, maximum } => write!(
                formatter,
                "endpoint capacity {requested} exceeds the representable maximum {maximum}",
            ),
        }
    }
}

impl std::error::Error for VirtualMediumConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualMediumConfig {
    pub(crate) topology: TopologyConfig,
    pub(crate) max_endpoints: usize,
    pub(crate) endpoint_receive_queue: usize,
    pub(crate) pending_deliveries: usize,
    pub(crate) trace_capacity: usize,
    pub(crate) fault_plan: FaultPlan,
}

impl VirtualMediumConfig {
    pub fn new(
        topology: TopologyConfig,
        max_endpoints: usize,
        endpoint_receive_queue: usize,
        pending_deliveries: usize,
        trace_capacity: usize,
        fault_plan: FaultPlan,
    ) -> Result<Self, VirtualMediumConfigError> {
        for (field, capacity) in [
            (CapacityField::Endpoints, max_endpoints),
            (CapacityField::EndpointReceiveQueue, endpoint_receive_queue),
            (CapacityField::PendingDeliveries, pending_deliveries),
            (CapacityField::Trace, trace_capacity),
        ] {
            if capacity == 0 {
                return Err(VirtualMediumConfigError::ZeroCapacity(field));
            }
        }
        let representable_endpoints = usize::from(u16::MAX) + 1;
        if max_endpoints > representable_endpoints {
            return Err(VirtualMediumConfigError::TooManyEndpoints {
                requested: max_endpoints,
                maximum: representable_endpoints,
            });
        }
        Ok(Self {
            topology,
            max_endpoints,
            endpoint_receive_queue,
            pending_deliveries,
            trace_capacity,
            fault_plan,
        })
    }
}
