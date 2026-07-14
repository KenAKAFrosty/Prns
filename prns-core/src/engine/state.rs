use crate::crypto::ratchets::SelfRatchets;
use crate::identity::held::HeldIdentities;
use crate::identity::IDENTITY_SECRET_KEY_LEN;
use crate::interfaces::InterfaceId;
use crate::routing::announce::destination_announce_limit::DestinationAnnounceLimits;
use crate::routing::announce::held::HeldAnnounces;
use crate::routing::announce::interface_announce_limit::InterfaceAnnounceLimits;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::delivery::receipts::Receipts;
use crate::routing::group_keys::GroupKeys;
use crate::routing::links::resources::assembly::{IncomingAssemblies, OutgoingAssemblies};
use crate::routing::links::resources::streamed_open::ResourceOpenLane;
use crate::routing::links::resources::table::{IncomingResources, OutgoingResources};
use crate::routing::links::table::Links;
use crate::routing::links::transported::TransportedLinks;
use crate::routing::path_requests::interface_path_request_limit::InterfacePathRequestLimits;
use crate::routing::path_requests::pending::PendingPathRequests;
use crate::routing::path_requests::recent::RecentPathRequests;
use crate::routing::path_requests::recursive::RecursivePathRequests;
use crate::routing::path_requests::seen::SeenPathRequests;
use crate::routing::request_handlers::RequestHandlers;
use crate::routing::reverse_routes::ReverseRoutes;
use crate::routing::tunnel::Tunnels;
use crate::routing::upstream_app_destinations::UpstreamAppDestinations;
use crate::routing::warmth::{DepartedInterfaces, Departure};
use crate::routing::RoutingTable;
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::wire::TransportId;
use zeroize::Zeroizing;

type EngineRoutingTable<S> = RoutingTable<
    <S as StorageLayout>::Routes,
    <S as StorageLayout>::Announces,
    <S as StorageLayout>::History,
    <S as StorageLayout>::AppData,
    <S as StorageLayout>::RouteExpiries,
>;

pub struct EngineState<S: StorageLayout> {
    pub(crate) ingested_packet_count: u64,
    pub(crate) ingested_command_count: u64,
    pub(crate) routing_table: EngineRoutingTable<S>,
    pub(crate) scheduled_announces: S::ScheduledAnnounces,
    pub(crate) upstream_app_destinations: UpstreamAppDestinations<S::UpstreamAppDestinations>,
    pub(crate) packet_hash_history: S::PacketHashes,
    pub(crate) held_identities: HeldIdentities<S::HeldIdentities>,
    pub(crate) transport_id: Option<TransportId>,
    pub(crate) self_ratchets: SelfRatchets<S::SelfRatchets>,
    pub(crate) receipts: Receipts<S::Receipts>,
    pub(crate) reverse_routes: ReverseRoutes<S::ReverseRoutes>,
    pub(crate) pending_path_requests: PendingPathRequests<S::PendingPathRequests>,
    pub(crate) recent_path_requests: RecentPathRequests<S::RecentPathRequests>,
    pub(crate) seen_path_requests: SeenPathRequests<S::SeenPathRequests>,
    pub(crate) tunnels: Tunnels<S::Tunnels>,
    pub(crate) recursive_path_requests: RecursivePathRequests<S::RecursivePathRequests>,
    pub(crate) interface_path_request_limits:
        InterfacePathRequestLimits<S::InterfacePathRequestLimits>,
    pub(crate) interface_announce_limits: InterfaceAnnounceLimits<S::InterfaceAnnounceLimits>,
    pub(crate) held_announces: HeldAnnounces<S::HeldAnnounces, S::HeldAnnounceAppData>,
    pub(crate) destination_announce_limits: DestinationAnnounceLimits<S::DestinationAnnounceLimits>,
    pub(crate) group_keys: GroupKeys<S::GroupKeys>,
    pub(crate) request_handlers: RequestHandlers<S::RequestHandlers>,
    pub(crate) transported_links: TransportedLinks<S::TransportedLinks>,
    pub(crate) links: Links<S::Links>,
    pub(crate) outgoing_resources: OutgoingResources<S::OutgoingResources>,
    pub(crate) incoming_resources: IncomingResources<S::IncomingResources>,
    pub resource_open_lane: ResourceOpenLane,
    pub(crate) incoming_assemblies: IncomingAssemblies<S::IncomingAssemblies>,
    pub(crate) outgoing_assemblies: OutgoingAssemblies<S::OutgoingAssemblies>,
    pub(crate) channels: S::Channels,
    pub(crate) dirty_interfaces: S::DirtyInterfaces,
    pub(crate) departed_interfaces: DepartedInterfaces<S::DepartedInterfaces>,
}

