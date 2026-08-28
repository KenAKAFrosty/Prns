use crate::crypto::ratchets::{LastRotated, SeedSelfRatchetsOutcome, TrackRatchetsError};
use crate::crypto::X25519SecretKey;
use crate::engine::state::{NetworkTransport, TransportState};
use crate::engine::InstantMillis;
use crate::engine::{AllowRequester, AllowRequesterRejection, CommandId, CommandOutcome};
use crate::engine::{EngineState, RatchetPolicy};
use crate::identity::held::HoldIdentityError;
use crate::identity::{IdentityHash, IDENTITY_SECRET_KEY_LEN};
use crate::remote_control::{RemoteControlNodeIdentities, RemoteControlNodeIdentitySecrets};
use crate::routing::announce::emit::MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::announce::{derive_destination_hash, expand_name, Announce};
use crate::routing::group_keys::{GroupKey, GroupKeyError};
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::request_handlers::{RequestHandlerError, RequestPathHash, RequestPolicy};
use crate::routing::upstream_app_destinations::LinkRequestPolicy;
use crate::routing::upstream_app_destinations::{
    ProofStrategy, RegisterDestinationError, UnregisterDestinationOutcome, UpstreamAppDestination,
};
use crate::routing::warmth::Departure;
use crate::routing::{PersistedRouteRow, SeedRouteOutcome};
use crate::storage::{StorageLayout, TablePushError};
use crate::units::ByteLimit;
use crate::wire::{DestinationHash, TransportId};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetTransportIdentityError {
    UnknownIdentity,
    AlreadyConfigured,
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

    #[allow(clippy::too_many_arguments)]
    pub fn register_single_destination(
        &mut self,
        identity: &IdentityHash,
        app_name: &str,
        aspects: &[&str],
        app_data: &[u8],
        proof_strategy: ProofStrategy,
        link_request_policy: LinkRequestPolicy,
        ratchet_policy: RatchetPolicy,
    ) -> Result<DestinationHash, RegisterDestinationError> {
        if !self.held_identities.contains(identity) {
            return Err(RegisterDestinationError::UnknownIdentity);
        }
        let ratcheted = matches!(
            ratchet_policy,
            RatchetPolicy::Ratcheted | RatchetPolicy::RatchetsRequired
        );
        if ratcheted {
            if app_data.len() > MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN {
                return Err(RegisterDestinationError::AppDataTooLong);
            }
            let name_hash =
                expand_name(app_name, aspects).map_err(RegisterDestinationError::Name)?;
            let destination = derive_destination_hash(identity, &name_hash);
            if !self.self_ratchets.is_tracked(&destination) && !self.self_ratchets.has_room() {
                return Err(RegisterDestinationError::RatchetTableFull);
            }
        }
        let registered = self.upstream_app_destinations.register_single(
            identity,
            app_name,
            aspects,
            app_data,
            proof_strategy,
            link_request_policy,
            ratchet_policy,
        )?;
        if ratcheted {
            self.self_ratchets
                .track(registered)
                .map_err(|TrackRatchetsError::TableFull| {
                    RegisterDestinationError::RatchetTableFull
                })?;
        }
        Ok(registered)
    }

    /// RNS 1.4.2 GROUP (type `0x01`): `identity` is addressing material only: a GROUP never announces, proves, or ratchets.
    pub fn register_group_destination(
        &mut self,
        identity: &IdentityHash,
        app_name: &str,
        aspects: &[&str],
        shared_key: &[u8],
    ) -> Result<DestinationHash, RegisterDestinationError> {
        let key = GroupKey::from_slice(shared_key)
            .map_err(|GroupKeyError::InvalidLength| RegisterDestinationError::InvalidGroupKey)?;
        let name_hash = expand_name(app_name, aspects).map_err(RegisterDestinationError::Name)?;
        let destination = derive_destination_hash(identity, &name_hash);
        if self.group_keys.key_for(&destination).is_none() && !self.group_keys.has_room() {
            return Err(RegisterDestinationError::RegistryFull);
        }
        let registered = self
            .upstream_app_destinations
            .register_group(identity, app_name, aspects)?;
        self.group_keys
            .insert(registered, key)
            .map_err(|TablePushError::TableFull| RegisterDestinationError::RegistryFull)?;
        Ok(registered)
    }

    pub fn hold_identity(
        &mut self,
        identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    ) -> Result<IdentityHash, HoldIdentityError> {
        self.held_identities.hold(identity_secret_key)
    }

    pub fn configure_remote_control_identities(
        &mut self,
        secrets: RemoteControlNodeIdentitySecrets,
    ) -> Result<RemoteControlNodeIdentities, HoldIdentityError> {
        let identities = secrets.identities();
        let (controller, target) = secrets.into_parts();
        self.held_identities.hold_pair(controller, target)?;
        Ok(identities)
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
        let id = TransportId::new(*identity.as_bytes());
        match self.transport {
            TransportState::Unidentified => {
                self.transport = TransportState::Identified {
                    id,
                    network: NetworkTransport::Enabled,
                };
                Ok(())
            }
            TransportState::Identified { id: existing, .. } if existing == id => {
                self.transport = TransportState::Identified {
                    id,
                    network: NetworkTransport::Enabled,
                };
                Ok(())
            }
            TransportState::Identified { .. } => Err(SetTransportIdentityError::AlreadyConfigured),
        }
    }

    pub fn set_non_routing_identity(
        &mut self,
        identity: &IdentityHash,
    ) -> Result<(), SetTransportIdentityError> {
        if !self.held_identities.contains(identity) {
            return Err(SetTransportIdentityError::UnknownIdentity);
        }
        let id = TransportId::new(*identity.as_bytes());
        match self.transport {
            TransportState::Unidentified => {
                self.transport = TransportState::Identified {
                    id,
                    network: NetworkTransport::Disabled,
                };
                Ok(())
            }
            TransportState::Identified { id: existing, .. } if existing == id => {
                self.transport = TransportState::Identified {
                    id,
                    network: NetworkTransport::Disabled,
                };
                Ok(())
            }
            TransportState::Identified { .. } => Err(SetTransportIdentityError::AlreadyConfigured),
        }
    }

    pub fn set_shared_instance_identity(
        &mut self,
        identity: &IdentityHash,
    ) -> Result<(), SetTransportIdentityError> {
        self.set_non_routing_identity(identity)
    }

    pub const fn transport_id(&self) -> Option<TransportId> {
        self.transport.id()
    }

    pub const fn network_transport_enabled(&self) -> bool {
        self.transport.network_transport_enabled()
    }

    pub fn upstream_app_destinations(&self) -> impl Iterator<Item = UpstreamAppDestination> + '_ {
        self.upstream_app_destinations.iter()
    }

    pub fn unregister_destination(
        &mut self,
        destination: &DestinationHash,
    ) -> UnregisterDestinationOutcome {
        let outcome = self.upstream_app_destinations.unregister(destination);
        let UnregisterDestinationOutcome::Unregistered { .. } = outcome else {
            return outcome;
        };
        self.request_handlers.unregister_destination(destination);
        let _ = self.scheduled_announces.cancel(destination);
        self.self_ratchets.untrack(destination);
        self.group_keys.remove(destination);
        outcome
    }

    /// RNS 1.4.2 apps set `Link.resource_strategy` in the link-established callback, a de facto per-destination default; stamping at activation outraces a sender's instant advertise.
    pub fn set_default_resource_strategy(
        &mut self,
        destination: &DestinationHash,
        strategy: ResourceStrategy,
    ) -> bool {
        self.upstream_app_destinations
            .set_default_resource_strategy(destination, strategy)
    }

    pub fn set_maximum_request_bytes(
        &mut self,
        destination: &DestinationHash,
        maximum: ByteLimit,
    ) -> bool {
        self.upstream_app_destinations
            .set_maximum_request_bytes(destination, maximum)
    }

    /// RNS 1.4.2 `Destination.register_request_handler`; last write wins, and a re-registration starts from an empty allow list.
    pub fn register_request_handler(
        &mut self,
        destination: &DestinationHash,
        path: &str,
        policy: RequestPolicy,
    ) -> Result<(), TablePushError> {
        self.register_request_handler_hash(destination, RequestPathHash::of(path), policy)
    }

    /// Register a request handler when the host has already derived the
    /// protocol path hash.
    pub fn register_request_handler_hash(
        &mut self,
        destination: &DestinationHash,
        path_hash: RequestPathHash,
        policy: RequestPolicy,
    ) -> Result<(), TablePushError> {
        self.request_handlers
            .register(*destination, path_hash, policy)
    }

    /// Remove a runtime request handler. Requests already admitted before this
    /// mutation may still reach the application router; callers that need an
    /// immediate content cutoff should update their application state first.
    pub fn unregister_request_handler_hash(
        &mut self,
        destination: &DestinationHash,
        path_hash: &RequestPathHash,
    ) -> bool {
        self.request_handlers.unregister(destination, path_hash)
    }

    /// Admit one identified peer to an [`RequestPolicy::AllowList`] handler (RNS 1.4.2's `allowed_list`)
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

    pub(crate) fn ingest_allow_requester_command(
        &mut self,
        id: CommandId,
        allow: AllowRequester,
    ) -> CommandOutcome {
        match self
            .request_handlers
            .allow(&allow.destination, &allow.path_hash, allow.identity)
        {
            Ok(()) => CommandOutcome::RequesterAllowed { id },
            Err(RequestHandlerError::NoSuchHandler) => CommandOutcome::AllowRequesterRejected {
                id,
                rejection: AllowRequesterRejection::NoSuchHandler,
            },
            Err(RequestHandlerError::NoAllowList) => CommandOutcome::AllowRequesterRejected {
                id,
                rejection: AllowRequesterRejection::NoAllowList,
            },
            Err(RequestHandlerError::AllowListFull) => CommandOutcome::AllowRequesterRejected {
                id,
                rejection: AllowRequesterRejection::AllowListFull,
            },
        }
    }

    /// Every routing-table row in the shape the persistence codec carries, for a host's flush pass.
    pub fn persisted_route_rows(&self) -> impl Iterator<Item = PersistedRouteRow<'_>> + '_ {
        self.routing_table.persisted_rows()
    }

    pub fn persisted_route_destinations(&self) -> impl Iterator<Item = DestinationHash> + '_ {
        self.routing_table.persisted_destinations()
    }

    pub fn persisted_route_row(
        &self,
        destination: &DestinationHash,
    ) -> Option<PersistedRouteRow<'_>> {
        self.routing_table.persisted_row(destination)
    }

    /// Boot-restore for one snapshot row, refusing what storage may have forged: the address binding re-derives and the announce signature re-verifies before anything lands.
    /// RNS 1.4.2's load path instead re-reads the cached announce packet and counts the cache read as a hop (`announce_packet.hops += 1`); seeding writes the row directly, so `hops` carries verbatim.
    /// A seeded row's interface gets the departed grace (`Departure::MayReturn`), holding the route warm until the medium re-derives the same id at attach.
    pub fn seed_route(
        &mut self,
        row: &PersistedRouteRow<'_>,
        now: InstantMillis,
    ) -> RouteSeedOutcome {
        let pending = match self.prepare_persisted_route(row.clone()) {
            Ok(pending) => pending,
            Err(PersistedRoutePreflightError::DestinationMismatch) => {
                return RouteSeedOutcome::RefusedDestinationMismatch;
            }
            Err(PersistedRoutePreflightError::BlackholedIdentity) => {
                return RouteSeedOutcome::RefusedBlackholedIdentity;
            }
        };
        let verified = match pending.verify() {
            Ok(verified) => verified,
            Err(PersistedRouteVerificationError::InvalidSignature) => {
                return RouteSeedOutcome::RefusedInvalidSignature;
            }
        };
        self.seed_verified_route(verified, now)
    }

    pub fn prepare_persisted_route<'a>(
        &self,
        row: PersistedRouteRow<'a>,
    ) -> Result<PersistedRouteSignaturePending<'a>, PersistedRoutePreflightError> {
        let identity_hash = row.public_keys.identity_hash();
        if derive_destination_hash(&identity_hash, &row.dotted_name_hash) != row.destination {
            return Err(PersistedRoutePreflightError::DestinationMismatch);
        }
        if self.identity_blackholes.is_blackholed(&identity_hash) {
            return Err(PersistedRoutePreflightError::BlackholedIdentity);
        }
        Ok(PersistedRouteSignaturePending { row, identity_hash })
    }

    pub fn seed_verified_route(
        &mut self,
        verified: VerifiedPersistedRoute<'_>,
        now: InstantMillis,
    ) -> RouteSeedOutcome {
        if self
            .identity_blackholes
            .is_blackholed(&verified.pending.identity_hash)
        {
            return RouteSeedOutcome::RefusedBlackholedIdentity;
        }
        let route_evidence_id = self.route_evidence_id_for_update(
            &verified.pending.row.destination,
            verified.pending.row.entry.receiving_interface,
            verified.pending.row.entry.next_hop,
        );
        match self
            .routing_table
            .seed_route(&verified.pending.row, route_evidence_id)
        {
            SeedRouteOutcome::Seeded => {
                self.departed_interfaces.record(
                    verified.pending.row.entry.receiving_interface,
                    Departure::MayReturn,
                    now,
                );
                RouteSeedOutcome::Seeded
            }
            SeedRouteOutcome::AlreadyPresent => RouteSeedOutcome::AlreadyPresent,
            SeedRouteOutcome::TableFull => RouteSeedOutcome::TableFull,
            SeedRouteOutcome::AppDataArenaFull => RouteSeedOutcome::AppDataArenaFull,
        }
    }
}

