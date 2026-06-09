pub mod announce_rate;
mod command;
pub mod commands;
pub mod egress;
pub mod identity_registration;
mod inbound;
pub mod ingress;
pub mod pending_path_requests;
pub mod proof;
pub mod reaction;
pub mod receipts;
pub mod request_path;
pub mod reverse_routes;
mod scheduled;
pub mod seen_path_requests;
pub mod self_announce;
pub mod self_ratchets;
pub mod send_single;
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
pub use ingress::{
    AcceptedAnnounce, AnnounceIngest, IngestPacketOutcome, PacketToForward, RebroadcastDecision,
};
pub use ingress::{DataPacket, Ingress};
pub use pending_path_requests::{
    CulledPathRequest, ExpiredPathRequest, SettledPathRequest, PATH_REQUEST_TIMEOUT_MS,
};
pub use proof::{ProofIngest, ProofOwed, WriteProofError};
pub use reaction::{Directive, EngineReaction, Journaled};
pub use request_path::{CachedPathResponseOutcome, PathRequestWriteOutcome};
pub use seen_path_requests::PathRequestIdBytes;
pub use self_announce::{
    CommandedAnnounceWriteOutcome, DueSelfAnnounceWriteOutcome, PathResponseWriteOutcome,
    ReannounceSchedule, SelfAnnounceAppData, SelfAnnounceRejection, SelfAnnounceWriteFailure,
    WriteSelfAnnounceError,
};
pub use self_ratchets::{RatchetEntropy, RatchetPolicy, RatchetRotation};
pub use send_single::{
    SendSingleDispatch, SendSingleEntropy, SendSingleRejection, SendSingleWriteOutcome,
    WriteSendSingleError,
};
pub use tick::TickOutput;

use crate::engine::announce_rate::AnnounceRates;
use crate::engine::pending_path_requests::PendingPathRequests;
use crate::engine::receipts::Receipts;
use crate::engine::reverse_routes::ReverseRoutes;
use crate::engine::seen_path_requests::SeenPathRequests;
use crate::engine::self_announce::SelfAnnounces;
use crate::engine::self_ratchets::SelfRatchets;
use crate::identity::held::HeldIdentities;
use crate::identity::IDENTITY_SECRET_KEY_LEN;
use crate::interfaces::InterfaceId;
use crate::routing::announce::held_cache::HeldAnnounces;
use crate::routing::announce::schedule::RebroadcastQueue;
use crate::routing::storage::EngineStorage;
use crate::routing::upstream_app_destinations::UpstreamAppDestinations;
use crate::routing::RoutingTable;
use crate::wire::TransportId;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextScheduledEngineWork {
    Immediate,
    At(InstantMillis),
    Idle,
}

/// Which lane of scheduled work a wake is for. The reactor's timer edge fans on this:
/// every variant names exactly one engine method, so a wake fires the work it woke for
/// and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueLane {
    /// Retry the held announces against the now-unblocked arena
    /// ([`recover_held_announces`](EngineState::recover_held_announces)).
    HeldAnnounces,
    /// Emit this node's due self-announce
    /// ([`fire_due_self_announces`](EngineState::fire_due_self_announces)).
    SelfAnnounce,
    /// Re-emit a learned announce whose jittered rebroadcast is due
    /// ([`fire_due_announce_rebroadcasts`](EngineState::fire_due_announce_rebroadcasts)).
    Rebroadcast,
    /// Give up on a send whose proof never came
    /// ([`settle_timed_out_send_singles`](EngineState::settle_timed_out_send_singles)).
    SendSingleTimeout,
    /// Give up on a path request whose answer never came
    /// ([`settle_timed_out_path_requests`](EngineState::settle_timed_out_path_requests)).
    PathRequestTimeout,
}

/// The next scheduled wake, naming both *when* and *which* lane. `At` carries the lane
/// that owns the earliest deadline so the reactor can sleep, wake, and fire it without
/// re-deriving — the engine is frozen while the reactor parks, so the owner is still the
/// one that comes due. The coarser [`NextScheduledEngineWork`] is this with the lane
/// dropped, kept for the legacy runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledWake {
    Idle,
    Due(DueLane),
    At { at: InstantMillis, lane: DueLane },
}

pub struct EngineState<S: EngineStorage> {
    tick_count: u64,
    ingested_packet_count: u64,
    ingested_command_count: u64,
    routing_table: RoutingTable<S::Routes, S::Announces, S::History, S::AppData>,
    held_announces_cache: S::Held,
    pending_rebroadcasts: S::Pending,
    upstream_app_destinations: UpstreamAppDestinations<S::UpstreamAppDestinations>,
    packet_hash_history: S::PacketHashes,
    held_identities: HeldIdentities<S::HeldIdentities>,
    transport_id: Option<TransportId>,
    self_announces: SelfAnnounces<S::SelfAnnounces>,
    self_ratchets: SelfRatchets<S::SelfRatchets>,
    pub(crate) receipts: Receipts<S::Receipts>,
    reverse_routes: ReverseRoutes<S::ReverseRoutes>,
    pending_path_requests: PendingPathRequests<S::PendingPathRequests>,
    seen_path_requests: SeenPathRequests<S::SeenPathRequests>,
    announce_rates: AnnounceRates<S::AnnounceRates>,
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
            self_announces: SelfAnnounces::default(),
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
    S::SelfAnnounces: core::fmt::Debug,
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
            .field("self_announces", &self.self_announces)
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

