use super::super::DeviceRegistry;
use super::CompleteEnrollment;
use super::complete::CompletionCheck;
use crate::domain_primitives::{DeviceId, RememberedDevice, RememberedPairing};
use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableGetOutcome};

#[derive(Debug, PartialEq, Eq)]
pub struct PrepareEnrollmentCompletion<const CAPACITY: usize> {
    pub completion: CompleteEnrollment,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum PrepareEnrollmentCompletionOutcome<const CAPACITY: usize> {
    Prepared {
        completion: CompleteEnrollment,
        devices: heapless::Vec<RememberedDevice, CAPACITY>,
    },
    MissingDevice {
        rejected: CompleteEnrollment,
    },
    StaleEnrollment {
        rejected: CompleteEnrollment,
    },
    TargetMismatch {
        rejected: CompleteEnrollment,
    },
    TargetAlreadyPaired {
        rejected: CompleteEnrollment,
        device: DeviceId,
    },
    InsufficientCapacity {
        rejected: CompleteEnrollment,
        required: usize,
    },
}

impl<const CAPACITY: usize> StepInputOf<DeviceRegistry> for PrepareEnrollmentCompletion<CAPACITY> {
    type Outcome = PrepareEnrollmentCompletionOutcome<CAPACITY>;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        let input = self.completion;
        let row = match registry.devices.get(WarpId::new(input.enrollment.device.0)) {
            WarpTableGetOutcome::Present { row } => row,
            WarpTableGetOutcome::Absent { .. } => {
                return Self::Outcome::MissingDevice { rejected: input };
            }
        };
        let target = match row
            .enrollment
            .check_completion(&input, registry.paired_target(&input.target))
        {
            CompletionCheck::Ready { target } => target,
            CompletionCheck::Stale => return Self::Outcome::StaleEnrollment { rejected: input },
            CompletionCheck::Mismatch => return Self::Outcome::TargetMismatch { rejected: input },
            CompletionCheck::Duplicate { device } => {
                return Self::Outcome::TargetAlreadyPaired {
                    rejected: input,
                    device,
                };
            }
        };
        let mut devices = heapless::Vec::new();
        for row in registry.devices.rows() {
            let pairing = if row.id.value() == input.enrollment.device.0 {
                RememberedPairing::Paired { target }
            } else {
                row.enrollment.remembered()
            };
            if devices
                .push(RememberedDevice {
                    label: row.label.clone(),
                    pairing,
                })
                .is_err()
            {
                return Self::Outcome::InsufficientCapacity {
                    rejected: input,
                    required: registry.devices.rows().len(),
                };
            }
        }
        Self::Outcome::Prepared {
            completion: input,
            devices,
        }
    }
}
