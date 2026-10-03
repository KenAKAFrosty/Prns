use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableUpdateWithOutcome};

use super::DeviceRegistry;
use crate::domain_primitives::{DeviceId, DeviceLabel};

#[derive(Debug, PartialEq, Eq)]
pub struct RenameDevice {
    pub device: DeviceId,
    pub label: DeviceLabel,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum RenameDeviceOutcome {
    Renamed { device: DeviceId },
    MissingDevice { rejected: RenameDevice },
}

impl StepInputOf<DeviceRegistry> for RenameDevice {
    type Outcome = RenameDeviceOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        match registry.devices.update_with(
            WarpId::new(self.device.0),
            self.label,
            |mut row, label| {
                *row.label_mut() = label;
            },
        ) {
            WarpTableUpdateWithOutcome::Updated { .. } => RenameDeviceOutcome::Renamed {
                device: self.device,
            },
            WarpTableUpdateWithOutcome::Absent { context: label, .. } => {
                RenameDeviceOutcome::MissingDevice {
                    rejected: RenameDevice {
                        device: self.device,
                        label,
                    },
                }
            }
        }
    }
}
