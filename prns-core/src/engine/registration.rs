use crate::crypto::ratchets::TrackRatchetsError;
use crate::engine::InstantMillis;
use crate::engine::{AllowRequester, AllowRequesterRejection, CommandId, CommandOutcome};
use crate::engine::{EngineState, RatchetPolicy};
use crate::identity::held::HoldIdentityError;
use crate::identity::{derive_identity_hash, IdentityHash, IDENTITY_SECRET_KEY_LEN};
use crate::routing::announce::emit::MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN;
use crate::routing::announce::{derive_destination_hash, expand_name, Announce};
use crate::routing::group_keys::{GroupKey, GroupKeyError};
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::request_handlers::{RequestHandlerError, RequestPathHash, RequestPolicy};
use crate::routing::upstream_app_destinations::{
    ProofStrategy, RegisterDestinationError, UpstreamAppDestination,
};
use crate::routing::warmth::Departure;
use crate::routing::{PersistedRouteRow, SeedRouteOutcome};
use crate::storage::{StorageLayout, TablePushError};
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

    /// RNS 1.3.5 GROUP (type `0x01`): `identity` is addressing material only: a GROUP never announces, proves, or ratchets.
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
        self.transport_id = Some(TransportId::new(*identity.as_bytes()));
        Ok(())
    }

    pub const fn transport_id(&self) -> Option<TransportId> {
        self.transport_id
    }

    pub fn upstream_app_destinations(&self) -> impl Iterator<Item = UpstreamAppDestination> + '_ {
        self.upstream_app_destinations.iter()
    }

    /// RNS 1.3.5 apps set `Link.resource_strategy` in the link-established callback, a de facto per-destination default; stamping at activation outraces a sender's instant advertise.
    pub fn set_default_resource_strategy(
        &mut self,
        destination: &DestinationHash,
        strategy: ResourceStrategy,
    ) -> bool {
        self.upstream_app_destinations
            .set_default_resource_strategy(destination, strategy)
    }

    /// RNS 1.3.5 `Destination.register_request_handler`; last write wins, and a re-registration starts from an empty allow list.
    pub fn register_request_handler(
        &mut self,
        destination: &DestinationHash,
        path: &str,
        policy: RequestPolicy,
    ) -> Result<(), TablePushError> {
        self.request_handlers
            .register(*destination, RequestPathHash::of(path), policy)
    }

    /// Admit one identified peer to an [`RequestPolicy::AllowList`] handler (RNS 1.3.5's `allowed_list`)
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

    /// Boot-restore for one snapshot row, refusing what storage may have forged: the address binding re-derives and the announce signature re-verifies before anything lands.
    /// RNS 1.3.5's load path instead re-reads the cached announce packet and counts the cache read as a hop (`announce_packet.hops += 1`); seeding writes the row directly, so `hops` carries verbatim.
    /// A seeded row's interface gets the departed grace (`Departure::MayReturn`), holding the route warm until the medium re-derives the same id at attach.
    pub fn seed_route(
        &mut self,
        row: &PersistedRouteRow<'_>,
        now: InstantMillis,
    ) -> RouteSeedOutcome {
        let announce = Announce {
            destination: row.destination,
            public_keys: row.public_keys,
            dotted_name_hash: row.dotted_name_hash,
            announce_id: row.announce_id,
            ratchet: row.ratchet,
            signature: row.signature,
            app_data: row.app_data,
        };
        let identity_hash = derive_identity_hash(
            &announce.public_keys.encryption,
            &announce.public_keys.signing,
        );
        if derive_destination_hash(&identity_hash, &announce.dotted_name_hash)
            != announce.destination
        {
            return RouteSeedOutcome::RefusedDestinationMismatch;
        }
        if !announce.signature_is_valid() {
            return RouteSeedOutcome::RefusedInvalidSignature;
        }
        match self.routing_table.seed_route(row) {
            SeedRouteOutcome::Seeded => {
                self.departed_interfaces.record(
                    row.entry.receiving_interface,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSeedOutcome {
    Seeded,
    RefusedDestinationMismatch,
    RefusedInvalidSignature,
    AlreadyPresent,
    TableFull,
    AppDataArenaFull,
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
        let mut state = EngineState::<TestStorageLayout>::default();
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
                last_relayed_at: InstantMillis(700),
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
        assert_eq!(
            state.seed_route(&row, InstantMillis(1_000)),
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
}
