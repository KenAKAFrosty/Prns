mod circuits;
mod controller_installation;
mod fittings;
mod reactors;
mod runtime;
mod session_supervisor;
mod switchboards;

pub use controller_installation::{ControllerInstallation, ControllerInstallationError};
pub use fittings::{
    PrnsDeviceCompletion, PrnsDeviceFitting, PrnsDeviceIncoming, PrnsDeviceOutgoing,
    PrnsDeviceQueueCapacityError, PrnsDeviceSubmission, PrnsDeviceWork, PrnsDeviceWorker,
    PrnsDeviceWorkerError, PrnsDeviceWorkerIncoming, PrnsDeviceWorkerOutgoing, PrnsFittingError,
    PrnsInventoryTransport,
};
pub use runtime::{NativeHubRuntime, prepare_native_hub};
pub use switchboards::{
    DeviceSessionEvent, DeviceSessionInput, DeviceSessionMessage, DeviceSessionRoute,
    DeviceSessionRoutingError, DeviceSessionSnapshot, DeviceSessionSwitchboard,
};

pub use circuits::{MioDeviceSessionCircuit, MioDeviceSessionTurn};
pub use reactors::{
    MioSessionPoll, MioSessionReaction, MioSessionReactor, MioSessionSender, MioSessionSubmission,
    MioSessionWakeFailure,
};

pub use circuits::{
    DeviceDriverFailure, DeviceDriverReaction, DeviceDriverReactor, DeviceDriverTurn,
    DeviceSessionDriver, DeviceSessionExit, DeviceSessionHandle, DeviceSessionIntent,
    DeviceSessionRuntime, DeviceSessionSubmission, DeviceSessionUpdate, prepare_device_session,
};

pub use session_supervisor::{SessionStopReason, SupervisedSessionExit, supervise_device_session};

pub use switchboards::{
    UsbDiscoveryEvent, UsbDiscoveryInput, UsbDiscoveryIntent, UsbDiscoveryMessage,
    UsbDiscoveryRoutingError, UsbDiscoverySwitchboard, UsbDiscoveryUpdate,
};

mod usb_discovery;
pub use usb_discovery::{
    NativeUsbDiscoveryEvent, NativeUsbDiscoveryRuntime, UsbDiscoveryFailure, UsbDiscoveryHandle,
    prepare_native_usb_discovery,
};
