use crate::crypto::ratchets::SelfRatchets;
use crate::identity::destination_identity::DestinationIdentities;
use crate::identity::held::HeldIdentities;
use crate::identity::IDENTITY_SECRET_KEY_LEN;
use crate::interfaces::InterfaceId;
use crate::routing::announce::destination_announce_limit::DestinationAnnounceLimits;
use crate::routing::announce::held::HeldAnnounces;
use crate::routing::announce::interface_announce_limit::InterfaceAnnounceLimits;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::blackhole::IdentityBlackholes;
use crate::routing::delivery::receipts::Receipts;
use crate::routing::group_keys::GroupKeys;
use crate::routing::links::resources::assembly::{IncomingAssemblies, OutgoingAssemblies};
use crate::routing::links::resources::pending::PendingResourceOffers;
use crate::routing::links::resources::streamed_open::ResourceOpenLane;
use crate::routing::links::resources::table::{IncomingResources, OutgoingResources};
#[cfg(feature = "alloc")]
use crate::routing::links::resources::ResourceMemoryLimits;
use crate::routing::links::table::Links;
use crate::routing::links::transported::TransportedLinks;
use crate::routing::path_requests::interface_path_request_limit::InterfacePathRequestLimits;
use crate::routing::path_requests::pending::PendingPathRequests;
use crate::routing::path_requests::recent::RecentPathRequests;
use crate::routing::path_requests::recursive::RecursivePathRequests;
use crate::routing::path_requests::seen::SeenPathRequests;
use crate::routing::request_handlers::RequestHandlers;
use crate::routing::reverse_routes::ReverseRoutes;
use crate::routing::routes::RouteEvidenceIdIssuer;
use crate::routing::tunnel::Tunnels;
use crate::routing::upstream_app_destinations::UpstreamAppDestinations;
use crate::routing::warmth::{DepartedInterfaces, Departure};
use crate::routing::RoutingTable;
#[cfg(feature = "alloc")]
use crate::storage::GrowableHeap;
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::wire::TransportId;
use core::mem::MaybeUninit;
use zeroize::Zeroizing;

#[cfg(feature = "runtime-metrics")]
use alloc::vec::Vec;

