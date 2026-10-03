extern crate alloc;

mod fittings;
mod participants;

pub use fittings::{
    PrnsDeviceFitting, PrnsDeviceIncoming, PrnsDeviceOutgoing, PrnsDeviceWork, PrnsFittingError,
    PrnsInventoryTransport,
};
pub use participants::{PrnsDevice, PrnsDeviceIn, PrnsDeviceOut};

mod switchboards;
pub use switchboards::{
    DeviceSessionEvent, DeviceSessionInput, DeviceSessionMessage, DeviceSessionRoute,
    DeviceSessionRoutingError, DeviceSessionSnapshot, DeviceSessionSwitchboard,
};

#[cfg(test)]
mod tests;
