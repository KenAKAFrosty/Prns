use super::{OperationRejection, RedbBackend, RedbError};

/// Capacity kept available for higher-priority writes by this backend lease.
/// These are portions of the persisted hard limits, not additional capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedbReserve {
    pub objects: usize,
    pub bytes: usize,
    pub receipts: usize,
}

pub(crate) enum Admission {
    Full,
    Reserved {
        objects: usize,
        bytes: usize,
        receipts: usize,
    },
}

impl Admission {
    pub(crate) fn check(
        &self,
        objects: usize,
        bytes: usize,
        receipts: usize,
    ) -> Result<(), OperationRejection> {
        match self {
            Self::Full => Ok(()),
            Self::Reserved {
                objects: max_objects,
                bytes: max_bytes,
                receipts: max_receipts,
            } => {
                if objects > *max_objects || bytes > *max_bytes || receipts > *max_receipts {
                    Err(OperationRejection::ReservedCapacity)
                } else {
                    Ok(())
                }
            }
        }
    }
}

impl RedbBackend<'_> {
    /// Admit new commits only when their final usage leaves this reserve intact.
    /// Exact committed retries, reads, recovery and receipt retirement retain
    /// their normal behavior, even if earlier management writes used the reserve.
    /// The setting applies only to this lease; opening again uses the hard limits.
    pub fn reserve_capacity(mut self, reserve: RedbReserve) -> Result<Self, RedbError> {
        self.admission = Admission::Reserved {
            objects: self
                .limits
                .max_live_objects
                .checked_sub(reserve.objects)
                .ok_or(RedbError::Configuration)?,
            bytes: self
                .limits
                .max_live_bytes
                .checked_sub(reserve.bytes)
                .ok_or(RedbError::Configuration)?,
            receipts: self
                .limits
                .max_receipts
                .checked_sub(reserve.receipts)
                .ok_or(RedbError::Configuration)?,
        };
        Ok(self)
    }
}
