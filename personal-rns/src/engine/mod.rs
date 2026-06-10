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

pub use commands::{
    AnnounceAppData, AnnounceNow, AnnounceNowError, AnnounceNowFailure, AnnounceTarget, CommandId,
    CommandOutcome, Delivered, EngineCommand, IssuedCommand, PathFound, PathRequestId, RequestPath,
    RequestPathFailure, SendSingle, SendSingleError, SendSingleFailure, SendSinglePayload,
    Settleable, Settlement, MAX_SEND_SINGLE_PLAINTEXT_LEN, PATH_REQUEST_ID_LEN,
};
pub use egress::{
    write_path_request_wire_packet, EgressDirective, EgressSerializeError,
    PATH_REQUEST_DESTINATION, PATH_REQUEST_PAYLOAD_LEN,
};
pub use identity_registration::SetTransportIdentityError;
pub use reaction::{Directive, EngineReaction, Journaled};
pub use tick::TickOutput;

pub use crate::crypto::ratchets::{RatchetEntropy, RatchetPolicy, RatchetRotation};
pub use crate::routing::announce::emit::{
    AnnounceAppDataBytes, AnnounceRejection, AnnounceWriteFailure, CommandedAnnounceWriteOutcome,
    PathResponseWriteOutcome, WriteAnnounceError,
};
pub use crate::routing::announce::ingress::{
    AcceptedAnnounce, AnnounceIngest, DataPacket, IngestPacketOutcome, Ingress, PacketToForward,
    RebroadcastDecision,
};
pub use crate::routing::delivery::send_single::{
    SendSingleDispatch, SendSingleEntropy, SendSingleRejection, SendSingleWriteOutcome,
    WriteSendSingleError,
};
pub use crate::routing::path_requests::pending::{
    CulledPathRequest, ExpiredPathRequest, SettledPathRequest, PATH_REQUEST_TIMEOUT_MS,
};
pub use crate::routing::path_requests::request_path::{
    CachedPathResponseOutcome, PathRequestWriteOutcome,
};
pub use crate::routing::path_requests::seen::PathRequestIdBytes;
pub use crate::routing::proof::{ProofIngest, ProofOwed, WriteProofError};

use crate::crypto::ratchets::SelfRatchets;
use crate::identity::held::HeldIdentities;
use crate::identity::IDENTITY_SECRET_KEY_LEN;
use crate::interfaces::InterfaceId;
use crate::routing::announce::held_cache::HeldAnnounces;
use crate::routing::announce::rate_limit::AnnounceRates;
use crate::routing::announce::schedule::RebroadcastQueue;
use crate::routing::delivery::receipts::Receipts;
use crate::routing::path_requests::pending::PendingPathRequests;
use crate::routing::path_requests::seen::SeenPathRequests;
use crate::routing::reverse_routes::ReverseRoutes;
use crate::routing::storage::EngineStorage;
use crate::routing::upstream_app_destinations::UpstreamAppDestinations;
use crate::routing::RoutingTable;
use crate::wire::TransportId;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueLane {
    HeldAnnounces,
    RebroadcastAnnounces,
    SendSingleTimeout,
    PathRequestTimeout,
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
    Due,
    At(InstantMillis),
}

