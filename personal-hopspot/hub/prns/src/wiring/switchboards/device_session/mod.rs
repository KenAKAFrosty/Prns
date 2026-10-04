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
    controls: hopspot_hub_core::DeviceControls,
    device: DeviceId,
    interfaces: DeviceInterfaces<CAPACITY>,
}

impl<const CAPACITY: usize> DeviceSessionSwitchboard<CAPACITY> {
    pub fn new(device: DeviceId) -> Self {
        Self {
            controls: hopspot_hub_core::DeviceControls::new(device),
            device,
            interfaces: DeviceInterfaces::new(device),
        }
    }
}
