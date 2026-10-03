use core::num::NonZeroU32;
use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableInsertOutcome};

use super::{ConnectionState, DeviceRecord, DeviceRegistry, EnrollmentState};
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
}

#[derive(Debug, PartialEq, Eq)]
pub enum CreateDeviceError {
    IdentifiersExhausted { rejected: CreateDevice },
}

impl StepInputOf<DeviceRegistry> for CreateDevice {
    type Outcome = Result<CreateDeviceOutcome, CreateDeviceError>;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        let outcome = registry.devices.insert(DeviceRecord {
            label: self.label,
            enrollment: EnrollmentState::Planned,
            connection: ConnectionState::NotConnected,
        });
        registry.created(outcome)
    }
}

impl DeviceRegistry {
    #[expect(clippy::result_large_err)]
    pub(super) fn created(
        &self,
        outcome: WarpTableInsertOutcome<WarpId<DeviceRecord>, DeviceRecord>,
    ) -> Result<CreateDeviceOutcome, CreateDeviceError> {
        match outcome {
            WarpTableInsertOutcome::Inserted { id, .. } => Ok(CreateDeviceOutcome::Created {
                device: DeviceId(id.value()),
            }),
            WarpTableInsertOutcome::Full { rejected } => Ok(CreateDeviceOutcome::AtCapacity {
                rejected: CreateDevice {
                    label: rejected.label,
                },
                maximum_devices: self.devices.capacity().value(),
            }),
            WarpTableInsertOutcome::IdentifiersExhausted { rejected } => {
                Err(CreateDeviceError::IdentifiersExhausted {
                    rejected: CreateDevice {
                        label: rejected.label,
                    },
                })
            }
            WarpTableInsertOutcome::UniqueConflict { conflict, .. } => match conflict {},
        }
    }
}
