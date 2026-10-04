extern crate alloc;

mod wiring;

pub use wiring::{
    ControllerInstallation, ControllerInstallationError, NativeHubRuntime, prepare_native_hub,
};

mod participants;

pub use participants::{PrnsDevice, PrnsDeviceIn, PrnsDeviceOut};
pub use wiring::{
    PrnsDeviceCompletion, PrnsDeviceFitting, PrnsDeviceIncoming, PrnsDeviceOutgoing,
    PrnsDeviceQueueCapacityError, PrnsDeviceSubmission, PrnsDeviceWork, PrnsDeviceWorker,
    PrnsDeviceWorkerError, PrnsDeviceWorkerIncoming, PrnsDeviceWorkerOutgoing, PrnsFittingError,
    PrnsInventoryTransport,
};

pub use wiring::{
    DeviceSessionEvent, DeviceSessionInput, DeviceSessionMessage, DeviceSessionRoute,
    DeviceSessionRoutingError, DeviceSessionSnapshot, DeviceSessionSwitchboard,
};

pub use wiring::{
    MioDeviceSessionCircuit, MioDeviceSessionTurn, MioSessionPoll, MioSessionReaction,
    MioSessionReactor, MioSessionSender, MioSessionSubmission, MioSessionWakeFailure,
};

pub use wiring::{
    DeviceDriverFailure, DeviceDriverReaction, DeviceDriverReactor, DeviceDriverTurn,
    DeviceSessionDriver, DeviceSessionExit, DeviceSessionHandle, DeviceSessionIntent,
    DeviceSessionRuntime, DeviceSessionSubmission, DeviceSessionUpdate, prepare_device_session,
};

pub use wiring::{SessionStopReason, SupervisedSessionExit, supervise_device_session};

#[cfg(test)]
mod tests;

pub use wiring::{
    UsbDiscoveryEvent, UsbDiscoveryInput, UsbDiscoveryIntent, UsbDiscoveryMessage,
    UsbDiscoveryRoutingError, UsbDiscoverySwitchboard, UsbDiscoveryUpdate,
};

pub use wiring::{
    NativeUsbDiscoveryEvent, NativeUsbDiscoveryRuntime, UsbDiscoveryFailure, UsbDiscoveryHandle,
    prepare_native_usb_discovery,
};

pub use wiring::{
    DeviceArchiveError, DeviceStore, DeviceStoreError, LoadDevicesOutcome, SaveDevicesOutcome,
};

pub use wiring::{PersistEnrollmentError, PersistEnrollmentOutcome};
