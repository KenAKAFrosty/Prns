#[cfg(test)]
mod architecture;
#[cfg(test)]
mod behavior;
mod create;
mod enrollment;
mod forget;
mod query;
mod rename;
mod state;
#[cfg(test)]
mod tests;

use core::num::{NonZeroU32, NonZeroU64};

use pipecircuit::StateMachine;
use pipecircuit::storage::warp_table::{Capacity, WarpTable, WarpTableCreationError, WarpTableRow};

use crate::domain_primitives::DeviceLabel;
pub use create::{CreateDevice, CreateDeviceOutcome};
pub use enrollment::{
    BeginEnrollment, BeginEnrollmentOutcome, CancelEnrollment, CancelEnrollmentOutcome,
    CompleteEnrollment, CompleteEnrollmentOutcome, FailEnrollment, FailEnrollmentOutcome,
};
pub use forget::{ForgetDevice, ForgetDeviceOutcome};
pub use query::{ListDevices, ListDevicesOutcome, ReadDevice, ReadDeviceOutcome};
pub use rename::{RenameDevice, RenameDeviceOutcome};
pub use state::{DeviceSnapshot, EnrollmentState};

#[derive(WarpTableRow)]
struct DeviceRecord {
    label: DeviceLabel,
    enrollment: EnrollmentState,
}

type StorageCreationError = WarpTableCreationError<
    <DeviceRecord as WarpTableRow>::ColumnsCreationError,
    <DeviceRecord as WarpTableRow>::IndexesCreationError,
>;

#[derive(Debug)]
pub struct DeviceRegistryCreationError {
    source: StorageCreationError,
}

impl DeviceRegistryCreationError {
    fn from_storage(source: StorageCreationError) -> Self {
        Self { source }
    }

    pub fn source(&self) -> &impl core::fmt::Debug {
        &self.source
    }
}

pub struct DeviceRegistry {
    devices: WarpTable<DeviceRecord>,
    next_enrollment: Option<NonZeroU64>,
}

impl DeviceRegistry {
    pub fn try_new(maximum_devices: NonZeroU32) -> Result<Self, DeviceRegistryCreationError> {
        WarpTable::try_new(Capacity::new(maximum_devices))
            .map(|devices| Self {
                devices,
                next_enrollment: Some(NonZeroU64::MIN),
            })
            .map_err(DeviceRegistryCreationError::from_storage)
    }
}

impl StateMachine for DeviceRegistry {}