impl LaneWake {
    fn from_deadline(earliest: Option<InstantMillis>) -> Self {
        earliest.map_or(LaneWake::Idle, LaneWake::At)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeSchedules {
    pub held_announces: LaneWake,
    pub rebroadcast_announces: LaneWake,
    pub send_single_timeout: LaneWake,
    pub path_request_timeout: LaneWake,
}

impl WakeSchedules {
    pub const UNCHANGED: Self = Self {
        held_announces: LaneWake::Unchanged,
        rebroadcast_announces: LaneWake::Unchanged,
        send_single_timeout: LaneWake::Unchanged,
        path_request_timeout: LaneWake::Unchanged,
    };

    pub fn merge(&mut self, delta: WakeSchedules) {
        for (slot, change) in [
            (&mut self.held_announces, delta.held_announces),
            (&mut self.rebroadcast_announces, delta.rebroadcast_announces),
            (&mut self.send_single_timeout, delta.send_single_timeout),
            (&mut self.path_request_timeout, delta.path_request_timeout),
        ] {
            if change != LaneWake::Unchanged {
                *slot = change;
            }
        }
    }

    pub fn soonest(&self, now: InstantMillis) -> ScheduledWake {
        let mut earliest: Option<(InstantMillis, DueLane)> = None;
        for (wake, lane) in [
            //An implicit priority is here. Items higher in this list will trigger their 'due' before the later items
            //If we need to manage this, the WakeSchedules might need some more light bookkeeping to better distribute that. Marked here for later REVIEW
            (self.held_announces, DueLane::HeldAnnounces),
            (self.rebroadcast_announces, DueLane::RebroadcastAnnounces),
            (self.send_single_timeout, DueLane::SendSingleTimeout),
            (self.path_request_timeout, DueLane::PathRequestTimeout),
        ] {
            match wake {
                LaneWake::Unchanged | LaneWake::Idle => {}
                LaneWake::Due => return ScheduledWake::Due(lane),
                LaneWake::At(at) => {
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

pub struct EngineState<S: EngineStorage> {
    pub(crate) tick_count: u64,
    pub(crate) ingested_packet_count: u64,
    pub(crate) ingested_command_count: u64,
    pub(crate) routing_table: RoutingTable<S::Routes, S::Announces, S::History, S::AppData>,
    pub(crate) held_announces_cache: S::Held,
    pub(crate) pending_rebroadcasts: S::Pending,
    pub(crate) upstream_app_destinations: UpstreamAppDestinations<S::UpstreamAppDestinations>,
    pub(crate) packet_hash_history: S::PacketHashes,
    pub(crate) held_identities: HeldIdentities<S::HeldIdentities>,
    pub(crate) transport_id: Option<TransportId>,
    pub(crate) self_ratchets: SelfRatchets<S::SelfRatchets>,
    pub(crate) receipts: Receipts<S::Receipts>,
    pub(crate) reverse_routes: ReverseRoutes<S::ReverseRoutes>,
    pub(crate) pending_path_requests: PendingPathRequests<S::PendingPathRequests>,
    pub(crate) seen_path_requests: SeenPathRequests<S::SeenPathRequests>,
    pub(crate) announce_rates: AnnounceRates<S::AnnounceRates>,
}

impl<S: EngineStorage> Default for EngineState<S> {
    fn default() -> Self {
        Self {
            tick_count: 0,
            ingested_packet_count: 0,
            ingested_command_count: 0,
            routing_table: Default::default(),
            held_announces_cache: Default::default(),
            pending_rebroadcasts: Default::default(),
            upstream_app_destinations: UpstreamAppDestinations::default(),
            packet_hash_history: Default::default(),
            held_identities: HeldIdentities::default(),
            transport_id: None,
            self_ratchets: SelfRatchets::default(),
            receipts: Receipts::default(),
            reverse_routes: ReverseRoutes::default(),
            pending_path_requests: PendingPathRequests::default(),
            seen_path_requests: SeenPathRequests::default(),
            announce_rates: AnnounceRates::default(),
        }
    }
}

impl<S: EngineStorage> core::fmt::Debug for EngineState<S>
where
    S::Routes: core::fmt::Debug,
    S::Announces: core::fmt::Debug,
    S::History: core::fmt::Debug,
    S::AppData: core::fmt::Debug,
    S::Held: core::fmt::Debug,
    S::Pending: core::fmt::Debug,
    S::UpstreamAppDestinations: core::fmt::Debug,
    S::PacketHashes: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EngineState")
            .field("tick_count", &self.tick_count)
            .field("ingested_packet_count", &self.ingested_packet_count)
            .field("ingested_command_count", &self.ingested_command_count)
            .field("routing_table", &self.routing_table)
            .field("held_announces_cache", &self.held_announces_cache)
            .field("pending_rebroadcasts", &self.pending_rebroadcasts)
            .field("upstream_app_destinations", &self.upstream_app_destinations)
            .field("packet_hash_history", &self.packet_hash_history)
            .field("held_identities", &self.held_identities)
            .field("transport_id", &self.transport_id)
            .field("self_ratchets", &self.self_ratchets)
            .finish()
    }
}

impl<S: EngineStorage> EngineState<S> {
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

    pub const fn tick_count(&self) -> u64 {
        self.tick_count
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

    pub fn held_announce_count(&self) -> usize {
        self.held_announces_cache.len()
    }

    pub fn pending_announce_rebroadcast_count(&self) -> usize {
        self.pending_rebroadcasts.pending_count()
    }

    /// The held lane's state: `Due` while any announce waits on the arena, else `Idle`.
    pub(crate) fn held_lane(&self) -> LaneWake {
        if self.held_announce_count() > 0 {
            LaneWake::Due
        } else {
            LaneWake::Idle
        }
    }

    /// The rebroadcast lane's state from the soonest scheduled re-emit.
    pub(crate) fn rebroadcast_lane(&self) -> LaneWake {
        LaneWake::from_deadline(self.pending_rebroadcasts.earliest_due_at())
    }

    /// The send-single-timeout lane's state from the soonest receipt deadline.
    pub(crate) fn send_timeout_lane(&self) -> LaneWake {
        LaneWake::from_deadline(self.receipts.earliest_timeout_at())
    }

    /// The path-request-timeout lane's state from the soonest pending-request deadline.
    pub(crate) fn path_timeout_lane(&self) -> LaneWake {
        LaneWake::from_deadline(self.pending_path_requests.earliest_timeout_at())
    }

    /// Probe every reactor-scheduled lane fresh into a [`WakeSchedules`]. This is the full
    /// re-derive — the reactor seeds from it once and then advances incrementally, and it
    /// stands as the oracle the running schedules are checked against. Each method's delta
    /// recomputes the same per-lane helpers, so the two can only diverge if a method forgets
    /// a lane it moved (which the oracle catches).
    pub fn wake_schedules(&self) -> WakeSchedules {
        WakeSchedules {
            held_announces: self.held_lane(),
            rebroadcast_announces: self.rebroadcast_lane(),
            send_single_timeout: self.send_timeout_lane(),
            path_request_timeout: self.path_timeout_lane(),
        }
    }

    /// The reactor's next scheduled wake, named by lane. Equivalent to
    /// [`wake_schedules`](Self::wake_schedules) resolved at `now`; announce scheduling is
    /// deliberately absent — it is the application's to schedule, fired immediately through an
    /// `AnnounceNow` command, never a lingering deadline the engine holds.
    pub fn next_scheduled_wake(&self, now: InstantMillis) -> ScheduledWake {
        self.wake_schedules().soonest(now)
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
    use crate::routing::storage::FixedInline;

    #[test]
    fn next_scheduled_wake_is_idle_with_no_scheduled_work() {
        let state: EngineState<Cap> = EngineState::<Cap>::default();
        assert_eq!(
            state.next_scheduled_wake(InstantMillis(1_000)),
            ScheduledWake::Idle,
        );
    }

    #[test]
    fn next_scheduled_wake_names_the_rebroadcast_lane_future_then_due() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        match state.next_scheduled_wake(InstantMillis(0)) {
            ScheduledWake::At { at, lane } => {
                assert_eq!(lane, DueLane::RebroadcastAnnounces);
                assert!(
                    at.0 >= 1_000 && at.0 < 1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                    "due_at {} should sit within the jitter window after arrival",
                    at.0,
                );
            }
            other => panic!("expected At {{ Rebroadcast }}, got {other:?}"),
        }

        assert_eq!(
            state.next_scheduled_wake(InstantMillis(1_000_000)),
            ScheduledWake::Due(DueLane::RebroadcastAnnounces),
        );
    }

    fn schedules(
        held: LaneWake,
        rebroadcast: LaneWake,
        send: LaneWake,
        path: LaneWake,
    ) -> WakeSchedules {
        WakeSchedules {
            held_announces: held,
            rebroadcast_announces: rebroadcast,
            send_single_timeout: send,
            path_request_timeout: path,
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
    fn wake_schedules_soonest_lets_held_preempt_every_deadline() {
        let pressed = schedules(
            LaneWake::Due,
            LaneWake::At(InstantMillis(10)),
            LaneWake::At(InstantMillis(5)),
            LaneWake::Idle,
        );
        assert_eq!(
            pressed.soonest(InstantMillis(0)),
            ScheduledWake::Due(DueLane::HeldAnnounces),
        );
    }

    #[test]
    fn wake_schedules_soonest_names_the_earliest_future_deadline() {
        let scheduled = schedules(
            LaneWake::Idle,
            LaneWake::At(InstantMillis(9_000)),
            LaneWake::At(InstantMillis(3_000)),
            LaneWake::At(InstantMillis(7_000)),
        );
        assert_eq!(
            scheduled.soonest(InstantMillis(1_000)),
            ScheduledWake::At {
                at: InstantMillis(3_000),
                lane: DueLane::SendSingleTimeout,
            },
        );
    }

    #[test]
    fn wake_schedules_soonest_fires_a_deadline_already_passed() {
        let scheduled = schedules(
            LaneWake::Idle,
            LaneWake::At(InstantMillis(9_000)),
            LaneWake::At(InstantMillis(3_000)),
            LaneWake::Idle,
        );
        assert_eq!(
            scheduled.soonest(InstantMillis(5_000)),
            ScheduledWake::Due(DueLane::SendSingleTimeout),
            "now is past the send-timeout, so it fires before the future rebroadcast",
        );
    }

    #[test]
    fn wake_schedules_soonest_breaks_a_tie_toward_the_higher_priority_lane() {
        let tied = schedules(
            LaneWake::Idle,
            LaneWake::At(InstantMillis(5_000)),
            LaneWake::At(InstantMillis(5_000)),
            LaneWake::Idle,
        );
        assert_eq!(
            tied.soonest(InstantMillis(1_000)),
            ScheduledWake::At {
                at: InstantMillis(5_000),
                lane: DueLane::RebroadcastAnnounces,
            },
        );
    }

    #[test]
    fn wake_schedules_merge_replaces_named_lanes_and_keeps_the_rest() {
        let mut live = schedules(
            LaneWake::Idle,
            LaneWake::At(InstantMillis(9_000)),
            LaneWake::At(InstantMillis(3_000)),
            LaneWake::Idle,
        );
        live.merge(WakeSchedules {
            rebroadcast_announces: LaneWake::Idle,
            ..WakeSchedules::UNCHANGED
        });
        assert_eq!(
            live.rebroadcast_announces,
            LaneWake::Idle,
            "the fired lane is cleared"
        );
        assert_eq!(
            live.send_single_timeout,
            LaneWake::At(InstantMillis(3_000)),
            "an untouched lane keeps its cached deadline",
        );
        assert_eq!(live.held_announces, LaneWake::Idle);
        assert_eq!(live.path_request_timeout, LaneWake::Idle);
    }

    #[test]
    fn wake_schedules_delta_tracks_a_recompute_across_a_rebroadcast_lifecycle() {
        let mut state = transporting_node();
        let mut schedules = state.wake_schedules();

        let mut raw = hx(RAW_ANNOUNCE);
        let delta = state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
            InstantMillis(1_000),
            &mut |bytes| bytes.fill(0),
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(),
            "an accepted announce arms the rebroadcast lane; the delta tracks the recompute",
        );

        let delta = state.fire_due_announce_rebroadcasts(
            InstantMillis(1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            &transporting_view(),
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(),
            "firing the rebroadcast clears the lane; the delta still tracks",
        );
    }

    #[test]
    fn wake_schedules_delta_tracks_a_recompute_across_a_path_request_lifecycle() {
        use crate::wire::DestinationHash;

        let mut state = EngineState::<Cap>::default();
        let mut schedules = state.wake_schedules();
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
            state.wake_schedules(),
            "a fresh path request arms the path-timeout lane",
        );

        let delta = state.settle_timed_out_path_requests(
            InstantMillis(issued_at.0 + PATH_REQUEST_TIMEOUT_MS + 1),
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(),
            "settling the timeout clears the lane; the delta still tracks",
        );
    }

    #[test]
    fn a_capable_host_can_widen_the_routing_table_at_the_type_level() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = EngineState::<
            FixedInline<64, 128, 4096, 4, 512, 64, 8, 8, 128, 8, 8, 8, 8, 16>,
        >::default();
        state.set_transport_id(TEST_TRANSPORT_ID);
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(1));
        assert_eq!(state.route_count(), 1);
    }
}
