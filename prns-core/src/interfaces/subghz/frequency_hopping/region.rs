use super::super::{Frequency, RegulatoryRegion};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationWindow(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationWindowError {
    Empty,
}

impl ObservationWindow {
    pub const fn from_micros(micros: u64) -> Result<Self, ObservationWindowError> {
        if micros == 0 {
            return Err(ObservationWindowError::Empty);
        }
        Ok(Self(micros))
    }

    pub const fn micros(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_known_nonzero(micros: u64) -> Self {
        assert!(micros != 0);
        Self(micros)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaximumChannelOccupancy(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaximumChannelOccupancyError {
    Empty,
}

impl MaximumChannelOccupancy {
    pub const fn from_micros(micros: u64) -> Result<Self, MaximumChannelOccupancyError> {
        if micros == 0 {
            return Err(MaximumChannelOccupancyError::Empty);
        }
        Ok(Self(micros))
    }

    pub const fn micros(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_known_nonzero(micros: u64) -> Self {
        assert!(micros != 0);
        Self(micros)
    }
}

pub trait HoppingRegion<const N: usize> {
    fn radio_region(&self) -> RegulatoryRegion;

    fn channels(&self) -> &[Frequency; N];

    fn observation_window(&self) -> ObservationWindow;

    fn maximum_channel_occupancy(&self) -> MaximumChannelOccupancy;
}
