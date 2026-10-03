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

#[cfg(test)]
mod tests;
