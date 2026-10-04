use super::*;

#[derive(Debug)]
pub enum PersistEnrollmentError {
    Store {
        completion: Box<CompleteEnrollment>,
        source: DeviceStoreError,
    },
    CompletionInvariant {
        outcome: Box<CompleteEnrollmentOutcome>,
    },
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum PersistEnrollmentOutcome {
    Recorded {
        enrollment: Enrollment,
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

impl DeviceStore {
    pub fn persist_enrollment<const CAPACITY: usize>(
        &mut self,
        registry: &mut DeviceRegistry,
        completion: CompleteEnrollment,
    ) -> Result<PersistEnrollmentOutcome, PersistEnrollmentError> {
        self.persist_enrollment_with::<CAPACITY>(registry, completion, Self::save_records)
    }

    fn persist_enrollment_with<const CAPACITY: usize>(
        &mut self,
        registry: &mut DeviceRegistry,
        completion: CompleteEnrollment,
        save: fn(&mut Self, &[RememberedDevice]) -> Result<(), DeviceStoreError>,
    ) -> Result<PersistEnrollmentOutcome, PersistEnrollmentError> {
        let (completion, devices) = match registry
            .step(PrepareEnrollmentCompletion::<CAPACITY> { completion })
        {
            PrepareEnrollmentCompletionOutcome::Prepared {
                completion,
                devices,
            } => (completion, devices),
            PrepareEnrollmentCompletionOutcome::MissingDevice { rejected } => {
                return Ok(PersistEnrollmentOutcome::MissingDevice { rejected });
            }
            PrepareEnrollmentCompletionOutcome::StaleEnrollment { rejected } => {
                return Ok(PersistEnrollmentOutcome::StaleEnrollment { rejected });
            }
            PrepareEnrollmentCompletionOutcome::TargetMismatch { rejected } => {
                return Ok(PersistEnrollmentOutcome::TargetMismatch { rejected });
            }
            PrepareEnrollmentCompletionOutcome::TargetAlreadyPaired { rejected, device } => {
                return Ok(PersistEnrollmentOutcome::TargetAlreadyPaired { rejected, device });
            }
            PrepareEnrollmentCompletionOutcome::InsufficientCapacity { rejected, required } => {
                return Ok(PersistEnrollmentOutcome::InsufficientCapacity { rejected, required });
            }
        };
        if let Err(source) = save(self, &devices) {
            return Err(PersistEnrollmentError::Store {
                completion: Box::new(completion),
                source,
            });
        }
        settle_completion(registry.step(completion))
    }
}

fn settle_completion(
    outcome: CompleteEnrollmentOutcome,
) -> Result<PersistEnrollmentOutcome, PersistEnrollmentError> {
    match outcome {
        CompleteEnrollmentOutcome::Recorded { enrollment } => {
            Ok(PersistEnrollmentOutcome::Recorded { enrollment })
        }
        outcome @ (CompleteEnrollmentOutcome::MissingDevice { .. }
        | CompleteEnrollmentOutcome::StaleEnrollment { .. }
        | CompleteEnrollmentOutcome::TargetMismatch { .. }
        | CompleteEnrollmentOutcome::TargetAlreadyPaired { .. }) => {
            Err(PersistEnrollmentError::CompletionInvariant {
                outcome: Box::new(outcome),
            })
        }
    }
}

#[cfg(test)]
mod tests;
