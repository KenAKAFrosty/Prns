use crate::engine::state::EngineState;
use crate::interfaces::AttachedInterfaces;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::links::channel::table::ChannelTable;
use crate::routing::warmth::WarmestOf;
use crate::storage::StorageLayout;
use crate::units::InstantMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    ScheduledAnnounces,
    ReceiptTimeouts,
    PathRequestTimeouts,
    ExpiredRoutes,
    ExpiredDestinationIdentities,
    ExpiredBlackholes,
    LinkDeadlines,
    ResourceDeadlines,
    ChannelTimeouts,
    HeldAnnounceRelease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextWake {
    Idle,
    Due(WakeReason),
    At {
        at: InstantMillis,
        reason: WakeReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeSchedule {
    Unchanged,
    Idle,
    At(InstantMillis),
    /// Early, never late. If woken early, that wake's full recompute will resync the schedule exactly.
    AtMost(InstantMillis),
}

impl WakeSchedule {
    fn from_deadline(earliest: Option<InstantMillis>) -> Self {
        earliest.map_or(WakeSchedule::Idle, WakeSchedule::At)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeSchedules {
    pub scheduled_announces: WakeSchedule,
    pub receipt_timeouts: WakeSchedule,
    pub path_request_timeouts: WakeSchedule,
    pub expired_routes: WakeSchedule,
    pub expired_destination_identities: WakeSchedule,
    pub expired_blackholes: WakeSchedule,
    pub link_deadlines: WakeSchedule,
    pub resource_deadlines: WakeSchedule,
    pub channel_timeouts: WakeSchedule,
    pub held_announce_release: WakeSchedule,
}

impl WakeSchedules {
    pub const UNCHANGED: Self = Self {
        scheduled_announces: WakeSchedule::Unchanged,
        receipt_timeouts: WakeSchedule::Unchanged,
        path_request_timeouts: WakeSchedule::Unchanged,
        expired_routes: WakeSchedule::Unchanged,
        expired_destination_identities: WakeSchedule::Unchanged,
        expired_blackholes: WakeSchedule::Unchanged,
        link_deadlines: WakeSchedule::Unchanged,
        resource_deadlines: WakeSchedule::Unchanged,
        channel_timeouts: WakeSchedule::Unchanged,
        held_announce_release: WakeSchedule::Unchanged,
    };

    pub fn merge(&mut self, delta: WakeSchedules) {
        for (slot, change) in [
            (&mut self.scheduled_announces, delta.scheduled_announces),
            (&mut self.receipt_timeouts, delta.receipt_timeouts),
            (&mut self.path_request_timeouts, delta.path_request_timeouts),
            (&mut self.expired_routes, delta.expired_routes),
            (
                &mut self.expired_destination_identities,
                delta.expired_destination_identities,
            ),
            (&mut self.expired_blackholes, delta.expired_blackholes),
            (&mut self.link_deadlines, delta.link_deadlines),
            (&mut self.resource_deadlines, delta.resource_deadlines),
            (&mut self.channel_timeouts, delta.channel_timeouts),
            (&mut self.held_announce_release, delta.held_announce_release),
        ] {
            match change {
                WakeSchedule::Unchanged => {}
                WakeSchedule::AtMost(ceiling) => {
                    *slot = match *slot {
                        WakeSchedule::At(cached) if cached <= ceiling => WakeSchedule::At(cached),
                        _ => WakeSchedule::At(ceiling),
                    };
                }
                replacement => *slot = replacement,
            }
        }
    }

    pub fn soonest(&self, now: InstantMillis) -> NextWake {
        let mut earliest: Option<(InstantMillis, WakeReason)> = None;
        for (wake, reason) in [
            // List order is the tie-break when several reasons are due at `now`.
            (self.scheduled_announces, WakeReason::ScheduledAnnounces),
            (self.receipt_timeouts, WakeReason::ReceiptTimeouts),
            (self.path_request_timeouts, WakeReason::PathRequestTimeouts),
            (self.expired_routes, WakeReason::ExpiredRoutes),
            (
                self.expired_destination_identities,
                WakeReason::ExpiredDestinationIdentities,
            ),
            (self.expired_blackholes, WakeReason::ExpiredBlackholes),
            (self.link_deadlines, WakeReason::LinkDeadlines),
            (self.resource_deadlines, WakeReason::ResourceDeadlines),
            (self.channel_timeouts, WakeReason::ChannelTimeouts),
            (self.held_announce_release, WakeReason::HeldAnnounceRelease),
        ] {
            match wake {
                WakeSchedule::Unchanged | WakeSchedule::Idle => {}
                WakeSchedule::At(at) | WakeSchedule::AtMost(at) => {
                    if at <= now {
                        return NextWake::Due(reason);
                    }
                    earliest = merge_earliest(earliest, at, reason);
                }
            }
        }
        match earliest {
            Some((at, reason)) => NextWake::At { at, reason },
            None => NextWake::Idle,
        }
    }
}

impl<S: StorageLayout> EngineState<S> {
    pub fn scheduled_announces_wake(&self) -> WakeSchedule {
        WakeSchedule::from_deadline(self.scheduled_announces.earliest_due_at())
    }

    pub fn receipt_timeouts_wake(&self) -> WakeSchedule {
        WakeSchedule::from_deadline(self.receipts.earliest_timeout_at())
    }

    pub fn path_request_timeouts_wake(&self) -> WakeSchedule {
        let pending = self.pending_path_requests.earliest_timeout_at();
        let discovery = self.recursive_path_requests.earliest_expiry_at();
        let earliest = match (pending, discovery) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        WakeSchedule::from_deadline(earliest)
    }

    pub fn link_deadlines_wake(&self) -> WakeSchedule {
        let own = self.links.earliest_timeout_at();
        let transported = self.transported_links.earliest_deadline();
        WakeSchedule::from_deadline(match (own, transported) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        })
    }

    pub fn resource_deadlines_wake(&self) -> WakeSchedule {
        let outgoing = self.outgoing_resources.earliest_timeout_at();
        let incoming = self.incoming_resources.earliest_timeout_at();
        let active = match (outgoing, incoming) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let pending = self.pending_resource_deadline();
        WakeSchedule::from_deadline(match (active, pending) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        })
    }

    pub fn channel_timeouts_wake(&self) -> WakeSchedule {
        WakeSchedule::from_deadline(self.channels.earliest_tx_timeout_at())
    }

    pub fn held_announce_release_wake(&self) -> WakeSchedule {
        let earliest = self
            .held_announces
            .interfaces()
            .filter_map(|interface| {
                self.interface_announce_limits
                    .next_held_release_at(interface)
            })
            .min();
        WakeSchedule::from_deadline(earliest)
    }

    pub fn route_expiry_wake(&self, interfaces: AttachedInterfaces<'_>) -> WakeSchedule {
        let warmth = WarmestOf(&self.tunnels, &self.departed_interfaces);
        let routes = self
            .routing_table
            .soonest_route_expiry_indexed_with_warmth(interfaces, &warmth);
        let earliest = match (routes, self.tunnels.soonest_expiry()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        WakeSchedule::from_deadline(earliest)
    }

    pub fn blackhole_expiry_wake(&self) -> WakeSchedule {
        WakeSchedule::from_deadline(self.identity_blackholes.earliest_expiry_at())
    }

    pub fn destination_identity_expiry_wake(&self) -> WakeSchedule {
        let routing_table = &self.routing_table;
        WakeSchedule::from_deadline(
            self.destination_identities
                .soonest_expiry(|destination| routing_table.has_route(destination)),
        )
    }

    pub(crate) fn route_removal_wake_schedules(
        &self,
        interfaces: AttachedInterfaces<'_>,
    ) -> WakeSchedules {
        WakeSchedules {
            scheduled_announces: self.scheduled_announces_wake(),
            expired_routes: self.route_expiry_wake(interfaces),
            expired_destination_identities: self.destination_identity_expiry_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    /// Recomputes every schedule from live engine state.
    /// The manifold never calls this on the hot path; each engine mutation returns a `WakeSchedules` delta that the manifold merges into a cached copy instead.
    /// This full re-derive is the ground truth for those deltas: debug builds assert the merged cache matches it after every merge, so a mutation that moves a deadline without reporting it in its delta surfaces as a loud divergence instead of a silently missed wake.
    pub fn wake_schedules(&self, interfaces: AttachedInterfaces<'_>) -> WakeSchedules {
        WakeSchedules {
            scheduled_announces: self.scheduled_announces_wake(),
            receipt_timeouts: self.receipt_timeouts_wake(),
            path_request_timeouts: self.path_request_timeouts_wake(),
            expired_routes: self.route_expiry_wake(interfaces),
            expired_destination_identities: self.destination_identity_expiry_wake(),
            expired_blackholes: self.blackhole_expiry_wake(),
            link_deadlines: self.link_deadlines_wake(),
            resource_deadlines: self.resource_deadlines_wake(),
            channel_timeouts: self.channel_timeouts_wake(),
            held_announce_release: self.held_announce_release_wake(),
        }
    }

    pub fn next_wake(&self, now: InstantMillis, interfaces: AttachedInterfaces<'_>) -> NextWake {
        self.wake_schedules(interfaces).soonest(now)
    }
}

/// Breaks ties in favor of the one already held
fn merge_earliest(
    current: Option<(InstantMillis, WakeReason)>,
    candidate: InstantMillis,
    reason: WakeReason,
) -> Option<(InstantMillis, WakeReason)> {
    match current {
        Some((existing, _)) if existing <= candidate => current,
        _ => Some((candidate, reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{
        CommandId, EngineReaction, IngestIo, IssuedCommand, Journaled, PathRequestId, PrnsCommand,
        ProofRequest, RequestPath, PATH_REQUEST_TIMEOUT_MS,
    };
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::storage::TestFixedStorage;

    #[test]
    fn next_wake_is_idle_with_no_scheduled_work() {
        let state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        assert_eq!(
            state.next_wake(
                InstantMillis(1_000),
                AttachedInterfaces::new(&transporting_interfaces())
            ),
            NextWake::Idle,
        );
    }

    #[test]
    fn next_wake_names_the_scheduled_announce_reason_future_then_due() {
        let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let mut state = transporting_node();
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xEE; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        match state.next_wake(
            InstantMillis(0),
            AttachedInterfaces::new(&transporting_interfaces()),
        ) {
            NextWake::At { at, reason } => {
                assert_eq!(reason, WakeReason::ScheduledAnnounces);
                assert!(
                    at.0 >= 1_000 && at.0 < 1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                    "due_at {} should sit within the jitter window after arrival",
                    at.0,
                );
            }
            other => panic!("expected At {{ ScheduledAnnounces }}, got {other:?}"),
        }

        assert_eq!(
            state.next_wake(
                InstantMillis(1_000_000),
                AttachedInterfaces::new(&transporting_interfaces())
            ),
            NextWake::Due(WakeReason::ScheduledAnnounces),
        );
    }

    #[test]
    fn next_wake_names_the_route_expiry_for_a_leaf_future_then_due() {
        use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;

        let source = InterfaceId::new([0u8; 8]);
        let interfaces = [routable_descriptor(source)];
        let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        assert_eq!(state.route_count(), 1);
        assert_eq!(
            state.scheduled_announce_count(),
            0,
            "a leaf owes no rebroadcast, so the expiry is its only deadline",
        );

        let expiry = InstantMillis(1_000 + DEFAULT_ROUTE_EXPIRY_MILLIS);
        assert_eq!(
            state.next_wake(InstantMillis(2_000), AttachedInterfaces::new(&interfaces)),
            NextWake::At {
                at: expiry,
                reason: WakeReason::ExpiredRoutes,
            },
        );
        assert_eq!(
            state.next_wake(expiry, AttachedInterfaces::new(&interfaces)),
            NextWake::Due(WakeReason::ExpiredRoutes),
            "the expiry instant itself is actionable",
        );
    }

    fn schedules(
        scheduled_announces: WakeSchedule,
        receipt_timeouts: WakeSchedule,
        path_request_timeouts: WakeSchedule,
        expired_routes: WakeSchedule,
    ) -> WakeSchedules {
        WakeSchedules {
            scheduled_announces,
            receipt_timeouts,
            path_request_timeouts,
            expired_routes,
            expired_destination_identities: WakeSchedule::Unchanged,
            expired_blackholes: WakeSchedule::Unchanged,
            link_deadlines: WakeSchedule::Unchanged,
            resource_deadlines: WakeSchedule::Unchanged,
            channel_timeouts: WakeSchedule::Unchanged,
            held_announce_release: WakeSchedule::Unchanged,
        }
    }

    #[test]
    fn wake_schedules_soonest_is_idle_when_every_schedule_is_clear() {
        let clear = schedules(
            WakeSchedule::Idle,
            WakeSchedule::Idle,
            WakeSchedule::Idle,
            WakeSchedule::Idle,
        );
        assert_eq!(clear.soonest(InstantMillis(1_000)), NextWake::Idle);
    }

    #[test]
    fn wake_schedules_soonest_names_the_earliest_future_deadline() {
        let scheduled = schedules(
            WakeSchedule::At(InstantMillis(9_000)),
            WakeSchedule::At(InstantMillis(3_000)),
            WakeSchedule::At(InstantMillis(7_000)),
            WakeSchedule::At(InstantMillis(2_000)),
        );
        assert_eq!(
            scheduled.soonest(InstantMillis(1_000)),
            NextWake::At {
                at: InstantMillis(2_000),
                reason: WakeReason::ExpiredRoutes,
            },
        );
    }

    #[test]
    fn wake_schedules_soonest_fires_a_deadline_already_passed() {
        let scheduled = schedules(
            WakeSchedule::At(InstantMillis(9_000)),
            WakeSchedule::At(InstantMillis(3_000)),
            WakeSchedule::Idle,
            WakeSchedule::Idle,
        );
        assert_eq!(
            scheduled.soonest(InstantMillis(5_000)),
            NextWake::Due(WakeReason::ReceiptTimeouts),
            "now is past the receipt timeout, so it fires before the future announce",
        );
    }

    #[test]
    fn wake_schedules_soonest_breaks_a_tie_toward_the_higher_priority_reason() {
        let tied = schedules(
            WakeSchedule::At(InstantMillis(5_000)),
            WakeSchedule::At(InstantMillis(5_000)),
            WakeSchedule::Idle,
            WakeSchedule::At(InstantMillis(5_000)),
        );
        assert_eq!(
            tied.soonest(InstantMillis(1_000)),
            NextWake::At {
                at: InstantMillis(5_000),
                reason: WakeReason::ScheduledAnnounces,
            },
        );
    }

    #[test]
    fn wake_schedules_merge_replaces_named_schedules_and_keeps_the_rest() {
        let mut live = schedules(
            WakeSchedule::At(InstantMillis(9_000)),
            WakeSchedule::At(InstantMillis(3_000)),
            WakeSchedule::Idle,
            WakeSchedule::Idle,
        );
        live.merge(WakeSchedules {
            scheduled_announces: WakeSchedule::Idle,
            ..WakeSchedules::UNCHANGED
        });
        assert_eq!(
            live.scheduled_announces,
            WakeSchedule::Idle,
            "the fired schedule is cleared"
        );
        assert_eq!(
            live.receipt_timeouts,
            WakeSchedule::At(InstantMillis(3_000)),
            "an untouched schedule keeps its cached deadline",
        );
        assert_eq!(live.path_request_timeouts, WakeSchedule::Idle);
    }

    #[test]
    fn merge_at_most_keeps_a_sooner_cached_deadline_and_lowers_a_later_one() {
        let mut live = schedules(
            WakeSchedule::Idle,
            WakeSchedule::Idle,
            WakeSchedule::Idle,
            WakeSchedule::At(InstantMillis(3_000)),
        );
        live.merge(WakeSchedules {
            expired_routes: WakeSchedule::AtMost(InstantMillis(5_000)),
            ..WakeSchedules::UNCHANGED
        });
        assert_eq!(
            live.expired_routes,
            WakeSchedule::At(InstantMillis(3_000)),
            "a sooner cached deadline stands",
        );

        live.merge(WakeSchedules {
            expired_routes: WakeSchedule::AtMost(InstantMillis(2_000)),
            ..WakeSchedules::UNCHANGED
        });
        assert_eq!(
            live.expired_routes,
            WakeSchedule::At(InstantMillis(2_000)),
            "a sooner ceiling pulls the deadline earlier",
        );
    }

    #[test]
    fn merge_at_most_arms_an_idle_schedule() {
        let mut live = schedules(
            WakeSchedule::Idle,
            WakeSchedule::Idle,
            WakeSchedule::Idle,
            WakeSchedule::Idle,
        );
        live.merge(WakeSchedules {
            expired_routes: WakeSchedule::AtMost(InstantMillis(7_000)),
            ..WakeSchedules::UNCHANGED
        });
        assert_eq!(
            live.expired_routes,
            WakeSchedule::At(InstantMillis(7_000)),
            "the first route arms the idle schedule at its own expiry",
        );
    }

    #[test]
    fn wake_schedules_delta_tracks_a_recompute_across_a_rebroadcast_lifecycle() {
        let mut state = transporting_node();
        let descriptors = transporting_interfaces();
        let interfaces = AttachedInterfaces::new(&descriptors);
        let mut schedules = state.wake_schedules(interfaces);

        let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let delta = state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&transporting_interfaces()),
                now: InstantMillis(1_000),
                fill_entropy: &mut |bytes| bytes.fill(0),
                should_prove: &mut |_: &ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |_| {},
            },
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(interfaces),
            "an accepted announce arms the scheduled-announces schedule; the delta tracks the recompute",
        );

        let delta = state.fire_due_scheduled_announces(
            InstantMillis(1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(interfaces),
            "firing the rebroadcast clears the schedule; the delta still tracks",
        );
    }

    #[test]
    fn wake_schedules_delta_tracks_a_recompute_across_a_path_request_lifecycle() {
        use crate::wire::DestinationHash;

        let mut state = EngineState::<TestStorageLayout>::default();
        let interfaces: AttachedInterfaces<'_> = AttachedInterfaces::new(&[]);
        let mut schedules = state.wake_schedules(interfaces);
        let issued_at = InstantMillis(1_000);

        let delta = state.ingest_command_into(
            IssuedCommand {
                id: CommandId(1),
                command: PrnsCommand::RequestPath(RequestPath {
                    destination: DestinationHash::new([0x44; 16]),
                    id: PathRequestId::new([0x55; 16]),
                }),
            },
            AttachedInterfaces::new(&[]),
            issued_at,
            &mut |bytes| bytes.fill(0),
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(interfaces),
            "a fresh path request arms the path-request-timeouts schedule",
        );

        let delta = state.settle_timed_out_path_requests(
            InstantMillis(issued_at.0 + PATH_REQUEST_TIMEOUT_MS + 1),
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(interfaces),
            "settling the timeout clears the schedule; the delta still tracks",
        );
    }

    #[test]
    fn a_route_learned_on_a_roaming_interface_arms_the_expiry_schedule_at_six_hours() {
        use crate::interfaces::{InterfaceDescriptor, InterfaceMode};
        use crate::routing::announce::defaults::ROAMING_ROUTE_EXPIRY_MILLIS;

        let source = InterfaceId::new([0u8; 8]);
        let roaming_view = [InterfaceDescriptor {
            mode: InterfaceMode::Roaming,
            ..routable_descriptor(source)
        }];
        let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&roaming_view),
            &mut |_| {},
            None,
        );
        assert_eq!(state.route_count(), 1);

        assert_eq!(
            state.next_wake(InstantMillis(2_000), AttachedInterfaces::new(&roaming_view)),
            NextWake::At {
                at: InstantMillis(1_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
                reason: WakeReason::ExpiredRoutes,
            },
            "a roaming-learned route owes its cull six hours out, not a week",
        );
    }

    #[test]
    fn wake_schedules_delta_tracks_a_recompute_across_a_route_expiry_lifecycle() {
        use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;

        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let descriptors = transporting_interfaces();
        let interfaces = AttachedInterfaces::new(&descriptors);
        let mut schedules = state.wake_schedules(interfaces);

        let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let delta = state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xEE; 8]),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&transporting_interfaces()),
                now: InstantMillis(1_000),
                fill_entropy: &mut |bytes| bytes.fill(0),
                should_prove: &mut |_: &ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |_| {},
            },
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(interfaces),
            "a learned route arms the expired-routes schedule; the delta tracks the recompute",
        );

        let delta = state.cull_expired_routes(
            InstantMillis(1_000 + DEFAULT_ROUTE_EXPIRY_MILLIS),
            interfaces,
            &mut |_| {},
        );
        schedules.merge(delta);
        assert_eq!(
            schedules,
            state.wake_schedules(interfaces),
            "culling the route clears the schedule; the delta still tracks",
        );
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn a_full_table_journals_the_eviction_then_the_new_hearing() {
        use crate::wire::DestinationHash;
        type OneSlot = TestFixedStorage<1, 8, 64, 4, 4, 32, 4, 4, 4, 4, 8, 4>;
        let mut state: EngineState<OneSlot> = EngineState::default();
        let descriptors = transporting_interfaces();
        let interfaces = AttachedInterfaces::new(&descriptors);
        let mut schedules = state.wake_schedules(interfaces);

        let mut first = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let delta = state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xEE; 8]),
                bytes: &mut first,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&transporting_interfaces()),
                now: InstantMillis(1_000),
                fill_entropy: &mut |bytes| bytes.fill(0),
                should_prove: &mut |_: &ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |_| {},
            },
        );
        schedules.merge(delta);
        assert_eq!(state.route_count(), 1);

        let mut journal = std::vec::Vec::new();
        let mut second = bytes_from_hex(RNS_1_4_2_RATCHETED_ANNOUNCE);
        let delta = state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0xEE; 8]),
                bytes: &mut second,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&transporting_interfaces()),
                now: InstantMillis(2_000),
                fill_entropy: &mut |bytes| bytes.fill(0),
                should_prove: &mut |_: &ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(journaled) = reaction {
                        match journaled {
                            Journaled::RouteRemoved {
                                destination,
                                cause: crate::engine::RouteRemovalCause::Evicted,
                            } => {
                                journal.push(("evicted", destination));
                            }
                            Journaled::AnnounceHeard { observation, .. } => {
                                journal.push(("heard", observation.destination));
                            }
                            _ => {}
                        }
                    }
                },
            },
        );
        schedules.merge(delta);
        use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
        assert_eq!(
            schedules.expired_routes,
            WakeSchedule::At(InstantMillis(1_000 + DEFAULT_ROUTE_EXPIRY_MILLIS)),
            "the eviction leaves the cached deadline at the victim's old expiry: early, never late",
        );
        assert_eq!(
            state.wake_schedules(interfaces).expired_routes,
            WakeSchedule::At(InstantMillis(2_000 + DEFAULT_ROUTE_EXPIRY_MILLIS)),
            "the truth sits later: only the newcomer remains",
        );

        let resync = state.cull_expired_routes(
            InstantMillis(1_000 + DEFAULT_ROUTE_EXPIRY_MILLIS),
            interfaces,
            &mut |_| {},
        );
        schedules.merge(resync);
        assert_eq!(
            schedules,
            state.wake_schedules(interfaces),
            "the premature wake culls nothing and its full recompute resyncs the schedule exactly",
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
                        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                            .try_into()
                            .unwrap()
                    ),
                ),
                (
                    "heard",
                    DestinationHash::new(
                        bytes_from_hex("c3cfae69b36bb6e3bbfd96a3b5867a59")
                            .try_into()
                            .unwrap()
                    ),
                ),
            ],
            "the victim's eviction is journaled before the newcomer's hearing",
        );
    }
}
