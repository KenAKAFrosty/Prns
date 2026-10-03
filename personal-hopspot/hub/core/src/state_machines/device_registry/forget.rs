use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableRemoveOutcome};

use super::{DeviceRegistry, EnrollmentState};
use crate::domain_primitives::DeviceId;

#[derive(Debug, PartialEq, Eq)]
pub struct ForgetDevice {
    pub device: DeviceId,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ForgetDeviceOutcome {
    Forgotten {
        device: DeviceId,
        enrollment: EnrollmentState,
    },
    MissingDevice {
        device: DeviceId,
    },
}

impl StepInputOf<DeviceRegistry> for ForgetDevice {
    type Outcome = ForgetDeviceOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        match registry.devices.remove(WarpId::new(self.device.0)) {
            WarpTableRemoveOutcome::Removed { removed, .. } => ForgetDeviceOutcome::Forgotten {
                device: self.device,
                enrollment: removed.enrollment,
            },
            WarpTableRemoveOutcome::Absent { .. } => ForgetDeviceOutcome::MissingDevice {
                device: self.device,
            },
        }
    }
}
