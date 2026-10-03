#![no_std]

mod domain_primitives;
mod state_machines;

pub use domain_primitives::{
    Connection, DeviceId, DeviceLabel, DeviceLabelError, DisconnectionReason, Enrollment,
    EnrollmentFailure, InterfacePageRequest, InterfaceRefreshFailure, MAX_DEVICE_LABEL_BYTES,
    PairingCandidate,
};
pub use state_machines::{
    AdvanceUsbPairingDiscovery, AdvanceUsbPairingDiscoveryError, AdvanceUsbPairingDiscoveryOutcome,
    BeginConnection, BeginConnectionError, BeginConnectionOutcome, BeginEnrollment,
    BeginEnrollmentError, BeginEnrollmentOutcome, CancelEnrollment, CancelEnrollmentOutcome,
    ClearUsbPairingCandidates, ClearUsbPairingCandidatesOutcome, CloseInterfaceInventory,
    CloseInterfaceInventoryOutcome, CompleteEnrollment, CompleteEnrollmentOutcome,
    ConfirmConnection, ConfirmConnectionOutcome, ConnectionState, CreateDevice, CreateDeviceError,
    CreateDeviceOutcome, DeviceRegistry, DeviceRegistryCreationError, DeviceSnapshot,
    EndConnection, EndConnectionOutcome, EnrollmentState, FailEnrollment, FailEnrollmentOutcome,
    ForgetDevice, ForgetDeviceOutcome, InterfaceInventory, InterfaceInventorySnapshot,
    InterfaceInventoryStatus, InterfaceRefreshFailed, InterfaceRefreshFailedOutcome, ListDevices,
    ListDevicesOutcome, ObserveUsbPairingAvailability, ObserveUsbPairingAvailabilityError,
    ObserveUsbPairingAvailabilityOutcome, ReadDevice, ReadDeviceOutcome, ReadInterfaces,
    ReadUsbPairingCandidates, ReceiveInterfacePage, ReceiveInterfacePageOutcome, RefreshInterfaces,
    RefreshInterfacesError, RefreshInterfacesOutcome, RenameDevice, RenameDeviceOutcome,
    SelectUsbPairingCandidate, SelectUsbPairingCandidateOutcome, UsbPairingCandidatesSnapshot,
    UsbPairingDiscovery,
};
