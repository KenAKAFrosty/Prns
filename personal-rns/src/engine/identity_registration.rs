//! Holding identities and registering what they answer as: the engine's
//! identity-bound registration surface. Interface registration lives with the
//! engine core; other registration kinds may earn their own homes later.

use crate::engine::self_ratchets::TrackRatchetsError;
use crate::engine::{EngineState, RatchetPolicy};
use crate::identity::held::HoldIdentityError;
use crate::identity::{IdentityHash, IDENTITY_SECRET_KEY_LEN};
use crate::routing::storage::EngineStorage;
use crate::routing::upstream_app_destinations::{
    ProofStrategy, RegisterDestinationError, UpstreamAppDestination,
};
use crate::wire::DestinationHash;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetTransportIdentityError {
    UnknownIdentity,
}

impl<S: EngineStorage> EngineState<S> {
    pub fn register_plain_destination(
        &mut self,
        app_name: &str,
        aspects: &[&str],
    ) -> Result<DestinationHash, RegisterDestinationError> {
        self.upstream_app_destinations
            .register_plain(app_name, aspects)
    }

    pub fn register_single_destination(
        &mut self,
        identity: &IdentityHash,
        app_name: &str,
        aspects: &[&str],
        proof_strategy: ProofStrategy,
        ratchet_policy: RatchetPolicy,
    ) -> Result<DestinationHash, RegisterDestinationError> {
        if !self.held_identities.contains(identity) {
            return Err(RegisterDestinationError::UnknownIdentity);
        }
        let registered = self.upstream_app_destinations.register_single(
            identity,
            app_name,
            aspects,
            proof_strategy,
        )?;
        if matches!(ratchet_policy, RatchetPolicy::Ratcheted) {
            self.self_ratchets
                .track(registered)
                .map_err(|TrackRatchetsError::TableFull| {
                    RegisterDestinationError::RatchetTableFull
                })?;
        }
        Ok(registered)
    }

    pub fn hold_identity(
        &mut self,
        identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    ) -> Result<IdentityHash, HoldIdentityError> {
        self.held_identities.hold(identity_secret_key)
    }

    pub fn held_identity_hashes(&self) -> &[IdentityHash] {
        self.held_identities.hashes()
    }

    pub fn set_transport_identity(
        &mut self,
        identity: &IdentityHash,
    ) -> Result<(), SetTransportIdentityError> {
        if !self.held_identities.contains(identity) {
            return Err(SetTransportIdentityError::UnknownIdentity);
        }
        self.transport_identity = Some(*identity);
        Ok(())
    }

    pub const fn transport_identity(&self) -> Option<IdentityHash> {
        self.transport_identity
    }

    pub fn upstream_app_destinations(&self) -> impl Iterator<Item = UpstreamAppDestination> + '_ {
        self.upstream_app_destinations.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;

    #[test]
    fn re_registering_the_announced_name_is_idempotent() {
        let mut state = personal_node_announcer();
        let node = state.transport_identity().unwrap();
        let registered = state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .expect("re-registration of the announced name is idempotent");
        assert_eq!(state.self_announced_destinations(), &[registered]);
        assert_eq!(state.upstream_app_destinations().count(), 1);
    }

    #[test]
    fn a_single_registration_requires_its_identity_to_be_held_but_plain_needs_none() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let unheld = IdentityHash::new([0x4c; 16]);
        assert_eq!(
            state.register_single_destination(
                &unheld,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            ),
            Err(RegisterDestinationError::UnknownIdentity),
        );
        assert!(state
            .register_plain_destination("personal", &["node"])
            .is_ok());
        assert_eq!(state.upstream_app_destinations().count(), 1);
    }

    #[test]
    fn transport_identity_requires_a_held_identity() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let unheld = IdentityHash::new([0x4c; 16]);
        assert_eq!(
            state.set_transport_identity(&unheld),
            Err(SetTransportIdentityError::UnknownIdentity),
        );
        assert_eq!(state.transport_identity(), None);

        let held = state.hold_identity(fixed_secret_key()).unwrap();
        assert_eq!(state.set_transport_identity(&held), Ok(()));
        assert_eq!(state.transport_identity(), Some(held));
    }
}