#[derive(Debug)]
pub struct PersistedRouteSignaturePending<'a> {
    row: PersistedRouteRow<'a>,
    identity_hash: IdentityHash,
}

impl<'a> PersistedRouteSignaturePending<'a> {
    pub fn verify(&self) -> Result<VerifiedPersistedRoute<'_>, PersistedRouteVerificationError> {
        let announce = Announce {
            destination: self.row.destination,
            public_keys: self.row.public_keys,
            dotted_name_hash: self.row.dotted_name_hash,
            announce_id: self.row.announce_id,
            ratchet: self.row.ratchet,
            signature: self.row.signature,
            app_data: self.row.app_data,
        };
        if !announce.signature_is_valid() {
            return Err(PersistedRouteVerificationError::InvalidSignature);
        }
        Ok(VerifiedPersistedRoute { pending: self })
    }
}

#[derive(Debug)]
pub struct VerifiedPersistedRoute<'a> {
    pending: &'a PersistedRouteSignaturePending<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistedRoutePreflightError {
    DestinationMismatch,
    BlackholedIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistedRouteVerificationError {
    InvalidSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSeedOutcome {
    Seeded,
    RefusedDestinationMismatch,
    RefusedBlackholedIdentity,
    RefusedInvalidSignature,
    AlreadyPresent,
    TableFull,
    AppDataArenaFull,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn persisted_self_ratchet_rows(
        &self,
    ) -> impl Iterator<Item = (DestinationHash, LastRotated, &[X25519SecretKey])> + '_ {
        self.self_ratchets.persisted_rows()
    }

    pub fn persisted_self_ratchet_row(
        &self,
        destination: &DestinationHash,
    ) -> Option<(LastRotated, &[X25519SecretKey])> {
        self.self_ratchets.persisted_row(destination)
    }

    /// Boot-restore after the recipe re-registers its destinations: only a destination this
    /// boot tracks as ratcheted accepts its stored record, and live secrets win over storage.
    pub fn seed_self_ratchets(
        &mut self,
        destination: &DestinationHash,
        last_rotated: LastRotated,
        secrets_newest_first: impl DoubleEndedIterator<Item = X25519SecretKey>,
    ) -> SeedSelfRatchetsOutcome {
        self.self_ratchets
            .seed(destination, last_rotated, secrets_newest_first)
    }

    pub fn replace_persisted_self_ratchets(
        &mut self,
        destination: &DestinationHash,
        last_rotated: LastRotated,
        secrets_newest_first: impl DoubleEndedIterator<Item = X25519SecretKey>,
    ) -> SeedSelfRatchetsOutcome {
        self.self_ratchets
            .replace_persisted(destination, last_rotated, secrets_newest_first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;

    #[test]
    fn a_ratcheted_registration_rejects_app_data_that_cannot_ride_beside_the_ratchet() {
        let mut state = personal_node_announcer();
        let node = state.held_identity_hashes()[0];
        let oversize = [0u8; MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN + 1];

        assert_eq!(
            state.register_single_destination(
                &node,
                "personal",
                &["ratcheted"],
                &oversize,
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::Ratcheted,
            ),
            Err(RegisterDestinationError::AppDataTooLong),
        );
        assert!(state
            .register_single_destination(
                &node,
                "personal",
                &["unratcheted"],
                &oversize,
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .is_ok());
    }

    #[test]
    fn the_allow_requester_command_opens_the_list_gate_for_one_peer() {
        let mut state = personal_node_announcer();
        let node = state.held_identity_hashes()[0];
        let destination = state
            .register_single_destination(
                &node,
                "bench",
                &["query"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the bench destination");
        state
            .register_request_handler(&destination, "/q", RequestPolicy::AllowList)
            .expect("registers the list handler");

        let path_hash = RequestPathHash::of("/q");
        let peer = IdentityHash::new([0x7A; 16]);
        assert!(
            !state
                .request_handlers
                .permits(&destination, &path_hash, Some(&peer)),
            "an empty list admits no one",
        );

        assert_eq!(
            state.ingest_allow_requester_command(
                CommandId(1),
                AllowRequester {
                    destination,
                    path_hash,
                    identity: peer,
                },
            ),
            CommandOutcome::RequesterAllowed { id: CommandId(1) },
        );
        assert!(
            state
                .request_handlers
                .permits(&destination, &path_hash, Some(&peer)),
            "the command admitted the peer to the gate",
        );

        assert_eq!(
            state.ingest_allow_requester_command(
                CommandId(2),
                AllowRequester {
                    destination,
                    path_hash: RequestPathHash::of("/unregistered"),
                    identity: peer,
                },
            ),
            CommandOutcome::AllowRequesterRejected {
                id: CommandId(2),
                rejection: AllowRequesterRejection::NoSuchHandler,
            },
        );
    }

    #[test]
    fn admission_to_a_handler_that_keeps_no_list_is_refused() {
        let mut state = personal_node_announcer();
        let node = state.held_identity_hashes()[0];
        let destination = state
            .register_single_destination(
                &node,
                "bench",
                &["open"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the open destination");
        state
            .register_request_handler(&destination, "/open", RequestPolicy::AllowAll)
            .expect("registers the open-door handler");

        let peer = IdentityHash::new([0x7A; 16]);
        assert_eq!(
            state.allow_requester(&destination, "/open", peer),
            Err(RequestHandlerError::NoAllowList),
        );
        assert_eq!(
            state.disallow_requester(&destination, "/open", &peer),
            Err(RequestHandlerError::NoAllowList),
        );
        assert_eq!(
            state.ingest_allow_requester_command(
                CommandId(7),
                AllowRequester {
                    destination,
                    path_hash: RequestPathHash::of("/open"),
                    identity: peer,
                },
            ),
            CommandOutcome::AllowRequesterRejected {
                id: CommandId(7),
                rejection: AllowRequesterRejection::NoAllowList,
            },
        );
    }

    #[test]
    fn a_full_ratchet_table_refuses_new_ratcheted_registrations_before_registering() {
        let mut state = EngineState::<TestStorageLayout>::default();
        let node = state.hold_identity(fixed_secret_key()).unwrap();
        for aspect in ["r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7"] {
            state
                .register_single_destination(
                    &node,
                    "personal",
                    &[aspect],
                    b"",
                    ProofStrategy::ProveAll,
                    LinkRequestPolicy::AcceptAll,
                    RatchetPolicy::Ratcheted,
                )
                .expect("fills one ratchet slot");
        }

        assert_eq!(
            state.register_single_destination(
                &node,
                "personal",
                &["overflow"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::Ratcheted,
            ),
            Err(RegisterDestinationError::RatchetTableFull),
        );
        assert_eq!(
            state.upstream_app_destinations().count(),
            8,
            "the refused registration left nothing behind",
        );

        assert!(
            state
                .register_single_destination(
                    &node,
                    "personal",
                    &["r0"],
                    b"",
                    ProofStrategy::ProveAll,
                    LinkRequestPolicy::AcceptAll,
                    RatchetPolicy::Ratcheted,
                )
                .is_ok(),
            "an already-ratcheted destination re-registers on a full table",
        );
    }

    #[test]
    fn a_full_key_registry_refuses_new_groups_before_registering() {
        let mut state = EngineState::<TestStorageLayout>::default();
        let identity = IdentityHash::new([0x4c; 16]);
        for aspect in ["g0", "g1", "g2", "g3", "g4", "g5", "g6", "g7"] {
            state
                .register_group_destination(&identity, "personal", &[aspect], &[0x42; 64])
                .expect("fills one key slot");
        }

        assert_eq!(
            state.register_group_destination(&identity, "personal", &["overflow"], &[0x42; 64]),
            Err(RegisterDestinationError::RegistryFull),
        );
        assert_eq!(
            state.upstream_app_destinations().count(),
            8,
            "the refused registration left nothing behind",
        );

        assert!(
            state
                .register_group_destination(&identity, "personal", &["g0"], &[0x99; 64])
                .is_ok(),
            "a group with a stored key re-registers on a full table",
        );
    }

    #[test]
    fn destination_unregistration_removes_only_state_owned_by_that_destination() {
        let mut state = EngineState::<TestStorageLayout>::default();
        let node = state.hold_identity(fixed_secret_key()).unwrap();
        let target = state
            .register_single_destination(
                &node,
                "personal",
                &["target"],
                b"target",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::Ratcheted,
            )
            .unwrap();
        let retained = state
            .register_group_destination(&node, "personal", &["retained"], &[0x42; 64])
            .unwrap();
        let target_registration = state
            .upstream_app_destinations()
            .find(|registration| registration.destination == target)
            .unwrap();
        let retained_registration = state
            .upstream_app_destinations()
            .find(|registration| registration.destination == retained)
            .unwrap();
        let target_first = RequestPathHash::of("/target/first");
        let target_second = RequestPathHash::of("/target/second");
        let retained_path = RequestPathHash::of("/retained");
        state
            .register_request_handler_hash(&target, target_first, RequestPolicy::AllowAll)
            .unwrap();
        state
            .register_request_handler_hash(&retained, retained_path, RequestPolicy::AllowAll)
            .unwrap();
        state
            .register_request_handler_hash(&target, target_second, RequestPolicy::AllowAll)
            .unwrap();
        state
            .self_ratchets
            .rotate_if_due(&target, InstantMillis(1_000), &mut test_fill_entropy);
        let interface = crate::interfaces::InterfaceId::new([0x41; 8]);
        let _ = state
            .scheduled_announces
            .schedule(target, InstantMillis(2_000), interface, 0);
        let _ = state
            .scheduled_announces
            .schedule(retained, InstantMillis(3_000), interface, 0);

        assert_eq!(
            state.unregister_destination(&target),
            UnregisterDestinationOutcome::Unregistered {
                registration: target_registration,
            },
        );

        assert_eq!(
            state
                .upstream_app_destinations()
                .collect::<std::vec::Vec<_>>(),
            [retained_registration],
        );
        assert!(!state.request_handlers.permits(&target, &target_first, None));
        assert!(!state
            .request_handlers
            .permits(&target, &target_second, None));
        assert!(state
            .request_handlers
            .permits(&retained, &retained_path, None));
        assert_eq!(state.request_handlers.len(), 1);
        assert!(!state.self_ratchets.is_tracked(&target));
        assert!(state.group_keys.key_for(&retained).is_some());
        assert_eq!(state.scheduled_announces.scheduled_count(), 1);
        assert_eq!(
            state
                .scheduled_announces
                .iter()
                .next()
                .map(|scheduled| scheduled.destination),
            Some(retained),
        );

        assert_eq!(
            state.unregister_destination(&target),
            UnregisterDestinationOutcome::NotRegistered,
        );
        assert_eq!(
            state.unregister_destination(&retained),
            UnregisterDestinationOutcome::Unregistered {
                registration: retained_registration,
            },
        );
        assert!(state.upstream_app_destinations().next().is_none());
        assert!(state.request_handlers.is_empty());
        assert!(state.group_keys.key_for(&retained).is_none());
        assert_eq!(state.scheduled_announces.scheduled_count(), 0);
    }

    #[test]
    fn destination_unregistration_reclaims_bounded_credential_slots() {
        let mut ratcheted = EngineState::<TestStorageLayout>::default();
        let node = ratcheted.hold_identity(fixed_secret_key()).unwrap();
        let mut removed_ratchet = None;
        for aspect in ["r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7"] {
            let destination = ratcheted
                .register_single_destination(
                    &node,
                    "personal",
                    &[aspect],
                    b"",
                    ProofStrategy::ProveAll,
                    LinkRequestPolicy::AcceptAll,
                    RatchetPolicy::Ratcheted,
                )
                .unwrap();
            if removed_ratchet.is_none() {
                removed_ratchet = Some(destination);
            }
        }
        ratcheted.unregister_destination(&removed_ratchet.unwrap());
        let replacement_ratchet = ratcheted
            .register_single_destination(
                &node,
                "personal",
                &["replacement"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::Ratcheted,
            )
            .unwrap();
        assert!(ratcheted.self_ratchets.is_tracked(&replacement_ratchet));
        assert_eq!(ratcheted.upstream_app_destinations().count(), 8);

        let mut grouped = EngineState::<TestStorageLayout>::default();
        let identity = IdentityHash::new([0x4C; 16]);
        let mut removed_group = None;
        for aspect in ["g0", "g1", "g2", "g3", "g4", "g5", "g6", "g7"] {
            let destination = grouped
                .register_group_destination(&identity, "personal", &[aspect], &[0x42; 64])
                .unwrap();
            if removed_group.is_none() {
                removed_group = Some(destination);
            }
        }
        grouped.unregister_destination(&removed_group.unwrap());
        let replacement_group = grouped
            .register_group_destination(&identity, "personal", &["replacement"], &[0x99; 64])
            .unwrap();
        assert_eq!(
            grouped
                .group_keys
                .key_for(&replacement_group)
                .map(GroupKey::as_slice),
            Some([0x99; 64].as_slice()),
        );
        assert_eq!(grouped.upstream_app_destinations().count(), 8);
    }

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
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("re-registration of the announced name is idempotent");
        assert_eq!(registered, personal_node_destination());
        assert_eq!(state.upstream_app_destinations().count(), 1);
    }

    #[test]
    fn a_single_registration_requires_its_identity_to_be_held_but_plain_needs_none() {
        let mut state = EngineState::<TestStorageLayout>::default();
        let unheld = IdentityHash::new([0x4c; 16]);
        assert_eq!(
            state.register_single_destination(
                &unheld,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
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
        let mut state = EngineState::<TestStorageLayout>::default();
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
        let mut state = EngineState::<TestStorageLayout>::default();
        let identity = IdentityHash::new([0x4c; 16]);
        assert_eq!(
            state.register_group_destination(&identity, "personal", &["group"], &[0x42; 48]),
            Err(RegisterDestinationError::InvalidGroupKey),
        );
        assert!(state.upstream_app_destinations().next().is_none());
    }

    #[test]
    fn transport_identity_requires_a_held_identity() {
        let mut state = EngineState::<TestStorageLayout>::default();
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
        assert!(state.network_transport_enabled());
    }

    #[test]
    fn a_non_routing_identity_and_routing_role_cannot_contradict_each_other() {
        let mut state = EngineState::<TestStorageLayout>::default();
        let held = state.hold_identity(fixed_secret_key()).unwrap();

        assert_eq!(state.set_shared_instance_identity(&held), Ok(()));
        assert_eq!(
            state.transport_id(),
            Some(TransportId::new(*held.as_bytes()))
        );
        assert!(!state.network_transport_enabled());

        assert_eq!(state.set_transport_identity(&held), Ok(()));
        assert!(state.network_transport_enabled());

        assert_eq!(state.set_non_routing_identity(&held), Ok(()));
        assert!(!state.network_transport_enabled());
    }

    fn signed_seed_row(app_data: &[u8]) -> (PersistedRouteRow<'_>, crate::interfaces::InterfaceId) {
        use crate::identity::in_memory::InMemoryNodeIdentity;
        use crate::routing::announce::AnnounceId;
        use crate::routing::routes::RouteEntry;
        use crate::routing::{AnnounceIdRing, NextHop, RouteResponsiveness};

        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[0x77; 64]);
        let announce = Announce::build_signed(
            &signer,
            crate::routing::announce::DottedNameHash::new([0x21; 10]),
            AnnounceId::from_wire([0x42; 10]),
            None,
            app_data,
        )
        .expect("a built announce");
        let interface = crate::interfaces::InterfaceId::new([0xAB; 8]);
        let row = PersistedRouteRow {
            destination: announce.destination,
            entry: RouteEntry {
                hops: 3,
                learned_at: InstantMillis(500),
                last_route_activity_at: InstantMillis(700),
                responsiveness: RouteResponsiveness::Responsive,
                receiving_interface: interface,
                next_hop: NextHop::Direct,
            },
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            announce_id: announce.announce_id,
            ratchet: announce.ratchet,
            signature: announce.signature,
            app_data,
            announce_id_ring: AnnounceIdRing::Wire(&[]),
        };
        (row, interface)
    }

    #[test]
    fn a_seed_lands_only_what_reverifies_against_its_own_signature() {
        let app_data = [0x5A; 8];
        let (row, _) = signed_seed_row(&app_data);

        let mut state = EngineState::<TestStorageLayout>::default();
        let pending = state.prepare_persisted_route(row.clone()).unwrap();
        let verified = pending.verify().unwrap();
        assert_eq!(
            state.seed_verified_route(verified, InstantMillis(1_000)),
            RouteSeedOutcome::Seeded,
        );
        assert_eq!(state.route_count(), 1);

        let mut forged_signature = row.clone();
        forged_signature.signature.0[0] ^= 0x01;
        forged_signature.destination = crate::wire::DestinationHash::new([0x0D; 16]);
        let mut fresh = EngineState::<TestStorageLayout>::default();
        assert_eq!(
            fresh.seed_route(&forged_signature, InstantMillis(1_000)),
            RouteSeedOutcome::RefusedDestinationMismatch,
            "a forged destination fails the address binding before any crypto runs",
        );

        let mut tampered = row.clone();
        tampered.signature.0[0] ^= 0x01;
        assert_eq!(
            fresh.seed_route(&tampered, InstantMillis(1_000)),
            RouteSeedOutcome::RefusedInvalidSignature,
        );
        assert_eq!(fresh.route_count(), 0);
    }

    #[test]
    fn a_verified_seed_rechecks_blackhole_policy_before_mutating_state() {
        let app_data = [0x5D; 8];
        let (row, _) = signed_seed_row(&app_data);
        let identity = row.public_keys.identity_hash();
        let mut state = EngineState::<TestStorageLayout>::default();
        let pending = state.prepare_persisted_route(row).unwrap();
        let verified = pending.verify().unwrap();

        assert_eq!(
            state
                .blackhole_identity(
                    crate::routing::BlackholedIdentity {
                        identity,
                        source: IdentityHash::new([0xC4; 16]),
                        expiry: crate::routing::BlackholeExpiry::Indefinite,
                        reason: None,
                    },
                    crate::interfaces::AttachedInterfaces::new(&[]),
                    &mut |_| {},
                )
                .outcome,
            Ok(crate::routing::BlackholeIdentityOutcome::Added),
        );
        assert_eq!(
            state.seed_verified_route(verified, InstantMillis(1_000)),
            RouteSeedOutcome::RefusedBlackholedIdentity,
        );
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn a_blackholed_identity_cannot_return_through_route_restore() {
        let app_data = [0x5C; 8];
        let (mut row, _) = signed_seed_row(&app_data);
        let identity = row.public_keys.identity_hash();
        row.signature.0[0] ^= 0x01;
        let mut state = EngineState::<TestStorageLayout>::default();

        assert_eq!(
            state
                .blackhole_identity(
                    crate::routing::BlackholedIdentity {
                        identity,
                        source: IdentityHash::new([0xC3; 16]),
                        expiry: crate::routing::BlackholeExpiry::Indefinite,
                        reason: None,
                    },
                    crate::interfaces::AttachedInterfaces::new(&[]),
                    &mut |_| {},
                )
                .outcome,
            Ok(crate::routing::BlackholeIdentityOutcome::Added),
        );
        assert_eq!(
            state.seed_route(&row, InstantMillis(1_000)),
            RouteSeedOutcome::RefusedBlackholedIdentity,
        );
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn a_seeded_routes_interface_rides_the_departed_grace() {
        use crate::routing::warmth::{RouteWarmth, DEPARTED_INTERFACE_GRACE_MS};

        let app_data = [0x5B; 4];
        let (row, interface) = signed_seed_row(&app_data);
        let mut state = EngineState::<TestStorageLayout>::default();
        let now = InstantMillis(2_000);
        assert_eq!(state.seed_route(&row, now), RouteSeedOutcome::Seeded);
        assert_eq!(
            state.departed_interfaces.warm_until(interface),
            Some(InstantMillis(now.0 + DEPARTED_INTERFACE_GRACE_MS)),
            "the not-yet-attached interface holds the route warm from boot",
        );
    }

    #[test]
    fn restoring_an_expired_route_does_not_refresh_its_age() {
        use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
        use crate::routing::warmth::DEPARTED_INTERFACE_GRACE_MS;

        let app_data = [0x5E; 4];
        let (row, _) = signed_seed_row(&app_data);
        let mut state = EngineState::<TestStorageLayout>::default();
        let restored_at = InstantMillis(DEFAULT_ROUTE_EXPIRY_MILLIS + 10_000);
        assert_eq!(
            state.seed_route(&row, restored_at),
            RouteSeedOutcome::Seeded
        );
        state.cull_expired_routes(
            InstantMillis(restored_at.0 + DEPARTED_INTERFACE_GRACE_MS + 1),
            crate::interfaces::AttachedInterfaces::new(&[]),
            &mut |_| {},
        );
        assert_eq!(state.route_count(), 0);
    }
}
