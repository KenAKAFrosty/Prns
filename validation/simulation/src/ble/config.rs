use std::fmt;

use crate::TopologyConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleCapacityField {
    Radios,
    ObservationQueue,
    EmissionsPerAdvance,
    Trace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleMediumConfigError {
    ZeroCapacity(BleCapacityField),
}

impl fmt::Display for BleMediumConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity(field) => write!(formatter, "{field:?} capacity must be nonzero"),
        }
    }
}

impl std::error::Error for BleMediumConfigError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BleMediumConfig {
    pub(crate) topology: TopologyConfig,
    pub(crate) max_radios: usize,
    pub(crate) observation_queue: usize,
    pub(crate) max_emissions_per_advance: usize,
    pub(crate) trace_capacity: usize,
}

impl BleMediumConfig {
    pub fn new(
        topology: TopologyConfig,
        max_radios: usize,
        observation_queue: usize,
        max_emissions_per_advance: usize,
        trace_capacity: usize,
    ) -> Result<Self, BleMediumConfigError> {
        for (field, capacity) in [
            (BleCapacityField::Radios, max_radios),
            (BleCapacityField::ObservationQueue, observation_queue),
            (
                BleCapacityField::EmissionsPerAdvance,
                max_emissions_per_advance,
            ),
            (BleCapacityField::Trace, trace_capacity),
        ] {
            if capacity == 0 {
                return Err(BleMediumConfigError::ZeroCapacity(field));
            }
        }
        Ok(Self {
            topology,
            max_radios,
            observation_queue,
            max_emissions_per_advance,
            trace_capacity,
        })
    }
}
