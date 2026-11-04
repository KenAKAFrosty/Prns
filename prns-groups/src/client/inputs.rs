//! One selected allocation budget per submission lane, shared by all clients.
use super::{Command, Job};
use crate::{ApiError, Error, Intent, Targets};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// Local admission memory policy. These bounds cover the owned job and its
/// dynamic input capacities while queued or executing. They do not set group
/// membership/profile limits or bound query results and executor scratch space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupClientLimits"))]
pub struct ClientLimits {
    pub max_job_bytes: u64,
    pub max_pending_job_bytes: u64,
}
impl Default for ClientLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
impl ClientLimits {
    pub const DEFAULT: Self = Self {
        max_job_bytes: 1024 * 1024,
        max_pending_job_bytes: 8 * 1024 * 1024,
    };
    /// Required input allowance for one owned carrier payload, including this
    /// library's actual fixed Job representation. Other dynamic fields, if any,
    /// must be included in payload_bytes; allocation capacity is charged later.
    pub fn required_job_bytes(payload_bytes: u64) -> Result<u64, ApiError> {
        let payload = usize::try_from(payload_bytes).map_err(|_| ApiError::InvalidInput)?;
        let bytes = payload
            .checked_add(std::mem::size_of::<Job>())
            .ok_or(ApiError::InvalidInput)?;
        u64::try_from(bytes).map_err(|_| ApiError::InvalidInput)
    }
    pub fn validate(self) -> Result<(), ApiError> {
        let job = usize::try_from(self.max_job_bytes).map_err(|_| ApiError::InvalidInput)?;
        let pending =
            usize::try_from(self.max_pending_job_bytes).map_err(|_| ApiError::InvalidInput)?;
        if job < std::mem::size_of::<Job>() || pending < job {
            return Err(ApiError::InvalidInput);
        }
        Ok(())
    }
}

/// The existing owner retains this budget beside its submission lane. Cloning
/// shares the same accounting; constructing another Client never creates one.
/// A dropped reply waiter cannot release an admitted job's input reservation.
#[derive(Clone)]
pub struct InputBudget(Arc<State>);
struct State {
    max_job: usize,
    max_pending: usize,
    used: AtomicUsize,
}
impl InputBudget {
    /// The immutable selection shared by every handle on this owner lane.
    pub fn limits(&self) -> ClientLimits {
        ClientLimits {
            max_job_bytes: self.0.max_job as u64,
            max_pending_job_bytes: self.0.max_pending as u64,
        }
    }
    pub fn new(limits: ClientLimits) -> Result<Self, ApiError> {
        limits.validate()?;
        Ok(Self(Arc::new(State {
            max_job: usize::try_from(limits.max_job_bytes).map_err(|_| ApiError::InvalidInput)?,
            max_pending: usize::try_from(limits.max_pending_job_bytes)
                .map_err(|_| ApiError::InvalidInput)?,
            used: AtomicUsize::new(0),
        })))
    }
    pub(super) fn admit(&self, command: &Command) -> Result<InputLease, Error> {
        let bytes = input_bytes(command)?;
        if bytes > self.0.max_job {
            return Err(Error::InvalidInput);
        }
        self.0
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.0.max_pending)
            })
            .map_err(|_| Error::Busy)?;
        Ok(InputLease {
            budget: Arc::clone(&self.0),
            bytes,
        })
    }
}
impl Default for InputBudget {
    fn default() -> Self {
        // The known defaults fit every supported native target. Avoid a panic
        // path in default construction while dynamic choices remain checked.
        Self(Arc::new(State {
            max_job: ClientLimits::DEFAULT.max_job_bytes as usize,
            max_pending: ClientLimits::DEFAULT.max_pending_job_bytes as usize,
            used: AtomicUsize::new(0),
        }))
    }
}
pub(crate) struct InputLease {
    budget: Arc<State>,
    bytes: usize,
}
impl Drop for InputLease {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

fn input_bytes(command: &Command) -> Result<usize, Error> {
    let dynamic = match command {
        Command::SubmitRetained { intent, .. }
        | Command::Execute {
            request: crate::Request { intent, .. },
            ..
        } => intent_heap_bytes(intent)?,
        Command::EnrollmentHandle { frame, .. } => frame.capacity(),
        Command::EnrollmentComplete { response, .. } => response.capacity(),
        Command::Receive { wire, .. } => wire.capacity(),
        #[cfg(feature = "text")]
        Command::Messages { .. } => 0,
        Command::Subscribe
        | Command::ProfileSupport(_)
        | Command::GroupScope(_)
        | Command::Identity
        | Command::RetainedOperation(_)
        | Command::RetainedOperations { .. }
        | Command::DismissRetained(_)
        | Command::Invitation { .. }
        | Command::Invitations { .. }
        | Command::EnrollmentWork { .. }
        | Command::Config
        | Command::Conversations { .. }
        | Command::Conversation { .. }
        | Command::Members { .. }
        | Command::Records { .. }
        | Command::Record { .. }
        | Command::Progress { .. }
        | Command::DeliveryTargets { .. }
        | Command::Changes { .. }
        | Command::LoadCursor(_)
        | Command::SaveCursor(_, _)
        | Command::Maintain(_)
        | Command::RepairReceipts
        | Command::Snapshot
        | Command::PendingDelivery { .. }
        | Command::Receipt(_)
        | Command::Inventory { .. }
        | Command::Envelope(_)
        | Command::Known { .. } => 0,
    };
    std::mem::size_of::<Job>()
        .checked_add(dynamic)
        .ok_or(Error::InvalidInput)
}

#[cfg(test)]
mod tests;

pub(crate) fn intent_heap_bytes(intent: &Intent) -> Result<usize, Error> {
    Ok(match intent {
        Intent::Create { title, .. } => title.capacity(),
        Intent::Publish(publication) => {
            let targets = match &publication.targets {
                Targets::Explicit(targets) => targets
                    .capacity()
                    .checked_mul(std::mem::size_of::<prns_groups_core::Endpoint>())
                    .ok_or(Error::InvalidInput)?,
                Targets::OtherMembers => 0,
            };
            publication
                .body
                .capacity()
                .checked_add(targets)
                .ok_or(Error::InvalidInput)?
        }
        Intent::Invite { .. }
        | Intent::Respond { .. }
        | Intent::CancelInvitation { .. }
        | Intent::DismissInvitation { .. }
        | Intent::RemoveMember { .. }
        | Intent::Leave { .. }
        | Intent::Close { .. }
        | Intent::DeleteLocal { .. } => 0,
    })
}
