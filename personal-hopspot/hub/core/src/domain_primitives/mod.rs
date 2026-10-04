mod connection;
mod device;
mod enrollment;
mod interface_inventory;
mod pairing_candidate;

pub use connection::{Connection, DisconnectionReason};
pub use device::{DeviceId, DeviceLabel, DeviceLabelError, MAX_DEVICE_LABEL_BYTES};
pub use enrollment::{Enrollment, EnrollmentFailure};
pub use interface_inventory::{InterfacePageRequest, InterfaceRefreshFailure};
pub use pairing_candidate::PairingCandidate;

mod remembered_device;
pub use remembered_device::{RememberedDevice, RememberedPairing};