//Hand-written because derive(Default) would put a spurious S: Default bound on the layout parameter.
//Only the column types it names need defaults, not the layout marker itself.
impl<S: StorageLayout> Default for EngineState<S> {
    fn default() -> Self {
        Self {
            ingested_packet_count: 0,
            ingested_command_count: 0,
            routing_table: Default::default(),
            scheduled_announces: Default::default(),
            upstream_app_destinations: UpstreamAppDestinations::default(),
            packet_hash_history: Default::default(),
            held_identities: HeldIdentities::default(),
            transport_id: None,
            self_ratchets: SelfRatchets::default(),
            receipts: Receipts::default(),
            reverse_routes: ReverseRoutes::default(),
            pending_path_requests: PendingPathRequests::default(),
            recent_path_requests: RecentPathRequests::default(),
            seen_path_requests: SeenPathRequests::default(),
            tunnels: Tunnels::default(),
            recursive_path_requests: RecursivePathRequests::default(),
            interface_path_request_limits: InterfacePathRequestLimits::default(),
            interface_announce_limits: InterfaceAnnounceLimits::default(),
            held_announces: HeldAnnounces::default(),
            destination_announce_limits: DestinationAnnounceLimits::default(),
            group_keys: GroupKeys::default(),
            request_handlers: RequestHandlers::default(),
            transported_links: TransportedLinks::default(),
            links: Links::default(),
            outgoing_resources: OutgoingResources::default(),
            incoming_resources: IncomingResources::default(),
            resource_open_lane: ResourceOpenLane::default(),
            incoming_assemblies: IncomingAssemblies::default(),
            outgoing_assemblies: OutgoingAssemblies::default(),
            channels: Default::default(),
            dirty_interfaces: Default::default(),
            departed_interfaces: DepartedInterfaces::default(),
        }
    }
}

impl<S: StorageLayout> core::fmt::Debug for EngineState<S>
where
    S::Routes: core::fmt::Debug,
    S::Announces: core::fmt::Debug,
    S::History: core::fmt::Debug,
    S::AppData: core::fmt::Debug,
    S::ScheduledAnnounces: core::fmt::Debug,
    S::UpstreamAppDestinations: core::fmt::Debug,
    S::PacketHashes: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EngineState")
            .field("ingested_packet_count", &self.ingested_packet_count)
            .field("ingested_command_count", &self.ingested_command_count)
            .field("routing_table", &self.routing_table)
            .field("scheduled_announces", &self.scheduled_announces)
            .field("upstream_app_destinations", &self.upstream_app_destinations)
            .field("packet_hash_history", &self.packet_hash_history)
            .field("held_identities", &self.held_identities)
            .field("transport_id", &self.transport_id)
            .field("self_ratchets", &self.self_ratchets)
            .finish_non_exhaustive()
    }
}

