use core::num::NonZeroU32;
use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableInsertOutcome};

use super::{DeviceRecord, DeviceRegistry, EnrollmentState};
use crate::domain_primitives::{DeviceId, DeviceLabel};

#[derive(Debug, PartialEq, Eq)]
pub struct CreateDevice {
    pub label: DeviceLabel,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum CreateDeviceOutcome {
    Created {
        device: DeviceId,
    },
    AtCapacity {
        rejected: CreateDevice,
        maximum_devices: NonZeroU32,
    },
    IdentifiersExhausted {
        rejected: CreateDevice,
    },
}

impl StepInputOf<DeviceRegistry> for CreateDevice {
    type Outcome = CreateDeviceOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        let outcome = registry.devices.insert(DeviceRecord {
            label: self.label,
            enrollment: EnrollmentState::Planned,
        });
        registry.created(outcome)
    }
}

impl DeviceRegistry {
    pub(super) fn created(
        &self,
        outcome: WarpTableInsertOutcome<WarpId<DeviceRecord>, DeviceRecord>,
    ) -> CreateDeviceOutcome {
        match outcome {
            WarpTableInsertOutcome::Inserted { id, .. } => CreateDeviceOutcome::Created {
                device: DeviceId(id.value()),
            },
            WarpTableInsertOutcome::Full { rejected } => CreateDeviceOutcome::AtCapacity {
                rejected: CreateDevice {
                    label: rejected.label,
                },
                maximum_devices: self.devices.capacity().value(),
            },
            WarpTableInsertOutcome::IdentifiersExhausted { rejected } => {
                CreateDeviceOutcome::IdentifiersExhausted {
                    rejected: CreateDevice {
                        label: rejected.label,
                    },
                }
            }
            WarpTableInsertOutcome::UniqueConflict { conflict, .. } => match conflict {},
        }
    }
}
