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
    CreateDeviceOutcome, DeviceInterfaces, DeviceRegistry, DeviceRegistryCreationError,
    DeviceSnapshot, EndConnection, EndConnectionOutcome, EnrollmentState, FailEnrollment,
    FailEnrollmentOutcome, ForgetDevice, ForgetDeviceOutcome, InterfaceInventory,
    InterfaceInventorySnapshot, InterfaceInventoryStatus, InterfaceRefreshFailed,
    InterfaceRefreshFailedOutcome, ListDevices, ListDevicesOutcome, ObserveUsbPairingAvailability,
    ObserveUsbPairingAvailabilityError, ObserveUsbPairingAvailabilityOutcome, ReadDevice,
    ReadDeviceInterfaces, ReadDeviceInterfacesOutcome, ReadDeviceOutcome, ReadInterfaces,
    ReadInventoryConnection, ReadUsbPairingCandidates, ReceiveInterfacePage,
    ReceiveInterfacePageOutcome, RefreshInterfaces, RefreshInterfacesError,
    RefreshInterfacesOutcome, RenameDevice, RenameDeviceOutcome, SelectUsbPairingCandidate,
    SelectUsbPairingCandidateOutcome, SynchronizeDeviceInterfaces,
    SynchronizeDeviceInterfacesOutcome, UsbPairingCandidatesSnapshot, UsbPairingDiscovery,
};

pub use domain_primitives::{RememberedDevice, RememberedPairing};
pub use state_machines::{
    ReadRememberedDevices, ReadRememberedDevicesOutcome, RestoreRememberedDevice,
    RestoreRememberedDeviceError, RestoreRememberedDeviceOutcome,
};

pub use state_machines::{PrepareEnrollmentCompletion, PrepareEnrollmentCompletionOutcome};

pub use state_machines::{
    EnrollmentProtocolResult, EnrollmentSettlement, EnrollmentSettlementSnapshot,
    EnrollmentSettlementStatus, ObserveEnrollmentResult, ObserveEnrollmentResultOutcome,
    ReadEnrollmentSettlement, RecordEnrollmentPersistence, RecordEnrollmentPersistenceOutcome,
    RetryEnrollmentPersistence, RetryEnrollmentPersistenceOutcome,
};

pub use state_machines::{
    ReviewUsbEnrollmentOffer, ReviewUsbEnrollmentOfferError, ReviewUsbEnrollmentOfferOutcome,
    UsbEnrollmentApproval,
};
