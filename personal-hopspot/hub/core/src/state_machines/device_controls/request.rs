use super::DeviceControls;
use crate::DeviceControlCommand;
use crate::DeviceControlRequest;
use pipecircuit::StepInputOf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestDeviceControl {
    pub command: DeviceControlCommand,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RequestDeviceControlOutcome {
    Requested { request: DeviceControlRequest },
    Busy { pending: DeviceControlRequest },
    NotConnected,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RequestDeviceControlError {
    IdentifiersExhausted,
}

impl StepInputOf<DeviceControls> for RequestDeviceControl {
    type Outcome = Result<RequestDeviceControlOutcome, RequestDeviceControlError>;

    fn step(self, control: &mut DeviceControls) -> Self::Outcome {
        let Some(connection) = control.connection else {
            return Ok(RequestDeviceControlOutcome::NotConnected);
        };
        if let Some(pending) = control.pending {
            return Ok(RequestDeviceControlOutcome::Busy { pending });
        }
        let Some(generation) = control.next_request else {
            return Err(RequestDeviceControlError::IdentifiersExhausted);
        };
        let request = DeviceControlRequest {
            connection,
            generation,
            command: self.command,
        };
        control.next_request = generation.checked_add(1);
        control.pending = Some(request);
        Ok(RequestDeviceControlOutcome::Requested { request })
    }
}
