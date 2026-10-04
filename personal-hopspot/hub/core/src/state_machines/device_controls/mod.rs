#[cfg(test)]
mod architecture;
#[cfg(test)]
mod behavior;
mod request;
mod settlement;
#[cfg(test)]
mod tests;

use crate::{Connection, DeviceControlRequest, DeviceId};
use core::num::NonZeroU64;
use pipecircuit::{StateMachine, StepInputOf};

pub use request::{RequestDeviceControl, RequestDeviceControlError, RequestDeviceControlOutcome};
pub use settlement::{SettleDeviceControl, SettleDeviceControlOutcome, SynchronizeDeviceControls};

pub struct DeviceControls {
    device: DeviceId,
    connection: Option<Connection>,
    pending: Option<DeviceControlRequest>,
    next_request: Option<NonZeroU64>,
}

impl DeviceControls {
    pub fn new(device: DeviceId) -> Self {
        Self {
            device,
            connection: None,
            pending: None,
            next_request: Some(NonZeroU64::MIN),
        }
    }
}

impl StateMachine for DeviceControls {}

pub struct ReadDeviceControls;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceControlsSnapshot {
    pub connection: Option<Connection>,
    pub pending: Option<DeviceControlRequest>,
}

impl StepInputOf<DeviceControls> for ReadDeviceControls {
    type Outcome = DeviceControlsSnapshot;

    fn step(self, control: &mut DeviceControls) -> Self::Outcome {
        DeviceControlsSnapshot {
            connection: control.connection,
            pending: control.pending,
        }
    }
}
