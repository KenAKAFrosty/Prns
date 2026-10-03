#[cfg(test)]
mod behavior;
mod protocol;
mod routing;
#[cfg(test)]
mod tests;

use hopspot_hub_core::{DeviceId, DeviceInterfaces};

pub use protocol::{
    DeviceSessionEvent, DeviceSessionInput, DeviceSessionMessage, DeviceSessionRoute,
    DeviceSessionRoutingError, DeviceSessionSnapshot,
};

pub struct DeviceSessionSwitchboard<const CAPACITY: usize> {
    device: DeviceId,
    interfaces: DeviceInterfaces<CAPACITY>,
}

impl<const CAPACITY: usize> DeviceSessionSwitchboard<CAPACITY> {
    pub fn new(device: DeviceId) -> Self {
        Self {
            device,
            interfaces: DeviceInterfaces::new(device),
        }
    }
}
