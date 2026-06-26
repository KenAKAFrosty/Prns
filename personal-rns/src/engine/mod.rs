mod command;
pub mod commands;
pub mod egress;
pub mod identity_registration;
mod inbound;
pub mod reaction;
mod scheduled;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tick;
mod tunnel;

#[cfg(feature = "alloc")]
pub use commands::RpcPathEntry;
pub use commands::{
    AllowRequester, AllowRequesterError, AllowRequesterFailure, AnnounceAppData, AnnounceNow,
    AnnounceNowError, AnnounceNowFailure, AnnounceTarget, CloseLink, CloseLinkError,
    CloseLinkFailure, CommandId, CommandOutcome, Delivered, EngineCommand, EstablishLink,
    EstablishLinkError, EstablishLinkFailure, Identify, IdentifyError, IdentifyFailure,
    InterfaceCounts, IssuedCommand, LinkEstablished, PathFound, PathRequestId, RequestPath,
    RequestPathFailure, Respond, RespondData, RespondError, RespondFailure, RpcQuery,
    RpcQueryResult, SendChannel, SendChannelBody, SendChannelError, SendChannelFailure, SendGroup,
    SendGroupFailure, SendGroupPayload, SendLink, SendLinkError, SendLinkFailure, SendLinkPayload,
    SendRequest, SendRequestData, SendRequestError, SendRequestFailure, SendResourceError,
    SendResourceFailure, SendSingle, SendSingleError, SendSingleFailure, SendSinglePayload,
    SetResourceStrategy, SetResourceStrategyError, SetResourceStrategyFailure, Settleable,
    Settlement, MAX_SEND_CHANNEL_BODY_LEN, MAX_SEND_GROUP_PLAINTEXT_LEN,
    MAX_SEND_LINK_PLAINTEXT_LEN, MAX_SEND_SINGLE_PLAINTEXT_LEN, PATH_REQUEST_ID_LEN,
};
pub use egress::{
    write_path_request_wire_packet, EgressDirective, EgressSerializeError,
    PATH_REQUEST_DESTINATION, PATH_REQUEST_PAYLOAD_LEN,
};
pub use identity_registration::SetTransportIdentityError;
pub use reaction::{Directive, EngineReaction, FanTarget, Journaled};

pub use crate::crypto::ratchets::{RatchetEntropy, RatchetPolicy, RatchetRotation};
pub use crate::routing::announce::emit::{
    AnnounceAppDataBytes, AnnounceRejection, AnnounceWriteFailure, CommandedAnnounceWriteOutcome,
    PathResponseWriteOutcome, WriteAnnounceError,
};
pub use crate::routing::delivery::send_group::WriteSendGroupError;
pub use crate::routing::delivery::send_single::{
    EncryptOwed, FinishSendSingleOutcome, SendSingleDispatch, SendSingleEntropy,
    SendSinglePrepared, SendSingleRejection, SendSingleWriteOutcome, WriteSendSingleError,
};
pub use crate::routing::ingress::{
    AcceptedAnnounce, AnnounceIngest, DataPacket, IngestPacketOutcome, Ingress, PacketToForward,
    RebroadcastDecision,
};
pub use crate::routing::links::data::{
    link_mdu, LinkDataError, SendLinkDispatch, SendLinkWriteError, LINK_MDU,
};
pub use crate::routing::links::establish::{
    EstablishLinkEntropy, EstablishLinkWriteOutcome, LinkRequestDispatch, WriteEstablishLinkError,
    WriteLinkProofError, WriteLinkRttError, LINK_KEEPALIVE_MS,
};
pub use crate::routing::links::maintenance::{
    keepalive_ms_from, stale_ms_from, write_keepalive, write_link_close, LinkCloseDispatch,
    WriteLinkCloseError, KEEPALIVE_ECHO, KEEPALIVE_REQUEST,
};
pub use crate::routing::path_requests::pending::{
    CulledPathRequest, ExpiredPathRequest, SettledPathRequest, PATH_REQUEST_TIMEOUT_MS,
};
pub use crate::routing::path_requests::request_path::PathRequestWriteOutcome;
pub use crate::routing::path_requests::seen::PathRequestIdBytes;
pub use crate::routing::proof::{
    ProofIngest, ProofObligation, ProofOwed, ProofRequest, WriteProofError,
};
pub use crate::units::InstantMillis;

