mod device_interfaces;
mod device_registry;
mod interface_inventory;
mod usb_pairing_discovery;

pub use usb_pairing_discovery::{
    AdvanceUsbPairingDiscovery, AdvanceUsbPairingDiscoveryError, AdvanceUsbPairingDiscoveryOutcome,
    ClearUsbPairingCandidates, ClearUsbPairingCandidatesOutcome, ObserveUsbPairingAvailability,
    ObserveUsbPairingAvailabilityError, ObserveUsbPairingAvailabilityOutcome,
    ReadUsbPairingCandidates, SelectUsbPairingCandidate, SelectUsbPairingCandidateOutcome,
    UsbPairingCandidatesSnapshot, UsbPairingDiscovery,
};

pub use device_registry::{
    BeginConnection, BeginConnectionError, BeginConnectionOutcome, BeginEnrollment,
    BeginEnrollmentError, BeginEnrollmentOutcome, CancelEnrollment, CancelEnrollmentOutcome,
    CompleteEnrollment, CompleteEnrollmentOutcome, ConfirmConnection, ConfirmConnectionOutcome,
    ConnectionState, CreateDevice, CreateDeviceError, CreateDeviceOutcome, DeviceRegistry,
    DeviceRegistryCreationError, DeviceSnapshot, EndConnection, EndConnectionOutcome,
    EnrollmentState, FailEnrollment, FailEnrollmentOutcome, ForgetDevice, ForgetDeviceOutcome,
    ListDevices, ListDevicesOutcome, ReadDevice, ReadDeviceOutcome, RenameDevice,
    RenameDeviceOutcome,
};

pub use interface_inventory::{
    CloseInterfaceInventory, CloseInterfaceInventoryOutcome, InterfaceInventory,
    InterfaceInventorySnapshot, InterfaceInventoryStatus, InterfaceRefreshFailed,
    InterfaceRefreshFailedOutcome, ReadInterfaces, ReadInventoryConnection, ReceiveInterfacePage,
    ReceiveInterfacePageOutcome, RefreshInterfaces, RefreshInterfacesError,
    RefreshInterfacesOutcome,
};

pub use device_interfaces::{
    DeviceInterfaces, ReadDeviceInterfaces, ReadDeviceInterfacesOutcome,
    SynchronizeDeviceInterfaces, SynchronizeDeviceInterfacesOutcome,
};
