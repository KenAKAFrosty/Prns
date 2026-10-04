use super::DeviceControls;
use crate::{ConnectionState, DeviceControlRequest, DeviceRegistry, ReadDevice, ReadDeviceOutcome};
use pipecircuit::{StateMachine, StepInputOf};

pub struct SynchronizeDeviceControls<'registry> {
    pub registry: &'registry mut DeviceRegistry,
}

impl StepInputOf<DeviceControls> for SynchronizeDeviceControls<'_> {
    type Outcome = Option<DeviceControlRequest>;

    fn step(self, control: &mut DeviceControls) -> Self::Outcome {
        let connection = match self.registry.step(ReadDevice {
            device: control.device,
        }) {
            ReadDeviceOutcome::Found { device } => match device.connection {
                ConnectionState::Connected { connection, .. } => Some(connection),
                ConnectionState::NotConnected
                | ConnectionState::Connecting { .. }
                | ConnectionState::Disconnected { .. } => None,
            },
            ReadDeviceOutcome::MissingDevice { .. } => None,
        };
        if control.connection == connection {
            return None;
        }
        control.connection = connection;
        control.pending.take()
    }
}

pub struct SettleDeviceControl {
    pub request: DeviceControlRequest,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SettleDeviceControlOutcome {
    Settled { request: DeviceControlRequest },
    StaleRequest { request: DeviceControlRequest },
}

impl StepInputOf<DeviceControls> for SettleDeviceControl {
    type Outcome = SettleDeviceControlOutcome;

    fn step(self, control: &mut DeviceControls) -> Self::Outcome {
        if control.pending != Some(self.request) {
            return SettleDeviceControlOutcome::StaleRequest {
                request: self.request,
            };
        }
        control.pending = None;
        SettleDeviceControlOutcome::Settled {
            request: self.request,
        }
    }
}
