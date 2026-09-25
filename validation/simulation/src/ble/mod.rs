//! Deterministic Bluetooth LE discovery semantics shared by every host adapter.

mod advertisement;
mod backend;
mod config;
mod connection;
mod discovery;
mod gatt;
mod medium;
mod radio_id;
mod trace;

pub use advertisement::{
    BleAdvertisement, BleAdvertisementError, BleAdvertisingParameters,
    BleAdvertisingParametersError,
};
pub use backend::{
    VirtualBleBackend, VirtualBleBackendConfig, VirtualBleBackendConfigError,
    VirtualBleBackendLimits, VirtualBleDisconnectReport, VirtualBleError, VirtualBleLab,
    VirtualBleLink, VirtualBleLinkConfig,
};
pub use config::{BleCapacityField, BleMediumConfig, BleMediumConfigError};
pub use discovery::{BleDiscoveredPeer, BleDiscoverySnapshot};
pub use gatt::{VirtualBleSink, VirtualBleSource, VirtualGattConfig, VirtualGattConfigError};
pub use medium::{
    BleAdvanceError, BleAdvanceReport, BleObservation, BleRadioMutation, BleSimulationError,
    VirtualBleMedium,
};
pub use personal_rns::interfaces::bluetooth_auto::{
    BleAddress, BleRoleCapabilities, RadioMode as BleRadioPower, ScanningMode as BleScanState,
};
pub use radio_id::BleRadioId;
pub use trace::{BleObservationDropReason, BleSimulationEvent, BleTraceSnapshot};

#[cfg(test)]
mod backend_tests;
#[cfg(test)]
mod connection_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;
