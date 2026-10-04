use super::{ConnectionState, DeviceRecord, DeviceRegistry, EnrollmentState};
use crate::domain_primitives::{DeviceId, RememberedDevice, RememberedPairing};
use core::num::NonZeroU32;
use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableInsertOutcome};

pub struct ReadRememberedDevices<const CAPACITY: usize>;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ReadRememberedDevicesOutcome<const CAPACITY: usize> {
    Read {
        devices: heapless::Vec<RememberedDevice, CAPACITY>,
    },
    InsufficientCapacity {
        required: usize,
    },
}

impl<const CAPACITY: usize> StepInputOf<DeviceRegistry> for ReadRememberedDevices<CAPACITY> {
    type Outcome = ReadRememberedDevicesOutcome<CAPACITY>;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        let mut devices = heapless::Vec::new();
        for row in registry.devices.rows() {
            if devices
                .push(RememberedDevice {
                    label: row.label.clone(),
                    pairing: row.enrollment.remembered(),
                })
                .is_err()
            {
                return ReadRememberedDevicesOutcome::InsufficientCapacity {
                    required: registry.devices.rows().len(),
                };
            }
        }
        ReadRememberedDevicesOutcome::Read { devices }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RestoreRememberedDevice {
    pub remembered: RememberedDevice,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum RestoreRememberedDeviceOutcome {
    Restored {
        device: DeviceId,
    },
    TargetAlreadyPaired {
        rejected: RestoreRememberedDevice,
        device: DeviceId,
    },
    AtCapacity {
        rejected: RestoreRememberedDevice,
        maximum_devices: NonZeroU32,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum RestoreRememberedDeviceError {
    IdentifiersExhausted { rejected: RestoreRememberedDevice },
}

impl StepInputOf<DeviceRegistry> for RestoreRememberedDevice {
    type Outcome = Result<RestoreRememberedDeviceOutcome, RestoreRememberedDeviceError>;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        if let RememberedPairing::Paired { target } = &self.remembered.pairing
            && let Some(row) = registry.devices.rows().find(|row| {
                matches!(row.enrollment, EnrollmentState::Paired { target: existing } if existing == target)
            })
        {
            return Ok(RestoreRememberedDeviceOutcome::TargetAlreadyPaired {
                device: DeviceId(row.id.value()),
                rejected: self,
            });
        }
        let enrollment = match self.remembered.pairing {
            RememberedPairing::Unpaired => EnrollmentState::Planned,
            RememberedPairing::Paired { target } => EnrollmentState::Paired { target },
        };
        let inserted = registry.devices.insert(DeviceRecord {
            label: self.remembered.label,
            enrollment,
            connection: ConnectionState::NotConnected,
        });
        registry.restored(inserted)
    }
}

impl DeviceRegistry {
    #[expect(clippy::result_large_err)]
    pub(super) fn restored(
        &self,
        inserted: WarpTableInsertOutcome<WarpId<DeviceRecord>, DeviceRecord>,
    ) -> Result<RestoreRememberedDeviceOutcome, RestoreRememberedDeviceError> {
        match inserted {
            WarpTableInsertOutcome::Inserted { id, .. } => {
                Ok(RestoreRememberedDeviceOutcome::Restored {
                    device: DeviceId(id.value()),
                })
            }
            WarpTableInsertOutcome::Full { rejected } => {
                Ok(RestoreRememberedDeviceOutcome::AtCapacity {
                    rejected: RestoreRememberedDevice {
                        remembered: RememberedDevice {
                            label: rejected.label,
                            pairing: rejected.enrollment.remembered(),
                        },
                    },
                    maximum_devices: self.devices.capacity().value(),
                })
            }
            WarpTableInsertOutcome::IdentifiersExhausted { rejected } => {
                Err(RestoreRememberedDeviceError::IdentifiersExhausted {
                    rejected: RestoreRememberedDevice {
                        remembered: RememberedDevice {
                            label: rejected.label,
                            pairing: rejected.enrollment.remembered(),
                        },
                    },
                })
            }
            WarpTableInsertOutcome::UniqueConflict { conflict, .. } => match conflict {},
        }
    }
}

impl EnrollmentState {
    fn remembered(&self) -> RememberedPairing {
        match self {
            Self::Planned | Self::Pairing { .. } => RememberedPairing::Unpaired,
            Self::Paired { target } => RememberedPairing::Paired { target: *target },
        }
    }
}
