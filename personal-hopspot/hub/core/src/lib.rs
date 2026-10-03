#![no_std]

mod domain_primitives;
mod state_machines;

pub use domain_primitives::{
    DeviceId, DeviceLabel, DeviceLabelError, Enrollment, EnrollmentFailure, MAX_DEVICE_LABEL_BYTES,
};
pub use state_machines::{
    BeginEnrollment, BeginEnrollmentOutcome, CancelEnrollment, CancelEnrollmentOutcome,
    CompleteEnrollment, CompleteEnrollmentOutcome, CreateDevice, CreateDeviceOutcome,
    DeviceRegistry, DeviceRegistryCreationError, DeviceSnapshot, EnrollmentState, FailEnrollment,
    FailEnrollmentOutcome, ForgetDevice, ForgetDeviceOutcome, ListDevices, ListDevicesOutcome,
    ReadDevice, ReadDeviceOutcome, RenameDevice, RenameDeviceOutcome,
};
