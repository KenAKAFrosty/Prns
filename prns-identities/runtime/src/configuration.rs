//! Explicit durable service preparation, separate from ordinary activation.

use crate::{Authority, Error, Fault};
use prns_identities_core::{
    Action, EffectTicket, IdentityGuard, InitializedService, KeyReference, ServiceId,
};

/// Synchronous domain preparation under the embedding's exclusive dispatch gate.
///
/// Initialize only for this explicit retained action. Use its stable operation
/// token and immutable identity binding to reconcile a prior successful domain
/// commit after reply loss; never reset already-present or unsupported state.
/// Success attests durable supported state. Neither success nor failure may
/// leave workers, network registrations, or other live resources unowned.
/// Async/resource-owning adapters need separately retained ownership and are not
/// implemented by this synchronous interface. An unavailable adapter must fail;
/// it cannot synthesize a successful witness or an empty dependency report.
pub trait ServiceInitializer {
    type Error;
    fn initialize_service(
        &mut self,
        ticket: EffectTicket,
        identity: IdentityGuard,
        key: KeyReference,
        service: ServiceId,
    ) -> Result<InitializedService, Self::Error>;
}

/// Execute only a current explicit initialization action. Ordinary controller
/// preparation/restoration continues to validate existing configured state and
/// never calls this function to repair a missing schema.
pub fn initialize_service<A: Authority, D: ServiceInitializer, const S: usize>(
    authority: &A,
    driver: &mut D,
    action: Action<S>,
) -> Result<InitializedService, Error<D::Error>> {
    authority.validate_action(&action)?;
    let Action::InitializeService {
        ticket,
        identity,
        key_reference,
        service,
    } = action
    else {
        return Err(Fault::WrongPhase.into());
    };
    let initialized = driver
        .initialize_service(ticket, identity, key_reference, service)
        .map_err(Error::Driver)?;
    if initialized.ticket != ticket || initialized.service != service {
        return Err(Fault::StaleEvidence.into());
    }
    Ok(initialized)
}
