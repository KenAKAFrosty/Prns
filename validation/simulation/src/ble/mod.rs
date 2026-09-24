//! Deterministic Bluetooth LE discovery semantics shared by every host adapter.

mod advertisement;
mod backend;
mod config;
mod medium;
mod trace;

pub use advertisement::{
    BleAdvertisement, BleAdvertisementError, BleAdvertisingParameters,
    BleAdvertisingParametersError,
};
pub use backend::{
    VirtualBleBackend, VirtualBleBackendConfig, VirtualBleBackendConfigError, VirtualBleError,
    VirtualBleLab, VirtualBleLink, VirtualBleLinkConfig, VirtualBleSink, VirtualBleSource,
};
pub use config::{BleCapacityField, BleMediumConfig, BleMediumConfigError};
pub use medium::{
    BleAdvanceError, BleAdvanceReport, BleObservation, BleRadioId, BleRadioMutation,
    BleSimulationError, VirtualBleMedium,
};
pub use personal_rns::interfaces::bluetooth_auto::{
    BleAddress, BleRoleCapabilities, RadioMode as BleRadioPower, ScanningMode as BleScanState,
};
pub use trace::{BleObservationDropReason, BleSimulationEvent, BleTraceSnapshot};

#[cfg(test)]
mod backend_tests;
#[cfg(test)]
mod tests;
