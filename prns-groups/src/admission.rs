//! Domain-derived admission on the existing exclusive native owner lane.
use crate::{Error, Intent, RequestId};

/// Domain-derived access. Recovery may finish a proven admitted effect; it never
/// authorizes fresh domain execution or fresh retained intent acceptance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Pure local reads without admitting new domain effects.
    History,
    Recovery,
    LocalMutation,
    ServiceMutation {
        request: RequestId,
    },
}
pub(crate) fn mutation_access(request: &crate::Request) -> Access {
    match request.intent {
        Intent::DeleteLocal { .. } | Intent::DismissInvitation { .. } => Access::LocalMutation,
        _ => Access::ServiceMutation {
            request: request.id,
        },
    }
}

/// Settlement of this live execution lease, not disposal of durable uncertainty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settlement {
    NotDispatched,
    Completed,
    Rejected,
    /// The already committed unresolved row continues to own recovery/exclusion.
    Retained,
}
/// Domain-derived identity of one carrier effect, separate from user request
/// tokens and from the exact physical OperationTicket used to recover it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarrierEffectId(pub(crate) [u8; 32]);
impl CarrierEffectId {
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Recovery can validate and settle only the original carrier permit already
/// held by the embedding. Neither method may allocate a replacement permit.
pub trait CarrierRecovery {
    fn validate_carrier(&mut self, _effect: CarrierEffectId) -> Result<(), Error> {
        Err(Error::Configuration)
    }
    /// A failed validation/settlement keeps the original permit and key guard.
    fn settle_carrier(
        &mut self,
        _effect: CarrierEffectId,
        _settlement: Settlement,
    ) -> Result<(), Error> {
        Err(Error::Configuration)
    }
}

/// A trusted assembly's real admission/exclusion gate. Permits are owned exact
/// leases, never boolean authorization snapshots. Normal handler completion
/// explicitly returns the permit; panic/drop must not fabricate settlement.
pub trait Authorization: CarrierRecovery {
    type Permit;
    fn acquire(&mut self, access: Access) -> Result<Self::Permit, Error>;
    fn settle(&mut self, permit: Self::Permit, settlement: Settlement) -> Result<(), Error>;
    /// Check the captured network generation and reserve the original owned
    /// permit in the embedding's single carrier slot BEFORE returning success.
    /// An occupied slot is rejected before admitting another mutation. The
    /// permit remains there through panic, caller loss and storage uncertainty.
    fn acquire_carrier(&mut self, _effect: CarrierEffectId) -> Result<(), Error> {
        Err(Error::Configuration)
    }
}
/// Explicit authority for catalog-free owners only. Catalog-managed assembly
/// must supply its tracked permits and must never select this fallback.
pub struct Unmanaged;
impl CarrierRecovery for Unmanaged {}
impl Authorization for Unmanaged {
    type Permit = ();
    fn acquire(&mut self, _: Access) -> Result<(), Error> {
        Ok(())
    }
    fn settle(&mut self, _: (), _: Settlement) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