use crate::crypto::ratchets::SelfRatchets;
use crate::identity::held::HeldIdentities;
use crate::identity::IDENTITY_SECRET_KEY_LEN;
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::routing::announce::held::HeldAnnounces;
use crate::routing::announce::interface_announce_limit::InterfaceAnnounceLimits;
use crate::routing::announce::rate_limit::AnnounceRates;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::delivery::receipts::Receipts;
use crate::routing::group_keys::GroupKeys;
use crate::routing::links::channel::columns::ChannelColumns;
use crate::routing::links::resources::assembly::{IncomingAssemblies, OutgoingAssemblies};
use crate::routing::links::resources::table::{IncomingResources, OutgoingResources};
use crate::routing::links::table::Links;
use crate::routing::links::transported::TransportedLinks;
use crate::routing::path_requests::discovery::DiscoveryPathRequests;
use crate::routing::path_requests::interface_path_request_limit::InterfacePathRequestLimits;
use crate::routing::path_requests::pending::PendingPathRequests;
use crate::routing::path_requests::recent::RecentPathRequests;
use crate::routing::path_requests::seen::SeenPathRequests;
use crate::routing::request_handlers::RequestHandlers;
use crate::routing::reverse_routes::ReverseRoutes;
use crate::routing::tunnel::Tunnels;
use crate::routing::upstream_app_destinations::UpstreamAppDestinations;
use crate::routing::RoutingTable;
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::wire::TransportId;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueLane {
    ScheduledAnnounces,
    ReceiptTimeouts,
    PathRequestTimeout,
    ExpiredRoutes,
    LinkDeadlines,
    ResourceDeadlines,
    ChannelTimeouts,
    HeldAnnounceRelease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledWake {
    Idle,
    Due(DueLane),
    At { at: InstantMillis, lane: DueLane },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneWake {
    Unchanged,
    Idle,
    At(InstantMillis),
    /// The deadline is no later than this instant; a sooner cached one stands. An
    /// emitter uses this when it knows the one entry it touched but not the whole
    /// lane — the cached deadline may end up early, never late, and the premature
    /// wake's full recompute resyncs it exactly.
    AtMost(InstantMillis),
}

impl LaneWake {
    fn from_deadline(earliest: Option<InstantMillis>) -> Self {
        earliest.map_or(LaneWake::Idle, LaneWake::At)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeSchedules {
    pub scheduled_announces: LaneWake,
    pub receipt_timeouts: LaneWake,
    pub path_request_timeout: LaneWake,
    pub expired_routes: LaneWake,
    pub link_deadlines: LaneWake,
    pub resource_deadlines: LaneWake,
    pub channel_timeouts: LaneWake,
    pub held_announce_release: LaneWake,
}

impl WakeSchedules {
    pub const UNCHANGED: Self = Self {
        scheduled_announces: LaneWake::Unchanged,
        receipt_timeouts: LaneWake::Unchanged,
        path_request_timeout: LaneWake::Unchanged,
        expired_routes: LaneWake::Unchanged,
        link_deadlines: LaneWake::Unchanged,
        resource_deadlines: LaneWake::Unchanged,
        channel_timeouts: LaneWake::Unchanged,
        held_announce_release: LaneWake::Unchanged,
    };

    pub fn merge(&mut self, delta: WakeSchedules) {
        for (slot, change) in [
            (&mut self.scheduled_announces, delta.scheduled_announces),
            (&mut self.receipt_timeouts, delta.receipt_timeouts),
            (&mut self.path_request_timeout, delta.path_request_timeout),
            (&mut self.expired_routes, delta.expired_routes),
            (&mut self.link_deadlines, delta.link_deadlines),
            (&mut self.resource_deadlines, delta.resource_deadlines),
            (&mut self.channel_timeouts, delta.channel_timeouts),
            (&mut self.held_announce_release, delta.held_announce_release),
        ] {
            match change {
                LaneWake::Unchanged => {}
                LaneWake::AtMost(ceiling) => {
                    *slot = match *slot {
                        LaneWake::At(cached) if cached <= ceiling => LaneWake::At(cached),
                        _ => LaneWake::At(ceiling),
                    };
                }
                replacement => *slot = replacement,
            }
        }
    }

    pub fn soonest(&self, now: InstantMillis) -> ScheduledWake {
        let mut earliest: Option<(InstantMillis, DueLane)> = None;
        for (wake, lane) in [
            // List order is the deliberate tie-break: when several lanes are due at `now`,
            // `soonest` returns the first in this order — announces, then receipt and
            // path-request timeouts, then route, link, and resource deadlines.
            (self.scheduled_announces, DueLane::ScheduledAnnounces),
            (self.receipt_timeouts, DueLane::ReceiptTimeouts),
            (self.path_request_timeout, DueLane::PathRequestTimeout),
            (self.expired_routes, DueLane::ExpiredRoutes),
            (self.link_deadlines, DueLane::LinkDeadlines),
            (self.resource_deadlines, DueLane::ResourceDeadlines),
            (self.channel_timeouts, DueLane::ChannelTimeouts),
            (self.held_announce_release, DueLane::HeldAnnounceRelease),
        ] {
            match wake {
                LaneWake::Unchanged | LaneWake::Idle => {}
                LaneWake::At(at) | LaneWake::AtMost(at) => {
                    if at <= now {
                        return ScheduledWake::Due(lane);
                    }
                    earliest = merge_earliest(earliest, at, lane);
                }
            }
        }
        match earliest {
            Some((at, lane)) => ScheduledWake::At { at, lane },
            None => ScheduledWake::Idle,
        }
    }
}

pub struct EngineState<S: StorageLayout> {
    pub(crate) ingested_packet_count: u64,
    pub(crate) ingested_command_count: u64,
    pub(crate) routing_table: RoutingTable<S::Routes, S::Announces, S::History, S::AppData>,
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
    pub(crate) discovery_path_requests: DiscoveryPathRequests<S::DiscoveryPathRequests>,
    pub(crate) interface_path_request_limits:
        InterfacePathRequestLimits<S::InterfacePathRequestLimits>,
    pub(crate) interface_announce_limits: InterfaceAnnounceLimits<S::InterfaceAnnounceLimits>,
    pub(crate) held_announces: HeldAnnounces<S::HeldAnnounces, S::HeldAnnounceAppData>,
    pub(crate) announce_rates: AnnounceRates<S::AnnounceRates>,
    pub(crate) group_keys: GroupKeys<S::GroupKeys>,
    pub(crate) request_handlers: RequestHandlers<S::RequestHandlers>,
    pub(crate) transported_links: TransportedLinks<S::TransportedLinks>,
    pub(crate) links: Links<S::Links>,
    pub(crate) outgoing_resources: OutgoingResources<S::OutgoingResources>,
    pub(crate) incoming_resources: IncomingResources<S::IncomingResources>,
    pub(crate) incoming_assemblies: IncomingAssemblies<S::IncomingAssemblies>,
    pub(crate) outgoing_assemblies: OutgoingAssemblies<S::OutgoingAssemblies>,
    pub(crate) channels: S::Channels,
    pub(crate) dirty_interfaces: S::DirtyInterfaces,
}

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
            discovery_path_requests: DiscoveryPathRequests::default(),
            interface_path_request_limits: InterfacePathRequestLimits::default(),
            interface_announce_limits: InterfaceAnnounceLimits::default(),
            held_announces: HeldAnnounces::default(),
            announce_rates: AnnounceRates::default(),
            group_keys: GroupKeys::default(),
            request_handlers: RequestHandlers::default(),
            transported_links: TransportedLinks::default(),
            links: Links::default(),
            outgoing_resources: OutgoingResources::default(),
            incoming_resources: IncomingResources::default(),
            incoming_assemblies: IncomingAssemblies::default(),
            outgoing_assemblies: OutgoingAssemblies::default(),
            channels: Default::default(),
            dirty_interfaces: Default::default(),
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
            .finish()
    }
}

impl<S: StorageLayout> EngineState<S> {
    /// The one-identity convenience constructor: the held identity's hash is also
    /// this node's transport id, so a `new(key)` node can relay. The deliberate
    /// alternative is `default()` plus explicit verbs — `set_transport_id` for an
    /// id-only relay (forwarding never signs), `set_transport_identity` to tie the
    /// role to a held identity, or neither for a leaf.
    #[allow(clippy::expect_used)]
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

    pub fn links_via(&self, interface: InterfaceId) -> usize {
        self.links.links_via(interface)
    }

    pub fn transported_links_via(&self, interface: InterfaceId) -> usize {
        self.transported_links.links_via(interface)
    }

    pub(crate) fn mark_interface_dirty(&mut self, interface: InterfaceId) {
        self.dirty_interfaces.mark(interface);
    }

    #[cfg(feature = "tokio-host")]
    pub(crate) fn drain_dirty_interfaces(&mut self, visit: impl FnMut(InterfaceId)) {
        self.dirty_interfaces.drain(visit);
    }

    pub fn scheduled_announce_count(&self) -> usize {
        self.scheduled_announces.scheduled_count()
    }

    pub fn scheduled_announces_wake(&self) -> LaneWake {
        LaneWake::from_deadline(self.scheduled_announces.earliest_due_at())
    }

    pub fn receipt_timeouts_wake(&self) -> LaneWake {
        LaneWake::from_deadline(self.receipts.earliest_timeout_at())
    }

    pub fn path_request_timeout_wake(&self) -> LaneWake {
        let pending = self.pending_path_requests.earliest_timeout_at();
        let discovery = self.discovery_path_requests.earliest_expiry_at();
        let earliest = match (pending, discovery) {
            (Some(pending), Some(discovery)) => Some(if pending.0 <= discovery.0 {
                pending
            } else {
                discovery
            }),
            (some, None) | (None, some) => some,
        };
        LaneWake::from_deadline(earliest)
    }

    pub fn link_deadlines_wake(&self) -> LaneWake {
        let own = self.links.earliest_timeout_at();
        let transported = self.transported_links.earliest_deadline();
        LaneWake::from_deadline(match (own, transported) {
            (Some(own), Some(transported)) => Some(if own.0 <= transported.0 {
                own
            } else {
                transported
            }),
            (deadline, None) | (None, deadline) => deadline,
        })
    }

    pub fn resource_deadlines_wake(&self) -> LaneWake {
        let outgoing = self.outgoing_resources.earliest_timeout_at();
        let incoming = self.incoming_resources.earliest_timeout_at();
        LaneWake::from_deadline(match (outgoing, incoming) {
            (Some(outgoing), Some(incoming)) => Some(if outgoing.0 <= incoming.0 {
                outgoing
            } else {
                incoming
            }),
            (deadline, None) | (None, deadline) => deadline,
        })
    }

    pub fn channel_timeouts_wake(&self) -> LaneWake {
        LaneWake::from_deadline(self.earliest_channel_tx_timeout_at())
    }

    /// The soonest a held announce may drip out: the earliest release deadline among
    /// the interfaces that actually hold one. An interface that latched a burst but
    /// holds nothing arms no wake.
    pub fn held_announce_release_wake(&self) -> LaneWake {
        let earliest = self
            .held_announces
            .interfaces()
            .filter_map(|interface| self.interface_announce_limits.held_release_for(interface))
            .min();
        LaneWake::from_deadline(earliest)
    }

    /// The soonest a channel send anywhere next times out — the watchdog's wake.
    fn earliest_channel_tx_timeout_at(&self) -> Option<InstantMillis> {
        let mut earliest: Option<InstantMillis> = None;
        for index in 0..self.channels.len() {
            for sub in 0..self.channels.outstanding_count(index) {
                let at = self.channels.outstanding_timeout_at(index, sub);
                earliest = Some(earliest.map_or(at, |best| if at.0 < best.0 { at } else { best }));
            }
        }
        earliest
    }

    pub fn route_expiry_wake(&self, view: &[InterfaceConfig]) -> LaneWake {
        let routes = self
            .routing_table
            .soonest_route_expiry_with_tunnels(view, &self.tunnels);
        let earliest = match (routes, self.tunnels.soonest_expiry()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        LaneWake::from_deadline(earliest)
    }

    /// Probe every reactor-scheduled lane fresh into a [`WakeSchedules`]. This is the full
    /// re-derive — the reactor seeds from it once and then advances incrementally, and it
    /// stands as the oracle the running schedules are checked against. Each method's delta
    /// recomputes the same per-lane helpers, so the two can only diverge if a method forgets
    /// a lane it moved (which the oracle catches).
    pub fn wake_schedules(&self, view: &[InterfaceConfig]) -> WakeSchedules {
        WakeSchedules {
            scheduled_announces: self.scheduled_announces_wake(),
            receipt_timeouts: self.receipt_timeouts_wake(),
            path_request_timeout: self.path_request_timeout_wake(),
            expired_routes: self.route_expiry_wake(view),
            link_deadlines: self.link_deadlines_wake(),
            resource_deadlines: self.resource_deadlines_wake(),
            channel_timeouts: self.channel_timeouts_wake(),
            held_announce_release: self.held_announce_release_wake(),
        }
    }

    /// The reactor's next scheduled wake, named by lane. Equivalent to
    /// [`wake_schedules`](Self::wake_schedules) resolved at `now`; announce scheduling is
    /// deliberately absent — it is the application's to schedule, fired immediately through an
    /// `AnnounceNow` command, never a lingering deadline the engine holds.
    pub fn next_scheduled_wake(
        &self,
        now: InstantMillis,
        view: &[InterfaceConfig],
    ) -> ScheduledWake {
        self.wake_schedules(view).soonest(now)
    }
}

/// Keep the earlier of a running earliest-deadline and a candidate, breaking an exact tie
/// in favour of the one already held — the lane checked first, i.e. higher priority.
fn merge_earliest(
    current: Option<(InstantMillis, DueLane)>,
    candidate: InstantMillis,
    lane: DueLane,
) -> Option<(InstantMillis, DueLane)> {
    match current {
        Some((existing, _)) if existing <= candidate => current,
        _ => Some((candidate, lane)),
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::interfaces::InboundPacket;
    use crate::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::storage::TestFixedStorage;

    #[test]
    fn next_scheduled_wake_is_idle_with_no_scheduled_work() {
        let state: EngineState<Cap> = EngineState::<Cap>::default();
        assert_eq!(
            state.next_scheduled_wake(InstantMillis(1_000), &transporting_view()),
            ScheduledWake::Idle,
        );
    }

    #[test]
    fn next_scheduled_wake_names_the_scheduled_announce_lane_future_then_due() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xEE; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        match state.next_scheduled_wake(InstantMillis(0), &transporting_view()) {
            ScheduledWake::At { at, lane } => {
                assert_eq!(lane, DueLane::ScheduledAnnounces);
                assert!(
                    at.0 >= 1_000 && at.0 < 1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                    "due_at {} should sit within the jitter window after arrival",
                    at.0,
                );
            }
            other => panic!("expected At {{ Rebroadcast }}, got {other:?}"),
        }

        assert_eq!(
            state.next_scheduled_wake(InstantMillis(1_000_000), &transporting_view()),
            ScheduledWake::Due(DueLane::ScheduledAnnounces),
        );
    }

    #[test]
    fn next_scheduled_wake_names_the_route_expiry_for_a_leaf_future_then_due() {
        use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;

        let source = InterfaceId::new([0u8; 8]);
        let view = [routable_descriptor(source)];
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &view,
        );
        assert_eq!(state.route_count(), 1);
        assert_eq!(
            state.scheduled_announce_count(),
            0,
            "a leaf owes no rebroadcast, so the expiry is its only deadline",
        );

        let expiry = InstantMillis(1_000 + DEFAULT_ROUTE_EXPIRY_MILLIS);
        assert_eq!(
            state.next_scheduled_wake(InstantMillis(2_000), &view),
            ScheduledWake::At {
                at: expiry,
                lane: DueLane::ExpiredRoutes,
            },
        );
        assert_eq!(
            state.next_scheduled_wake(expiry, &view),
            ScheduledWake::Due(DueLane::ExpiredRoutes),
            "the expiry instant itself is actionable",
        );
    }

    fn schedules(
        rebroadcast: LaneWake,
        send: LaneWake,
        path: LaneWake,
        expired: LaneWake,
    ) -> WakeSchedules {
        WakeSchedules {
            scheduled_announces: rebroadcast,
            receipt_timeouts: send,
            path_request_timeout: path,
            expired_routes: expired,
            link_deadlines: LaneWake::Unchanged,
            resource_deadlines: LaneWake::Unchanged,
            channel_timeouts: LaneWake::Unchanged,
            held_announce_release: LaneWake::Unchanged,
        }
    }

    #[test]
    fn wake_schedules_soonest_is_idle_when_every_lane_is_clear() {
        let clear = schedules(
            LaneWake::Idle,
            LaneWake::Idle,
            LaneWake::Idle,
            LaneWake::Idle,
        );
        assert_eq!(clear.soonest(InstantMillis(1_000)), ScheduledWake::Idle);
    }

    #[test]
    fn wake_schedules_soonest_names_the_earliest_future_deadline() {
        let scheduled = schedules(
            LaneWake::At(InstantMillis(9_000)),
            LaneWake::At(InstantMillis(3_000)),
            LaneWake::At(InstantMillis(7_000)),
            LaneWake::At(InstantMillis(2_000)),
        );
        assert_eq!(
            scheduled.soonest(InstantMillis(1_000)),
            ScheduledWake::At {
                at: InstantMillis(2_000),
                lane: DueLane::ExpiredRoutes,
            },
        );
    }

    #[test]
    fn wake_schedules_soonest_fires_a_deadline_already_passed() {
        let scheduled = schedules(
            LaneWake::At(InstantMillis(9_000)),
            LaneWake::At(InstantMillis(3_000)),
            LaneWake::Idle,
            LaneWake::Idle,
        );
        assert_eq!(
            scheduled.soonest(InstantMillis(5_000)),
            ScheduledWake::Due(DueLane::ReceiptTimeouts),
            "now is past the send-timeout, so it fires before the future rebroadcast",
        );
    }

    #[test]
    fn wake_schedules_soonest_breaks_a_tie_toward_the_higher_priority_lane() {
        let tied = schedules(
            LaneWake::At(InstantMillis(5_000)),
            LaneWake::At(InstantMillis(5_000)),
            LaneWake::Idle,
            LaneWake::At(InstantMillis(5_000)),
        );
        assert_eq!(
            tied.soonest(InstantMillis(1_000)),
            ScheduledWake::At {
                at: InstantMillis(5_000),
                lane: DueLane::ScheduledAnnounces,
            },
        );
    }

    #[test]
    fn wake_schedules_merge_replaces_named_lanes_and_keeps_the_rest() {
        let mut live = schedules(
            LaneWake::At(InstantMillis(9_000)),
            LaneWake::At(InstantMillis(3_000)),
            LaneWake::Idle,
            LaneWake::Idle,
        );
        live.merge(WakeSchedules {
            scheduled_announces: LaneWake::Idle,
            ..WakeSchedules::UNCHANGED
        });
        assert_eq!(
            live.scheduled_announces,
            LaneWake::Idle,
            "the fired lane is cleared"
        );
        assert_eq!(
            live.receipt_timeouts,
            LaneWake::At(InstantMillis(3_000)),
            "an untouched lane keeps its cached deadline",
        );
        assert_eq!(live.path_request_timeout, LaneWake::Idle);
    }

    #[test]
    fn merge_at_most_keeps_a_sooner_cached_deadline_and_lowers_a_later_one() {
        let mut live = schedules(
            LaneWake::Idle,
            LaneWake::Idle,
            LaneWake::Idle,
            LaneWake::At(InstantMillis(3_000)),
        );
        live.merge(WakeSchedules {
            expired_routes: LaneWake::AtMost(InstantMillis(5_000)),
            ..WakeSchedules::UNCHANGED
        });
        assert_eq!(
            live.expired_routes,
            LaneWake::At(InstantMillis(3_000)),
            "a sooner cached deadline stands",
        );

        live.merge(WakeSchedules {
            expired_routes: LaneWake::AtMost(InstantMillis(2_000)),
            ..WakeSchedules::UNCHANGED
        });
        assert_eq!(
            live.expired_routes,
            LaneWake::At(InstantMillis(2_000)),
            "a sooner ceiling pulls the deadline earlier",
        );
    }

    #[test]
    fn merge_at_most_arms_an_idle_lane() {
        let mut live = schedules(
            LaneWake::Idle,
            LaneWake::Idle,
            LaneWake::Idle,
            LaneWake::Idle,
        );
        live.merge(WakeSchedules {
            expired_routes: LaneWake::AtMost(InstantMillis(7_000)),
            ..WakeSchedules::UNCHANGED
        });
        assert_eq!(
            live.expired_routes,
            LaneWake::At(InstantMillis(7_000)),
            "the first route arms the idle lane at its own expiry",
        );
    }

    #[test]
    fn wake_schedules_delta_tracks_a_recompute_across_a_rebroadcast_lifecycle() {
        let mut state = transporting_node();
        let view = &transporting_view();
        let mut schedules = state.wake_schedules(view);

        let mut raw = hx(RAW_ANNOUNCE);
        let delta = state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
            InstantMillis(1_000),
            &mut |bytes| bytes.fill(0),
            &mut |_: &ProofRequest| false,
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(view),
            "an accepted announce arms the rebroadcast lane; the delta tracks the recompute",
        );

        let delta = state.fire_due_scheduled_announces(
            InstantMillis(1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            &transporting_view(),
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(view),
            "firing the rebroadcast clears the lane; the delta still tracks",
        );
    }

    #[test]
    fn wake_schedules_delta_tracks_a_recompute_across_a_path_request_lifecycle() {
        use crate::wire::DestinationHash;

        let mut state = EngineState::<Cap>::default();
        let view: &[InterfaceConfig] = &[];
        let mut schedules = state.wake_schedules(view);
        let issued_at = InstantMillis(1_000);

        let delta = state.ingest_command_into(
            IssuedCommand {
                id: CommandId(1),
                command: EngineCommand::RequestPath(RequestPath {
                    destination: DestinationHash::new([0x44; 16]),
                    id: PathRequestId::new([0x55; 16]),
                }),
            },
            &[],
            issued_at,
            &mut |bytes| bytes.fill(0),
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(view),
            "a fresh path request arms the path-timeout lane",
        );

        let delta = state.settle_timed_out_path_requests(
            InstantMillis(issued_at.0 + PATH_REQUEST_TIMEOUT_MS + 1),
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(view),
            "settling the timeout clears the lane; the delta still tracks",
        );
    }

    #[test]
    fn a_route_learned_on_a_roaming_interface_arms_the_expiry_lane_at_six_hours() {
        use crate::interfaces::{InterfaceConfig, InterfaceMode};
        use crate::routing::announce::defaults::ROAMING_ROUTE_EXPIRY_MILLIS;

        let source = InterfaceId::new([0u8; 8]);
        let roaming_view = [InterfaceConfig {
            mode: InterfaceMode::Roaming,
            ..routable_descriptor(source)
        }];
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &roaming_view,
        );
        assert_eq!(state.route_count(), 1);

        assert_eq!(
            state.next_scheduled_wake(InstantMillis(2_000), &roaming_view),
            ScheduledWake::At {
                at: InstantMillis(1_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
                lane: DueLane::ExpiredRoutes,
            },
            "a roaming-learned route owes its cull six hours out, not a week",
        );
    }

    #[test]
    fn wake_schedules_delta_tracks_a_recompute_across_a_route_expiry_lifecycle() {
        use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;

        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let view = &transporting_view();
        let mut schedules = state.wake_schedules(view);

        let mut raw = hx(RAW_ANNOUNCE);
        let delta = state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xEE; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
            InstantMillis(1_000),
            &mut |bytes| bytes.fill(0),
            &mut |_: &ProofRequest| false,
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(view),
            "a learned route arms the expired-routes lane; the delta tracks the recompute",
        );

        let delta = state.cull_expired_routes(
            InstantMillis(1_000 + DEFAULT_ROUTE_EXPIRY_MILLIS),
            view,
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(view),
            "culling the route clears the lane; the delta still tracks",
        );
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn a_full_table_journals_the_eviction_then_the_new_hearing() {
        use crate::wire::DestinationHash;
        type OneSlot = TestFixedStorage<1, 8, 64, 4, 32, 4, 4, 32, 4, 4, 4, 4, 8, 4>;
        let mut state: EngineState<OneSlot> = EngineState::default();
        let view = &transporting_view();
        let mut schedules = state.wake_schedules(view);

        let mut first = hx(RAW_ANNOUNCE);
        let delta = state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xEE; 8]),
                bytes: &mut first,
            },
            TEST_ENTROPY,
            &transporting_view(),
            InstantMillis(1_000),
            &mut |bytes| bytes.fill(0),
            &mut |_: &ProofRequest| false,
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(state.route_count(), 1);

        let mut journal = std::vec::Vec::new();
        let mut second = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let delta = state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0xEE; 8]),
                bytes: &mut second,
            },
            TEST_ENTROPY,
            &transporting_view(),
            InstantMillis(2_000),
            &mut |bytes| bytes.fill(0),
            &mut |_: &ProofRequest| false,
            &mut |reaction| {
                if let EngineReaction::Journaled(journaled) = reaction {
                    match journaled {
                        Journaled::RouteEvicted { destination } => {
                            journal.push(("evicted", destination));
                        }
                        Journaled::AnnounceHeard { destination, .. } => {
                            journal.push(("heard", destination));
                        }
                        _ => {}
                    }
                }
            },
        );
        schedules.merge(delta);
        use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
        assert_eq!(
            schedules.expired_routes,
            LaneWake::At(InstantMillis(1_000 + DEFAULT_ROUTE_EXPIRY_MILLIS)),
            "the eviction leaves the cached deadline at the victim's old expiry — early, never late",
        );
        assert_eq!(
            state.wake_schedules(view).expired_routes,
            LaneWake::At(InstantMillis(2_000 + DEFAULT_ROUTE_EXPIRY_MILLIS)),
            "the truth sits later: only the newcomer remains",
        );

        let resync = state.cull_expired_routes(
            InstantMillis(1_000 + DEFAULT_ROUTE_EXPIRY_MILLIS),
            view,
            &mut |_| {},
        );
        schedules.merge(resync);
        assert_eq!(
            schedules,
            state.wake_schedules(view),
            "the premature wake culls nothing and its full recompute resyncs the lane exactly",
        );
        assert_eq!(
            state.route_count(),
            1,
            "the newcomer survived the no-op cull"
        );

        assert_eq!(
            journal,
            std::vec![
                (
                    "evicted",
                    DestinationHash::new(
                        hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap()
                    ),
                ),
                (
                    "heard",
                    DestinationHash::new(
                        hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap()
                    ),
                ),
            ],
            "the victim's eviction is journaled before the newcomer's hearing",
        );
    }

    #[test]
    fn a_capable_host_can_widen_the_routing_table_at_the_type_level() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = EngineState::<
            TestFixedStorage<64, 128, 4096, 4, 512, 8, 8, 128, 8, 8, 8, 8, 16, 16>,
        >::default();
        state.set_transport_id(TEST_TRANSPORT_ID);
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(1));
        assert_eq!(state.route_count(), 1);
    }
}
