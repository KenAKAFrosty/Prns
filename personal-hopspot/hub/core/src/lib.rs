#![no_std]

mod domain_primitives;
mod state_machines;

pub use domain_primitives::{
    Connection, DeviceId, DeviceLabel, DeviceLabelError, DisconnectionReason, Enrollment,
    EnrollmentFailure, MAX_DEVICE_LABEL_BYTES,
};
pub use state_machines::{
    BeginConnection, BeginConnectionOutcome, BeginEnrollment, BeginEnrollmentOutcome,
    CancelEnrollment, CancelEnrollmentOutcome, CompleteEnrollment, CompleteEnrollmentOutcome,
    ConfirmConnection, ConfirmConnectionOutcome, ConnectionState, CreateDevice,
    CreateDeviceOutcome, DeviceRegistry, DeviceRegistryCreationError, DeviceSnapshot,
    EndConnection, EndConnectionOutcome, EnrollmentState, FailEnrollment, FailEnrollmentOutcome,
    ForgetDevice, ForgetDeviceOutcome, ListDevices, ListDevicesOutcome, ReadDevice,
    ReadDeviceOutcome, RenameDevice, RenameDeviceOutcome,
};
