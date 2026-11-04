use crate::{Error, IdentityStore};
use prns_identities_core::{Action, AdmissionGuard, Coordinator, EffectTicket, IdentityGuard};
use prns_identities_runtime::{Authority, Fault};

/// A runtime authority bound to both the live coordinator and the locked durable
/// catalog. A second coordinator opening the same store in this process makes
/// the older authority fail closed even when its in-memory ticket still matches.
///
/// Keep the store and runtime dispatch under the same exclusive owner. The check
/// must happen at execution as well as submission for asynchronous drivers.
pub struct CatalogAuthority<'a, 'buffers, const I: usize, const O: usize, const S: usize> {
    store: &'a IdentityStore<I, O, S>,
    coordinator: &'a Coordinator<'buffers, I, O, S>,
}

impl<const I: usize, const O: usize, const S: usize> IdentityStore<I, O, S> {
    pub fn runtime_authority<'a, 'buffers>(
        &'a self,
        coordinator: &'a Coordinator<'buffers, I, O, S>,
    ) -> CatalogAuthority<'a, 'buffers, I, O, S> {
        CatalogAuthority {
            store: self,
            coordinator,
        }
    }
}

impl<const I: usize, const O: usize, const S: usize> Authority
    for CatalogAuthority<'_, '_, I, O, S>
{
    fn validate_action<const SERVICES: usize>(
        &self,
        action: &Action<SERVICES>,
    ) -> Result<(), Fault> {
        self.store
            .validate_owner(self.coordinator)
            .map_err(map_error)?;
        Authority::validate_action(self.coordinator, action)
    }

    fn validate_effect(&self, ticket: EffectTicket) -> Result<(), Fault> {
        self.store
            .validate_effect(self.coordinator, ticket)
            .map_err(map_error)
    }

    fn validate_admission(&self, guard: AdmissionGuard) -> Result<(), Fault> {
        self.store
            .validate_owner(self.coordinator)
            .map_err(map_error)?;
        Authority::validate_admission(self.coordinator, guard)
    }

    fn validate_quiescence_commit(
        &self,
        ticket: EffectTicket,
        identity: IdentityGuard,
    ) -> Result<(), Fault> {
        self.store
            .validate_owner(self.coordinator)
            .map_err(map_error)?;
        Authority::validate_quiescence_commit(self.coordinator, ticket, identity)
    }
}

fn map_error(error: Error) -> Fault {
    match error {
        Error::Authority(fault) => Fault::Authority(fault),
        // Detailed filesystem diagnostics remain available through the store's
        // own API. The allocation-free dispatch boundary needs only to deny use.
        _ => Fault::AuthorityUnavailable,
    }
}
