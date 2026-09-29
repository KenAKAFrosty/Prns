use std::fmt;

use personal_rns::interfaces::bluetooth_auto::{
    contains_service, encode_advertisement, BleRoleCapabilities, MAX_ADVERTISEMENT_LEN,
};

use crate::SimulationDurationInTicks;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleAdvertisementError {
    Empty,
    TooLong { length: usize, maximum: usize },
    ProductionEncodingFailed,
}

impl fmt::Display for BleAdvertisementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("BLE advertisement must not be empty"),
            Self::TooLong { length, maximum } => write!(
                formatter,
                "BLE advertisement is {length} bytes; maximum is {maximum}",
            ),
            Self::ProductionEncodingFailed => {
                formatter.write_str("production BLE advertisement did not fit its maximum")
            }
        }
    }
}

impl std::error::Error for BleAdvertisementError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BleAdvertisement {
    bytes: [u8; MAX_ADVERTISEMENT_LEN],
    len: u8,
}

impl BleAdvertisement {
    pub fn new(bytes: &[u8]) -> Result<Self, BleAdvertisementError> {
        if bytes.is_empty() {
            return Err(BleAdvertisementError::Empty);
        }
        if bytes.len() > MAX_ADVERTISEMENT_LEN {
            return Err(BleAdvertisementError::TooLong {
                length: bytes.len(),
                maximum: MAX_ADVERTISEMENT_LEN,
            });
        }
        let mut stored = [0; MAX_ADVERTISEMENT_LEN];
        stored[..bytes.len()].copy_from_slice(bytes);
        let Ok(len) = u8::try_from(bytes.len()) else {
            return Err(BleAdvertisementError::TooLong {
                length: bytes.len(),
                maximum: MAX_ADVERTISEMENT_LEN,
            });
        };
        Ok(Self { bytes: stored, len })
    }

    pub fn reticulum(
        role_capabilities: BleRoleCapabilities,
    ) -> Result<Self, BleAdvertisementError> {
        let mut bytes = [0; MAX_ADVERTISEMENT_LEN];
        let len = encode_advertisement(&mut bytes, role_capabilities)
            .ok_or(BleAdvertisementError::ProductionEncodingFailed)?;
        Self::new(&bytes[..len])
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    #[must_use]
    pub fn contains_reticulum_service(&self) -> bool {
        contains_service(self.as_bytes())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleAdvertisingParametersError {
    ZeroInterval,
}

impl fmt::Display for BleAdvertisingParametersError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroInterval => formatter.write_str("BLE advertising interval must be nonzero"),
        }
    }
}

impl std::error::Error for BleAdvertisingParametersError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BleAdvertisingParameters {
    advertisement: BleAdvertisement,
    interval: SimulationDurationInTicks,
}

impl BleAdvertisingParameters {
    pub fn new(
        advertisement: BleAdvertisement,
        interval: SimulationDurationInTicks,
    ) -> Result<Self, BleAdvertisingParametersError> {
        if interval == SimulationDurationInTicks::ZERO {
            return Err(BleAdvertisingParametersError::ZeroInterval);
        }
        Ok(Self {
            advertisement,
            interval,
        })
    }

    #[must_use]
    pub const fn advertisement(self) -> BleAdvertisement {
        self.advertisement
    }

    #[must_use]
    pub const fn interval(self) -> SimulationDurationInTicks {
        self.interval
    }
}