impl<S: StorageLayout> EngineState<S> {
    /// # Panics
    /// Panics if `S` declares a zero-capacity held-identities column; such a layout cannot run a node.
    #[expect(
        clippy::expect_used,
        reason = "only a zero-capacity held-identities layout can fail here, and no caller can recover from choosing one"
    )]
    pub fn new(identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> Self {
        let mut state = Self::default();
        let identity = state
            .hold_identity(identity_secret_key)
            .expect("an empty store holds the first identity");
        state.transport_id = Some(TransportId::new(*identity.as_bytes()));
        state
    }

    pub const fn ingested_packet_count(&self) -> u64 {
        self.ingested_packet_count
    }

    pub const fn ingested_command_count(&self) -> u64 {
        self.ingested_command_count
    }

    pub fn route_count(&self) -> usize {
        self.routing_table.route_count()
    }

    pub fn route_count_via(&self, interface: InterfaceId) -> usize {
        self.routing_table.route_count_via(interface)
    }

    pub fn link_count_via(&self, interface: InterfaceId) -> usize {
        self.links.link_count_via(interface)
    }

    pub fn transported_link_count_via(&self, interface: InterfaceId) -> usize {
        self.transported_links.transported_link_count_via(interface)
    }

    pub(crate) fn mark_interface_dirty(&mut self, interface: InterfaceId) {
        self.dirty_interfaces.mark(interface);
    }

    pub fn take_dirty_interfaces(&mut self) -> S::DirtyInterfaces {
        core::mem::take(&mut self.dirty_interfaces)
    }

    pub fn interface_attached(&mut self, interface: InterfaceId, now: crate::units::InstantMillis) {
        self.interface_announce_limits
            .interface_attached(interface, now);
        self.routing_table.invalidate_route_expiries();
    }

    pub fn interface_departed(
        &mut self,
        interface: InterfaceId,
        departure: Departure,
        now: crate::units::InstantMillis,
    ) {
        match departure {
            Departure::Forgotten => self.held_announces.drop_interface(interface),
            Departure::MayReturn => {}
        }
        self.departed_interfaces.record(interface, departure, now);
        self.routing_table.invalidate_route_expiries();
    }

    pub fn scheduled_announce_count(&self) -> usize {
        self.scheduled_announces.scheduled_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::interfaces::AttachedInterfaces;
    use crate::interfaces::InboundPacket;
    use crate::storage::TestFixedStorage;
    use crate::units::InstantMillis;

    #[test]
    fn a_capable_host_can_widen_the_routing_table_at_the_type_level() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state =
            EngineState::<TestFixedStorage<64, 128, 4096, 8, 8, 128, 8, 8, 8, 8, 16, 16>>::default(
            );
        pin_transport_id(&mut state, TEST_TRANSPORT_ID);
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(out, rns_1_3_5_announce_accepted(1));
        assert_eq!(state.route_count(), 1);
    }

    #[test]
    fn a_forgotten_interface_drops_its_held_announces_a_may_return_keeps_them() {
        use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
        use crate::engine::Departure;
        use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
        use crate::routing::announce::{Announce, AnnounceId, DottedNameHash, IdentityPublicKeys};
        use crate::routing::NextHop;
        use crate::wire::DestinationHash;

        let source = InterfaceId::new([0xA1; 8]);
        let mut engine = EngineState::<
            TestFixedStorage<64, 128, 4096, 8, 8, 128, 8, 8, 8, 8, 16, 16>,
        >::default();
        let announce = Announce {
            destination: DestinationHash::new([0x42; 16]),
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            announce_id: AnnounceId::from_wire([0x01; 10]),
            ratchet: None,
            signature: Ed25519Signature([0u8; 64]),
            app_data: b"held",
        };

        engine
            .held_announces
            .hold(3, source, NextHop::Direct, false, &announce);
        assert_eq!(engine.held_announces.len(), 1);
        engine.interface_departed(source, Departure::Forgotten, InstantMillis(2_000));
        assert!(
            engine.held_announces.is_empty(),
            "a forgotten interface drops what it was holding",
        );

        engine
            .held_announces
            .hold(3, source, NextHop::Direct, false, &announce);
        engine.interface_departed(source, Departure::MayReturn, InstantMillis(3_000));
        assert_eq!(
            engine.held_announces.len(),
            1,
            "a may-return interface keeps its held announces to drain",
        );
    }
}
