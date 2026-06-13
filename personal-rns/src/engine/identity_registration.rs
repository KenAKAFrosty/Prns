//! Holding identities and registering what they answer as: the engine's
//! identity-bound registration surface. Interface registration lives with the
//! engine core; other registration kinds may earn their own homes later.

use crate::crypto::ratchets::TrackRatchetsError;
use crate::engine::{EngineState, RatchetPolicy};
use crate::identity::held::HoldIdentityError;
use crate::identity::{IdentityHash, IDENTITY_SECRET_KEY_LEN};
use crate::routing::group_keys::{GroupKey, GroupKeyError};
use crate::routing::request_handlers::{RequestHandlerError, RequestPathHash, RequestPolicy};
use crate::storage::{ColumnsFull, StorageLayout};
use crate::routing::upstream_app_destinations::{
    ProofStrategy, RegisterDestinationError, UpstreamAppDestination,
};
use crate::wire::{DestinationHash, TransportId};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetTransportIdentityError {
    UnknownIdentity,
}

impl<S: StorageLayout> EngineState<S> {
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
        app_data: &[u8],
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
            app_data,
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

    /// Register a GROUP destination (RNS 1.3.1 type `0x01`). The address derives
    /// like a Single's, but `identity` is addressing material only; no keypair
    /// is held for it. The shared symmetric key is what decrypts traffic; a GROUP
    /// never announces, proves, or ratchets.
    pub fn register_group_destination(
        &mut self,
        identity: &IdentityHash,
        app_name: &str,
        aspects: &[&str],
        shared_key: &[u8],
    ) -> Result<DestinationHash, RegisterDestinationError> {
        let key = GroupKey::from_slice(shared_key)
            .map_err(|GroupKeyError::InvalidLength| RegisterDestinationError::InvalidGroupKey)?;
        let registered = self
            .upstream_app_destinations
            .register_group(identity, app_name, aspects)?;
        self.group_keys
            .insert(registered, key)
            .map_err(|ColumnsFull| RegisterDestinationError::RegistryFull)?;
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

    /// Take the transport role with a bare 16-byte id (typically the relay flavor).
    /// Forwarding never signs, so no key needs to exist behind this id.
    pub fn set_transport_id(&mut self, id: TransportId) {
        self.transport_id = Some(id);
    }

    /// Take the transport role as a held identity — the addressable flavor, for
    /// nodes that will also answer as this identity (management, tunnels later).
    pub fn set_transport_identity(
        &mut self,
        identity: &IdentityHash,
    ) -> Result<(), SetTransportIdentityError> {
        if !self.held_identities.contains(identity) {
            return Err(SetTransportIdentityError::UnknownIdentity);
        }
        self.transport_id = Some(TransportId::new(*identity.as_bytes()));
        Ok(())
    }

    pub const fn transport_id(&self) -> Option<TransportId> {
        self.transport_id
    }

    pub fn upstream_app_destinations(&self) -> impl Iterator<Item = UpstreamAppDestination> + '_ {
        self.upstream_app_destinations.iter()
    }

    /// The destination's standing answer to inbound resource offers — RNS
    /// 1.3.1 apps set `Link.resource_strategy` inside the link-established
    /// callback on every link, which makes it a de facto per-destination
    /// default; registering it here stamps every responder-side link at
    /// activation, so no per-link command can race a sender who advertises
    /// the instant the link comes up.
    pub fn set_default_resource_strategy(
        &mut self,
        destination: &DestinationHash,
        strategy: crate::routing::links::resources::ResourceStrategy,
    ) -> bool {
        self.upstream_app_destinations
            .set_default_resource_strategy(destination, strategy)
    }

    /// RNS 1.3.1 `Destination.register_request_handler`: requests arriving
    /// over this destination's links at `truncated_hash(path)` pass the
    /// registry's gate and journal to the app; everything else dies silently.
    /// Last write wins, and a re-registration starts from an empty allow list.
    pub fn register_request_handler(
        &mut self,
        destination: &DestinationHash,
        path: &str,
        policy: RequestPolicy,
    ) -> Result<(), ColumnsFull> {
        self.request_handlers
            .register(*destination, RequestPathHash::of(path), policy)
    }

    /// Admit one identified peer to an [`RequestPolicy::AllowList`] handler
    /// (RNS 1.3.1's `allowed_list`)
    pub fn allow_requester(
        &mut self,
        destination: &DestinationHash,
        path: &str,
        identity: IdentityHash,
    ) -> Result<(), RequestHandlerError> {
        self.request_handlers
            .allow(destination, &RequestPathHash::of(path), identity)
    }

    pub fn disallow_requester(
        &mut self,
        destination: &DestinationHash,
        path: &str,
        identity: &IdentityHash,
    ) -> Result<(), RequestHandlerError> {
        self.request_handlers
            .disallow(destination, &RequestPathHash::of(path), identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;

    #[test]
    fn re_registering_the_announced_name_is_idempotent() {
        let mut state = personal_node_announcer();
        let node = state.held_identity_hashes()[0];
        let registered = state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .expect("re-registration of the announced name is idempotent");
        assert_eq!(registered, personal_node_destination());
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
                b"",
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
    fn a_group_registration_addresses_off_an_unheld_identity_and_is_idempotent() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        // A GROUP's identity is addressing material only — it need not be held.
        let identity = IdentityHash::new([0x4c; 16]);
        let group = state
            .register_group_destination(&identity, "personal", &["group"], &[0x42; 64])
            .expect("a group registers without holding its addressing identity");
        let again = state
            .register_group_destination(&identity, "personal", &["group"], &[0x42; 64])
            .expect("re-registration is idempotent");
        assert_eq!(group, again);
        assert_eq!(state.upstream_app_destinations().count(), 1);
    }

    #[test]
    fn a_group_key_that_is_neither_aes_128_nor_aes_256_is_rejected() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = IdentityHash::new([0x4c; 16]);
        assert_eq!(
            state.register_group_destination(&identity, "personal", &["group"], &[0x42; 48]),
            Err(RegisterDestinationError::InvalidGroupKey),
        );
        assert!(state.upstream_app_destinations().next().is_none());
    }

    #[test]
    fn transport_identity_requires_a_held_identity() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let unheld = IdentityHash::new([0x4c; 16]);
        assert_eq!(
            state.set_transport_identity(&unheld),
            Err(SetTransportIdentityError::UnknownIdentity),
        );
        assert_eq!(state.transport_id(), None);

        let held = state.hold_identity(fixed_secret_key()).unwrap();
        assert_eq!(state.set_transport_identity(&held), Ok(()));
        assert_eq!(
            state.transport_id(),
            Some(TransportId::new(*held.as_bytes()))
        );
    }
}
