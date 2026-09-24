use std::fmt;

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
    TooManyRadios { requested: usize, maximum: usize },
}

impl fmt::Display for BleMediumConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity(field) => write!(formatter, "{field:?} capacity must be nonzero"),
            Self::TooManyRadios { requested, maximum } => write!(
                formatter,
                "BLE radio capacity {requested} exceeds the representable maximum {maximum}",
            ),
        }
    }
}

impl std::error::Error for BleMediumConfigError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BleMediumConfig {
    pub(crate) max_radios: usize,
    pub(crate) observation_queue: usize,
    pub(crate) max_emissions_per_advance: usize,
    pub(crate) trace_capacity: usize,
}

impl BleMediumConfig {
    pub fn new(
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
        let maximum = usize::from(u16::MAX) + 1;
        if max_radios > maximum {
            return Err(BleMediumConfigError::TooManyRadios {
                requested: max_radios,
                maximum,
            });
        }
        Ok(Self {
            max_radios,
            observation_queue,
            max_emissions_per_advance,
            trace_capacity,
        })
    }
}