    pub fn next_wakeup(&self, now: InstantMillis) -> NextScheduledEngineWork {
        match self.next_scheduled_wake(now) {
            ScheduledWake::Idle => NextScheduledEngineWork::Idle,
            ScheduledWake::Due(_) => NextScheduledEngineWork::Immediate,
            ScheduledWake::At { at, .. } => NextScheduledEngineWork::At(at),
        }
    }

    /// The next scheduled wake, named by lane: the first lane already due (in priority
    /// order — held, self-announce, rebroadcast, send-timeout, path-timeout), else the
    /// lane owning the earliest future deadline, else `Idle`. Ties at the same instant go
    /// to the higher-priority lane. The reactor drives its timer edge off this; the
    /// lane-less [`next_wakeup`](Self::next_wakeup) is the same answer for the legacy
    /// runtime.
    pub fn next_scheduled_wake(&self, now: InstantMillis) -> ScheduledWake {
        if self.held_announce_count() > 0 {
            return ScheduledWake::Due(DueLane::HeldAnnounces);
        }

        let mut earliest: Option<(InstantMillis, DueLane)> = None;

        if self.self_announces.due_announce(now).is_some() {
            return ScheduledWake::Due(DueLane::SelfAnnounce);
        }
        if let Some(deadline) = self.self_announces.next_due_at() {
            earliest = merge_earliest(earliest, deadline, DueLane::SelfAnnounce);
        }

        if let Some(due_at) = self.pending_rebroadcasts.earliest_due_at() {
            if due_at <= now {
                return ScheduledWake::Due(DueLane::Rebroadcast);
            }
            earliest = merge_earliest(earliest, due_at, DueLane::Rebroadcast);
        }

        if let Some(timeout_at) = self.receipts.earliest_timeout_at() {
            if timeout_at <= now {
                return ScheduledWake::Due(DueLane::SendSingleTimeout);
            }
            earliest = merge_earliest(earliest, timeout_at, DueLane::SendSingleTimeout);
        }

        if let Some(timeout_at) = self.pending_path_requests.earliest_timeout_at() {
            if timeout_at <= now {
                return ScheduledWake::Due(DueLane::PathRequestTimeout);
            }
            earliest = merge_earliest(earliest, timeout_at, DueLane::PathRequestTimeout);
        }

        match earliest {
            Some((at, lane)) => ScheduledWake::At { at, lane },
            None => ScheduledWake::Idle,
        }
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
    use crate::wire::MTU;

    #[test]
    fn next_wakeup_is_idle_for_a_relay_with_no_scheduled_work() {
        let state: EngineState<Cap> = EngineState::<Cap>::default();
        assert_eq!(
            state.next_wakeup(InstantMillis(1_000)),
            NextScheduledEngineWork::Idle
        );
    }

    #[test]
    fn next_wakeup_is_immediate_when_a_self_announce_is_due() {
        let state = personal_node_announcer();
        assert_eq!(
            state.next_wakeup(InstantMillis(0)),
            NextScheduledEngineWork::Immediate
        );
    }

    #[test]
    fn next_wakeup_reports_the_reannounce_deadline_once_we_have_announced() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; MTU];
        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();

        let interval = ReannounceSchedule::default().interval_millis();
        assert_eq!(
            state.next_wakeup(InstantMillis(2_000)),
            NextScheduledEngineWork::At(InstantMillis(1_000 + interval)),
        );
    }

    #[test]
    fn next_wakeup_accounts_for_a_scheduled_rebroadcast() {
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

        match state.next_wakeup(InstantMillis(0)) {
            NextScheduledEngineWork::At(t) => assert!(
                t.0 >= 1_000 && t.0 < 1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                "due_at {} should sit within the jitter window after arrival",
                t.0,
            ),
            other => panic!("expected At(_), got {other:?}"),
        }

        assert_eq!(
            state.next_wakeup(InstantMillis(1_000_000)),
            NextScheduledEngineWork::Immediate,
        );
    }

    #[test]
    fn next_scheduled_wake_is_idle_with_no_scheduled_work() {
        let state: EngineState<Cap> = EngineState::<Cap>::default();
        assert_eq!(
            state.next_scheduled_wake(InstantMillis(1_000)),
            ScheduledWake::Idle,
        );
    }

    #[test]
    fn next_scheduled_wake_names_the_self_announce_lane_when_due() {
        let state = personal_node_announcer();
        assert_eq!(
            state.next_scheduled_wake(InstantMillis(0)),
            ScheduledWake::Due(DueLane::SelfAnnounce),
        );
    }

    #[test]
    fn next_scheduled_wake_carries_the_reannounce_deadline_and_its_lane() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; MTU];
        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_SELF_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .written_len();

        let interval = ReannounceSchedule::default().interval_millis();
        assert_eq!(
            state.next_scheduled_wake(InstantMillis(2_000)),
            ScheduledWake::At {
                at: InstantMillis(1_000 + interval),
                lane: DueLane::SelfAnnounce,
            },
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
                assert_eq!(lane, DueLane::Rebroadcast);
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
            ScheduledWake::Due(DueLane::Rebroadcast),
        );
    }

    #[test]
    fn next_scheduled_wake_prefers_the_higher_priority_due_lane() {
        let mut state = personal_node_announcer();
        let mut raw = hx(RAW_ANNOUNCE);
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

        assert_eq!(
            state.next_scheduled_wake(InstantMillis(1_000_000)),
            ScheduledWake::Due(DueLane::SelfAnnounce),
            "self-announce is checked before rebroadcast, so it names a shared wake",
        );
    }

    #[test]
    fn a_capable_host_can_widen_the_routing_table_at_the_type_level() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = EngineState::<
            FixedInline<64, 128, 4096, 4, 512, 64, 8, 8, 8, 128, 8, 8, 8, 8, 16>,
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