type EngineRoutingTable<S> = RoutingTable<
    <S as StorageLayout>::Routes,
    <S as StorageLayout>::Announces,
    <S as StorageLayout>::History,
    <S as StorageLayout>::AppData,
    <S as StorageLayout>::RouteExpiries,
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofForm {
    Implicit,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMtuDiscovery {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecursivePathRequestDefault {
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalOriginHopCount(u8);

impl LocalOriginHopCount {
    pub const fn new(hops: u8) -> Option<Self> {
        if hops >= 2 && hops <= 7 {
            Some(Self(hops))
        } else {
            None
        }
    }

    pub const fn from_entropy(entropy: u8) -> Self {
        Self(2 + entropy % 6)
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalHopCountOverride {
    #[default]
    Disabled,
    Override(LocalOriginHopCount),
}

impl LocalHopCountOverride {
    pub const fn override_with(hops: u8) -> Option<Self> {
        match LocalOriginHopCount::new(hops) {
            Some(hops) => Some(Self::Override(hops)),
            None => None,
        }
    }

    pub const fn from_entropy(entropy: u8) -> Self {
        Self::Override(LocalOriginHopCount::from_entropy(entropy))
    }

    pub const fn apply(self, original_hops: u8, crosses_local_boundary: bool) -> u8 {
        match (self, crosses_local_boundary) {
            (Self::Override(hops), true) => hops.get(),
            _ => original_hops,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineProtocolPolicy {
    pub proof_form: ProofForm,
    pub link_mtu_discovery: LinkMtuDiscovery,
    pub local_hop_count_override: LocalHopCountOverride,
    pub recursive_path_request_default: RecursivePathRequestDefault,
}

impl Default for EngineProtocolPolicy {
    fn default() -> Self {
        Self {
            proof_form: ProofForm::Implicit,
            link_mtu_discovery: LinkMtuDiscovery::Enabled,
            local_hop_count_override: LocalHopCountOverride::Disabled,
            recursive_path_request_default: RecursivePathRequestDefault::Disabled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkTransport {
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TransportState {
    #[default]
    Unidentified,
    Identified {
        id: TransportId,
        network: NetworkTransport,
    },
}

impl TransportState {
    pub(crate) const fn id(self) -> Option<TransportId> {
        match self {
            Self::Unidentified => None,
            Self::Identified { id, .. } => Some(id),
        }
    }

    pub(crate) const fn network_transport_enabled(self) -> bool {
        matches!(
            self,
            Self::Identified {
                network: NetworkTransport::Enabled,
                ..
            }
        )
    }
}

pub struct EngineState<S: StorageLayout> {
    pub(crate) ingested_packet_count: u64,
    pub(crate) ingested_command_count: u64,
    #[cfg(feature = "runtime-metrics")]
    pub(crate) ignored_packet_counts: super::IgnoreReasonCounts,
    #[cfg(feature = "runtime-metrics")]
    pub(crate) announce_ingress_counts: super::AnnounceIngressCounts,
    #[cfg(feature = "runtime-metrics")]
    pub(crate) announce_accepted_interface_counts: super::InterfaceKindCounts,
    #[cfg(feature = "runtime-metrics")]
    pub(crate) announce_command_counts: super::AnnounceCommandCounts,
    #[cfg(feature = "runtime-metrics")]
    pub(crate) announce_interface_metrics: Vec<super::InterfaceAnnounceMetricsSnapshot>,
    #[cfg(feature = "runtime-metrics")]
    pub(crate) interface_metric_groups: Vec<super::metrics::InterfaceMetricGroup>,
    #[cfg(feature = "runtime-metrics")]
    pub(crate) path_request_ingress_counts: super::PathRequestIngressCounts,
    #[cfg(feature = "runtime-metrics")]
    pub(crate) path_request_relay_counts: super::PathRequestRelayCounts,
    #[cfg(feature = "runtime-metrics")]
    pub(crate) resource_admission_event_counts: super::ResourceAdmissionEventCounts,
    pub(crate) routing_table: EngineRoutingTable<S>,
    pub(crate) route_evidence_id_issuer: RouteEvidenceIdIssuer,
    pub(crate) destination_identities:
        DestinationIdentities<S::DestinationIdentities, S::DestinationIdentityAppData>,
    pub(crate) scheduled_announces: S::ScheduledAnnounces,
    pub(crate) upstream_app_destinations: UpstreamAppDestinations<S::UpstreamAppDestinations>,
    pub(crate) packet_hash_history: S::PacketHashes,
    pub(crate) identity_blackholes: IdentityBlackholes<S::Blackholes>,
    pub(crate) held_identities: HeldIdentities<S::HeldIdentities>,
    pub(crate) transport: TransportState,
    pub(crate) protocol: EngineProtocolPolicy,
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
    pub(crate) egress_path_request_limits:
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
    pub(crate) pending_resource_offers: PendingResourceOffers<S::PendingResourceOffers>,
    pub resource_open_lane: ResourceOpenLane,
    pub(crate) incoming_assemblies: IncomingAssemblies<S::IncomingAssemblies>,
    pub(crate) outgoing_assemblies: OutgoingAssemblies<S::OutgoingAssemblies>,
    pub(crate) channels: S::Channels,
    pub(crate) dirty_interfaces: S::DirtyInterfaces,
    pub(crate) departed_interfaces: DepartedInterfaces<S::DepartedInterfaces>,
}

impl<S: StorageLayout> Default for EngineState<S> {
    fn default() -> Self {
        Self {
            ingested_packet_count: 0,
            ingested_command_count: 0,
            #[cfg(feature = "runtime-metrics")]
            ignored_packet_counts: Default::default(),
            #[cfg(feature = "runtime-metrics")]
            announce_ingress_counts: Default::default(),
            #[cfg(feature = "runtime-metrics")]
            announce_accepted_interface_counts: Default::default(),
            #[cfg(feature = "runtime-metrics")]
            announce_command_counts: Default::default(),
            #[cfg(feature = "runtime-metrics")]
            announce_interface_metrics: Vec::new(),
            #[cfg(feature = "runtime-metrics")]
            interface_metric_groups: Vec::new(),
            #[cfg(feature = "runtime-metrics")]
            path_request_ingress_counts: Default::default(),
            #[cfg(feature = "runtime-metrics")]
            path_request_relay_counts: Default::default(),
            #[cfg(feature = "runtime-metrics")]
            resource_admission_event_counts: Default::default(),
            routing_table: Default::default(),
            route_evidence_id_issuer: RouteEvidenceIdIssuer::default(),
            destination_identities: DestinationIdentities::default(),
            scheduled_announces: Default::default(),
            upstream_app_destinations: UpstreamAppDestinations::default(),
            packet_hash_history: Default::default(),
            identity_blackholes: IdentityBlackholes::default(),
            held_identities: HeldIdentities::default(),
            transport: TransportState::default(),
            protocol: EngineProtocolPolicy::default(),
            self_ratchets: SelfRatchets::default(),
            receipts: Receipts::default(),
            reverse_routes: ReverseRoutes::default(),
            pending_path_requests: PendingPathRequests::default(),
            recent_path_requests: RecentPathRequests::default(),
            seen_path_requests: SeenPathRequests::default(),
            tunnels: Tunnels::default(),
            recursive_path_requests: RecursivePathRequests::default(),
            interface_path_request_limits: InterfacePathRequestLimits::default(),
            egress_path_request_limits: InterfacePathRequestLimits::default(),
            interface_announce_limits: InterfaceAnnounceLimits::default(),
            held_announces: HeldAnnounces::default(),
            destination_announce_limits: DestinationAnnounceLimits::default(),
            group_keys: GroupKeys::default(),
            request_handlers: RequestHandlers::default(),
            transported_links: TransportedLinks::default(),
            links: Links::default(),
            outgoing_resources: OutgoingResources::default(),
            incoming_resources: IncomingResources::default(),
            pending_resource_offers: PendingResourceOffers::default(),
            resource_open_lane: ResourceOpenLane::default(),
            incoming_assemblies: IncomingAssemblies::default(),
            outgoing_assemblies: OutgoingAssemblies::default(),
            channels: Default::default(),
            dirty_interfaces: Default::default(),
            departed_interfaces: DepartedInterfaces::default(),
        }
    }
}

impl<S: StorageLayout> EngineState<S> {
    #[expect(
        unsafe_code,
        clippy::undocumented_unsafe_blocks,
        reason = "each field is written exactly once before returning the initialized value"
    )]
    #[doc(hidden)]
    pub fn init_in_place(slot: &mut MaybeUninit<Self>) -> &mut Self {
        let state = slot.as_mut_ptr();
        macro_rules! write {
            ($field:ident, $value:expr) => {
                core::ptr::addr_of_mut!((*state).$field).write($value)
            };
        }
        unsafe {
            write!(ingested_packet_count, 0);
            write!(ingested_command_count, 0);
            #[cfg(feature = "runtime-metrics")]
            write!(ignored_packet_counts, Default::default());
            #[cfg(feature = "runtime-metrics")]
            write!(announce_ingress_counts, Default::default());
            #[cfg(feature = "runtime-metrics")]
            write!(announce_accepted_interface_counts, Default::default());
            #[cfg(feature = "runtime-metrics")]
            write!(announce_command_counts, Default::default());
            #[cfg(feature = "runtime-metrics")]
            write!(announce_interface_metrics, Vec::new());
            #[cfg(feature = "runtime-metrics")]
            write!(interface_metric_groups, Vec::new());
            #[cfg(feature = "runtime-metrics")]
            write!(path_request_ingress_counts, Default::default());
            #[cfg(feature = "runtime-metrics")]
            write!(path_request_relay_counts, Default::default());
            #[cfg(feature = "runtime-metrics")]
            write!(resource_admission_event_counts, Default::default());
            write!(routing_table, Default::default());
            write!(route_evidence_id_issuer, RouteEvidenceIdIssuer::default());
            write!(destination_identities, DestinationIdentities::default());
            write!(scheduled_announces, Default::default());
            write!(
                upstream_app_destinations,
                UpstreamAppDestinations::default()
            );
            write!(packet_hash_history, Default::default());
            write!(identity_blackholes, IdentityBlackholes::default());
            write!(held_identities, HeldIdentities::default());
            write!(transport, TransportState::default());
            write!(protocol, EngineProtocolPolicy::default());
            write!(self_ratchets, SelfRatchets::default());
            write!(receipts, Receipts::default());
            write!(reverse_routes, ReverseRoutes::default());
            write!(pending_path_requests, PendingPathRequests::default());
            write!(recent_path_requests, RecentPathRequests::default());
            write!(seen_path_requests, SeenPathRequests::default());
            write!(tunnels, Tunnels::default());
            write!(recursive_path_requests, RecursivePathRequests::default());
            write!(
                interface_path_request_limits,
                InterfacePathRequestLimits::default()
            );
            write!(
                egress_path_request_limits,
                InterfacePathRequestLimits::default()
            );
            write!(
                interface_announce_limits,
                InterfaceAnnounceLimits::default()
            );
            write!(held_announces, HeldAnnounces::default());
            write!(
                destination_announce_limits,
                DestinationAnnounceLimits::default()
            );
            write!(group_keys, GroupKeys::default());
            write!(request_handlers, RequestHandlers::default());
            write!(transported_links, TransportedLinks::default());
            write!(links, Links::default());
            write!(outgoing_resources, OutgoingResources::default());
            write!(incoming_resources, IncomingResources::default());
            write!(pending_resource_offers, PendingResourceOffers::default());
            write!(resource_open_lane, ResourceOpenLane::default());
            write!(incoming_assemblies, IncomingAssemblies::default());
            write!(outgoing_assemblies, OutgoingAssemblies::default());
            write!(channels, Default::default());
            write!(dirty_interfaces, Default::default());
            write!(departed_interfaces, DepartedInterfaces::default());
            slot.assume_init_mut()
        }
    }
}

#[cfg(feature = "alloc")]
impl EngineState<GrowableHeap> {
    /// Replaces the independent incoming and outgoing active Resource buffer
    /// budgets used by heap hosts. Existing transfers remain active if a limit
    /// is lowered beneath their current usage; new rows wait for usage to fall
    /// back within the new limit.
    pub fn set_resource_memory_limits(&mut self, limits: ResourceMemoryLimits) {
        self.incoming_resources
            .set_memory_limit(limits.incoming_bytes);
        self.outgoing_resources
            .set_memory_limit(limits.outgoing_bytes);
    }

    #[must_use]
    pub fn resource_memory_limits(&self) -> ResourceMemoryLimits {
        ResourceMemoryLimits {
            incoming_bytes: self.incoming_resources.memory_limit(),
            outgoing_bytes: self.outgoing_resources.memory_limit(),
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
    S::Blackholes: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EngineState")
            .field("ingested_packet_count", &self.ingested_packet_count)
            .field("ingested_command_count", &self.ingested_command_count)
            .field("routing_table", &self.routing_table)
            .field(
                "destination_identity_count",
                &self.destination_identities.len(),
            )
            .field("scheduled_announces", &self.scheduled_announces)
            .field("upstream_app_destinations", &self.upstream_app_destinations)
            .field("packet_hash_history", &self.packet_hash_history)
            .field("identity_blackholes", &self.identity_blackholes)
            .field("held_identities", &self.held_identities)
            .field("transport", &self.transport)
            .field("self_ratchets", &self.self_ratchets)
            .finish_non_exhaustive()
    }
}

impl<S: StorageLayout> EngineState<S> {
    pub fn set_protocol_policy(&mut self, policy: EngineProtocolPolicy) {
        self.protocol = policy;
    }

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
        state.transport = TransportState::Identified {
            id: TransportId::new(*identity.as_bytes()),
            network: NetworkTransport::Enabled,
        };
        state
    }

    pub const fn ingested_packet_count(&self) -> u64 {
        self.ingested_packet_count
    }

    pub const fn ingested_command_count(&self) -> u64 {
        self.ingested_command_count
    }

    #[cfg(feature = "runtime-metrics")]
    pub fn metrics_snapshot(&self) -> super::EngineMetricsSnapshot {
        super::EngineMetricsSnapshot {
            ingested_packets: self.ingested_packet_count,
            ingested_commands: self.ingested_command_count,
            ignored_packets: self.ignored_packet_counts,
            announces: super::EngineAnnounceMetricsSnapshot {
                ingress: self.announce_ingress_counts,
                accepted_by_interface_kind: self.announce_accepted_interface_counts,
                commands: self.announce_command_counts,
                held_depth: u32::try_from(self.held_announces.len()).unwrap_or(u32::MAX),
                scheduled_depth: u32::try_from(self.scheduled_announces.scheduled_count())
                    .unwrap_or(u32::MAX),
                interfaces: self.interface_announce_metrics_snapshot(),
            },
            path_requests: super::EnginePathRequestMetricsSnapshot {
                ingress: self.path_request_ingress_counts,
                relays: self.path_request_relay_counts,
                pending_discoveries: u32::try_from(self.recursive_path_requests.in_flight_count())
                    .unwrap_or(u32::MAX),
            },
            resources: super::EngineResourceMetricsSnapshot {
                incoming: super::ResourceDirectionMetricsSnapshot {
                    active_buffer_bytes: u64::try_from(
                        self.incoming_resources.active_buffer_bytes(),
                    )
                    .unwrap_or(u64::MAX),
                    buffer_budget_bytes: u64::try_from(
                        self.incoming_resources.buffer_memory_limit(),
                    )
                    .unwrap_or(u64::MAX),
                    active_rows: u32::try_from(self.incoming_resources.len()).unwrap_or(u32::MAX),
                },
                outgoing: super::ResourceDirectionMetricsSnapshot {
                    active_buffer_bytes: u64::try_from(
                        self.outgoing_resources.active_buffer_bytes(),
                    )
                    .unwrap_or(u64::MAX),
                    buffer_budget_bytes: u64::try_from(
                        self.outgoing_resources.buffer_memory_limit(),
                    )
                    .unwrap_or(u64::MAX),
                    active_rows: u32::try_from(self.outgoing_resources.len()).unwrap_or(u32::MAX),
                },
                pending_depth: u32::try_from(self.pending_resource_offers.len())
                    .unwrap_or(u32::MAX),
                admission_events: self.resource_admission_event_counts,
            },
            route_count: u32::try_from(self.routing_table.route_count()).unwrap_or(u32::MAX),
            link_count: u32::try_from(self.links.active_link_count()).unwrap_or(u32::MAX),
            transported_link_count: u32::try_from(self.transported_links.validated_count())
                .unwrap_or(u32::MAX),
        }
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
        #[cfg(feature = "runtime-metrics")]
        self.detach_metrics_interface_if_idle(interface);
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
    fn blackhole_state_is_owned_by_the_engine_layout() {
        let state = EngineState::<TestStorageLayout>::default();

        assert!(state.identity_blackholes.is_empty());
    }

    #[test]
    fn default_protocol_policy_keeps_host_recursive_discovery_disabled() {
        assert_eq!(
            EngineProtocolPolicy::default().recursive_path_request_default,
            RecursivePathRequestDefault::Disabled,
        );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn heap_resource_memory_limits_are_public_directional_configuration() {
        let mut state = EngineState::<GrowableHeap>::default();
        assert_eq!(
            state.resource_memory_limits(),
            ResourceMemoryLimits::DEFAULT_HOST,
        );

        let limits = ResourceMemoryLimits {
            incoming_bytes: 123,
            outgoing_bytes: 456,
        };
        state.set_resource_memory_limits(limits);
        assert_eq!(state.resource_memory_limits(), limits);
    }

    #[test]
    fn local_hop_count_override_is_a_replacement_not_an_arithmetic_delta() {
        assert_eq!(LocalHopCountOverride::override_with(1), None);
        assert_eq!(LocalHopCountOverride::override_with(8), None);
        assert_eq!(LocalHopCountOverride::Disabled.apply(4, true), 4);
        let replacement = LocalHopCountOverride::from_entropy(3);
        assert_eq!(replacement.apply(4, false), 4);
        assert_eq!(replacement.apply(0, true), 5);
        assert_eq!(replacement.apply(4, true), 5);
        for entropy in u8::MIN..=u8::MAX {
            let LocalHopCountOverride::Override(hops) =
                LocalHopCountOverride::from_entropy(entropy)
            else {
                unreachable!()
            };
            assert!((2..=7).contains(&hops.get()));
        }
    }

    #[test]
    fn a_capable_host_can_widen_the_routing_table_at_the_type_level() {
        let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
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
        assert_eq!(out, rns_1_4_2_announce_accepted(1));
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
