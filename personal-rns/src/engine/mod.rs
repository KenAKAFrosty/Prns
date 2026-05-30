//! Pure protocol engine boundary.
//!
//! The engine has two verbs. `ingest` takes a batch of inbound packets, each
//! frozen with the instant it arrived, and is clock-free. `tick` advances the
//! engine's periodic work to a caller-supplied `now`. Neither reads clocks,
//! sockets, or storage directly.

mod driver;
pub mod egress;
pub mod ingress;

pub use driver::{EngineDriver, StepOutput, TickSummary};
pub use egress::{EgressDirective, EgressSerializeError};
pub use ingress::Ingress;

use crate::interfaces::{ConnectionState, Interface, InterfaceId};
use crate::routing::announce::{Announce, AnnounceAcceptanceDecision, AnnounceAcceptanceInput};
use crate::routing::defaults::jitter_offset_for;
use crate::routing::held_cache::{HeldAnnouncesCache, DEFAULT_HELD_CACHE_CAPACITY};
use crate::routing::schedule::PendingRebroadcasts;
use crate::routing::storage::{
    AnnounceIdHistory, FixedArrayRetainedAnnounceColumns, FixedArrayRouteColumns,
    PackedAppDataArena, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
    TieredAnnounceIdHistory,
};
use crate::routing::{
    DropCause, RoutingTable, UpsertRouteOutcome, DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
    DEFAULT_HISTORY_CAP_PER_DESTINATION, DEFAULT_HISTORY_FLOOR_PER_DESTINATION,
    DEFAULT_HISTORY_OVERFLOW_CAPACITY, DEFAULT_MAX_TRACKED_DESTINATIONS,
    DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
};
use crate::wire::DestinationHash;
use heapless::Vec as HeaplessVec;

/// Cap on how many interfaces the engine can own at once. Picked against
/// embedded reality — a real device typically has 1–4 active radios; 8
/// gives slack for hosts that present a virtual interface (USB, BLE,
/// loopback for diagnostics) without ballooning per-tick fanout arena
/// storage. Tunable if a real host outgrows it; not exposed as a const
/// generic to keep `EngineState`'s type signature manageable.
pub const MAX_REGISTERED_INTERFACES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboundPacket<'a> {
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub bytes: &'a [u8],
}

/// Retained engine state. **Purely abstract** in its type parameters — does
/// not name a preset. The no_std stack-resident preset lives in
/// [`DefaultEngineState`]; that's the canonical embedded entry point. A
/// capable host substitutes alternate routing-storage backends at the type
/// parameters directly.
///
/// The engine owns its **interface registry** — the host calls
/// [`register_routable_interface`] at startup for each concrete interface it
/// presents. From then on the engine computes positive `fire_on` fanout
/// targets per directive (see [`EgressDirective`]) rather than asking the host
/// to apply "don't reflect back to source" by hand.
///
/// [`register_routable_interface`]: EngineState::register_routable_interface
/// [`EgressDirective`]: crate::engine::EgressDirective
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EngineState<R, A, H, D, const MAX_HELD_ANNOUNCES: usize>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    tick_count: u64,
    ingested_packet_count: u64,
    routing_table: RoutingTable<R, A, H, D>,
    held_announces_cache: HeldAnnouncesCache<MAX_HELD_ANNOUNCES>,
    // The pending-rebroadcast set is capped at `MAX_HELD_ANNOUNCES`, reusing the held-cache
    // dial: one slot per destination, and the realistic per-tick burst of
    // unique accepts is the same order of magnitude as `MAX_HELD_ANNOUNCES` (we already park
    // up to `MAX_HELD_ANNOUNCES` arena-pressure overflows per tick). Hosts that genuinely
    // see larger bursts widen `MAX_HELD_ANNOUNCES` and get both columns at once. A separate
    // `MAX_PENDING` dial is the obvious next iteration if that assumption
    // breaks.
    pending_rebroadcasts: PendingRebroadcasts<MAX_HELD_ANNOUNCES>,
    // Interfaces the host has registered with this engine. tick() builds each
    // directive's `fire_on` list from this set minus the source.
    interfaces: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterInterfaceError {
    RegistryFull,
    NotTransmitting,
    NotRoutable { state: ConnectionState },
}

/// The no_std stack-resident engine-state preset — the only place the
/// default backend choices are named. Mirrors
/// [`DefaultRoutingTable`](crate::routing::DefaultRoutingTable).
pub type DefaultEngineState<
    const MAX_TRACKED_DESTINATIONS: usize = DEFAULT_MAX_TRACKED_DESTINATIONS,
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize = DEFAULT_HISTORY_CAP_PER_DESTINATION,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize = DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
    const HISTORY_FLOOR_PER_DESTINATION: usize = DEFAULT_HISTORY_FLOOR_PER_DESTINATION,
    const HISTORY_OVERFLOW_CAPACITY: usize = DEFAULT_HISTORY_OVERFLOW_CAPACITY,
    const HELD_CACHE_CAPACITY: usize = DEFAULT_HELD_CACHE_CAPACITY,
> = EngineState<
    FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>,
    FixedArrayRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS>,
    TieredAnnounceIdHistory<
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
    >,
    PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>,
    HELD_CACHE_CAPACITY,
>;

impl<R, A, H, D, const MAX_HELD_ANNOUNCES: usize> EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    pub const fn tick_count(&self) -> u64 {
        self.tick_count
    }

    pub const fn ingested_packet_count(&self) -> u64 {
        self.ingested_packet_count
    }

    pub fn route_count(&self) -> usize {
        self.routing_table.route_count()
    }

    pub fn held_announce_count(&self) -> usize {
        self.held_announces_cache.len()
    }

    pub fn pending_announce_rebroadcast_count(&self) -> usize {
        self.pending_rebroadcasts.pending_count()
    }

    /// Register a concrete interface for engine fanout after checking the
    /// load-bearing interface contract: it must be connected enough to route and
    /// it must be able to transmit. Idempotent: registering an already-known
    /// interface id is a no-op that returns `Ok(())`.
    pub fn register_routable_interface<I: Interface + ?Sized>(
        &mut self,
        interface: &I,
    ) -> Result<(), RegisterInterfaceError> {
        let connection_state = interface.state();
        match connection_state {
            ConnectionState::Connected | ConnectionState::Degraded => {}
            ConnectionState::Initializing
            | ConnectionState::Reconnecting
            | ConnectionState::Failed
            | ConnectionState::Disconnected => {
                return Err(RegisterInterfaceError::NotRoutable {
                    state: connection_state,
                });
            }
        }

        if !interface.capabilities().transmits {
            return Err(RegisterInterfaceError::NotTransmitting);
        }

        let id = interface.id();
        if self.interfaces.contains(&id) {
            return Ok(());
        }
        self.interfaces
            .push(id)
            .map_err(|_| RegisterInterfaceError::RegistryFull)
    }

    /// Currently-registered interfaces, in registration order.
    pub fn registered_interfaces(&self) -> &[InterfaceId] {
        &self.interfaces
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestOutput {
    processed_packet_count: usize,
    accepted_announce_count: usize,
    held_for_retry_count: usize,
    scheduled_rebroadcast_count: usize,
}

impl IngestOutput {
    pub const fn processed_packet_count(&self) -> usize {
        self.processed_packet_count
    }
    pub const fn accepted_announce_count(&self) -> usize {
        self.accepted_announce_count
    }
    pub const fn held_for_retry_count(&self) -> usize {
        self.held_for_retry_count
    }
    pub const fn scheduled_rebroadcast_count(&self) -> usize {
        self.scheduled_rebroadcast_count
    }
}

/// One due directive's fanout target list, materialised at tick-time.
/// Private to the engine: callers see typed [`EgressDirective`]s via
/// [`TickOutput::egress_directives`].
#[derive(Debug, Clone)]
struct DirectiveFanout {
    destination: DestinationHash,
    fire_on: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES>,
}

/// What `tick` produced this cycle.
///
/// Holds a mutable borrow on the engine state for its lifetime — so the
/// host can iterate the typed directives via [`egress_directives`], and
/// the engine's commit (drain of processed entries) happens
/// automatically when this value drops. The borrow also structurally
/// enforces the pseudo-pure tick contract: no other state mutation can
/// happen while a `TickOutput` is alive.
///
/// **What stays behind**: only the directives the host is handed this
/// tick get committed (removed from engine state) on Drop. Everything
/// else the engine had scheduled for a future tick — today only
/// not-yet-due entries in the rebroadcast schedule, tomorrow other
/// directive kinds with their own lifecycle (data sends, proofs, link
/// replies, ...) — stays inside engine state and surfaces on a later
/// tick when its time comes. The engine owns its own schedules; the
/// host is a dumb tx/rx pump. The fixed-cap backends used today cap
/// the in-flight schedule — a growable backend is on the roadmap
/// once we want to mentally validate the "engine has effectively
/// infinite room to carry stuff over" model at scale.
///
/// **Fanout is engine-computed**: each yielded [`EgressDirective`]
/// carries an explicit positive `fire_on: &[InterfaceId]` list. The
/// engine builds this from the [registered interfaces](
/// EngineState::registered_interfaces) minus the source, so the host
/// stays a pure tx/rx pump with no "don't reflect to source" filter
/// logic. Directives whose computed `fire_on` would be empty are
/// elided (engine processed → host sees nothing) but still drained on
/// Drop so they don't re-fire next tick.
///
/// [`egress_directives`]: TickOutput::egress_directives
#[must_use]
pub struct TickOutput<'a, R, A, H, D, const MAX_HELD_ANNOUNCES: usize>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    state: &'a mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
    now: InstantMillis,
    recovered_from_held_count: usize,
    fanouts: HeaplessVec<DirectiveFanout, MAX_HELD_ANNOUNCES>,
}

impl<'a, R, A, H, D, const MAX_HELD_ANNOUNCES: usize> TickOutput<'a, R, A, H, D, MAX_HELD_ANNOUNCES>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    /// Count of directives the host receives this tick — identical to
    /// the number of items [`egress_directives`] will yield.
    ///
    /// [`egress_directives`]: TickOutput::egress_directives
    pub fn egress_directive_count(&self) -> usize {
        self.fanouts.len()
    }

    pub const fn recovered_from_held_count(&self) -> usize {
        self.recovered_from_held_count
    }

    /// Iterate every directive the engine is handing to the host this
    /// tick. Read-only — the iterator yields [`EgressDirective`]
    /// borrowing from `&self` (the announce body comes from the
    /// routing table; the `fire_on` slice comes from this
    /// `TickOutput`'s fanout arena). The host can re-iterate, count,
    /// peek, find, collect snapshots. On Drop the engine commits:
    /// exactly the set of entries yielded here is removed from
    /// whichever per-kind schedule produced them; everything else
    /// stays in state for a future tick. Today the only source is the
    /// rebroadcast schedule; as more directive kinds land the
    /// iterator will chain across additional sources, each with its
    /// own commit.
    pub fn egress_directives(&self) -> impl Iterator<Item = EgressDirective<'_>> + '_ {
        let state = &*self.state;
        self.fanouts.iter().filter_map(move |fanout| {
            let retained = state
                .routing_table
                .retained_announce_for(&fanout.destination)?;
            Some(EgressDirective::ReemitAnnounce {
                announce: retained.announce,
                emit_hops: retained.hops,
                fire_on: fanout.fire_on.as_slice(),
            })
        })
    }
}

impl<R, A, H, D, const MAX_HELD_ANNOUNCES: usize> Drop
    for TickOutput<'_, R, A, H, D, MAX_HELD_ANNOUNCES>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    fn drop(&mut self) {
        // Commit the tick: each per-kind schedule drops exactly the
        // entries it just yielded to the host. Anything else stays
        // in state for a later tick. Today the only schedule is the
        // rebroadcast set (drain by `due_at <= self.now`); as more
        // directive kinds land we add their commits here. Failures
        // the host had during dispatch are not retried — per the
        // architecture, failure feedback flows back via future inputs
        // (interface state changes, the protocol's re-broadcast
        // cycle), never via cancellation of the current tick.
        self.state.pending_rebroadcasts.drain_due(self.now);
    }
}

/// Process a batch of inbound packets. Clock-free: each packet carries its own
/// arrival instant, so the result is a pure function of `(state, packets,
/// entropy)`. An empty batch is valid and a no-op.
#[must_use]
pub fn ingest<R, A, H, D, const MAX_HELD_ANNOUNCES: usize>(
    state: &mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
    packets: &[InboundPacket<'_>],
    entropy: u64,
) -> IngestOutput
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    state.ingested_packet_count = state
        .ingested_packet_count
        .saturating_add(packets.len() as u64);

    let mut counters = IngestCounters::default();

    for packet in packets {
        match Ingress::classify(packet) {
            Ingress::Announce {
                announce,
                received_hops,
                source_interface,
                arrived_at,
            } => ingest_announce(
                state,
                announce,
                received_hops,
                source_interface,
                arrived_at,
                entropy,
                &mut counters,
            ),

            // Wire-recognised but not yet handled by the engine. Future
            // slices land their dispatch here.
            Ingress::Data | Ingress::LinkRequest | Ingress::Proof => {}

            // Bad header / failed announce validation; dropped.
            Ingress::Unparseable => {}
        }
    }

    IngestOutput {
        processed_packet_count: packets.len(),
        accepted_announce_count: counters.accepted,
        held_for_retry_count: counters.held,
        scheduled_rebroadcast_count: counters.scheduled,
    }
}

/// Per-batch counters mutated by per-variant ingest handlers. Stays
/// private to `ingest`; the public surface is [`IngestOutput`].
#[derive(Default)]
struct IngestCounters {
    accepted: usize,
    held: usize,
    scheduled: usize,
}

/// Mutates `state` and `counters` in place; returns nothing because
/// every branch's side effects are already captured by the counters.
///
/// WIP - "Always-returns-()" is a current posture, not a committed stance.
/// We need to continue to observe the other handlers' impls and stay
/// vigilant at unifying & coupling what should be, while avoiding
/// doing so for what shouldn't be.
fn ingest_announce<R, A, H, D, const MAX_HELD_ANNOUNCES: usize>(
    state: &mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
    announce: Announce<'_>,
    received_hops: u8,
    source_interface: InterfaceId,
    arrived_at: InstantMillis,
    entropy: u64,
    counters: &mut IngestCounters,
) where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    let decision = AnnounceAcceptanceInput {
        packet_hops: received_hops,
        announce_id: announce.announce_id,
        // No local identities yet, so no announce is ever for us.
        destination_is_local: false,
        existing_route: state
            .routing_table
            .existing_route_for(&announce.destination),
        arrived_at,
    }
    .determine_acceptance();

    if !matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
        return;
    }

    let outcome = state
        .routing_table
        .upsert_route(received_hops, arrived_at, &announce);
    match outcome {
        UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated => {
            counters.accepted += 1;
            let offset = jitter_offset_for(
                entropy,
                &announce.destination,
                DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
            );
            state.pending_rebroadcasts.schedule(
                announce.destination,
                InstantMillis(arrived_at.0.saturating_add(offset)),
                source_interface,
            );
            counters.scheduled += 1;
        }
        UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull) => {
            // Park the structured announce; retry on tick will
            // re-evaluate against current arena state. Park can return
            // CacheFull (cap reached, dropped) — we count only the
            // successful parks.
            use crate::routing::held_cache::{HoldReason, ParkOutcome};
            match state.held_announces_cache.park(
                &announce,
                arrived_at,
                received_hops,
                HoldReason::RoutingArenaPressure,
                source_interface,
            ) {
                ParkOutcome::Parked | ParkOutcome::Overwrote => {
                    counters.held += 1;
                }
                ParkOutcome::CacheFull | ParkOutcome::AppDataTooLarge => {}
            }
        }
        UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull) => {
            // Nowhere to retry to until route eviction exists.
        }
    }
}

/// Advance the engine's periodic work to `now`. Performs the held-cache
/// retry (one entry max per tick), maintains the rebroadcast schedule,
/// and returns a [`TickOutput`] holding `&mut state` until the host has
/// iterated the directives the engine produced.
///
/// `entropy` is the same per-step value passed to `ingest`; reused here
/// so a held-recovery accept gets a deterministic jittered re-emission
/// slot. The returned [`TickOutput`] is itself `#[must_use]`, so
/// dropping it without iterating is a compile-time warning.
pub fn tick<R, A, H, D, const MAX_HELD_ANNOUNCES: usize>(
    state: &mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
    now: InstantMillis,
    entropy: u64,
) -> TickOutput<'_, R, A, H, D, MAX_HELD_ANNOUNCES>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    state.tick_count = state.tick_count.saturating_add(1);

    let mut recovered_from_held_count = 0;
    if let Some(held) = state.held_announces_cache.take_next() {
        use crate::routing::held_cache::HoldReason;
        match held.reason() {
            HoldReason::RoutingArenaPressure => {
                let announce = held.announce();
                let arrival = held.arrived_at();
                let received_hops = held.received_hops();
                let source_interface = held.source_interface();
                let decision = AnnounceAcceptanceInput {
                    packet_hops: received_hops,
                    announce_id: announce.announce_id,
                    destination_is_local: false,
                    existing_route: state
                        .routing_table
                        .existing_route_for(&announce.destination),
                    arrived_at: arrival,
                }
                .determine_acceptance();
                if matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
                    let outcome =
                        state
                            .routing_table
                            .upsert_route(received_hops, arrival, &announce);
                    if matches!(
                        outcome,
                        UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated
                    ) {
                        recovered_from_held_count += 1;
                        let offset = jitter_offset_for(
                            entropy,
                            &announce.destination,
                            DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                        );
                        state.pending_rebroadcasts.schedule(
                            announce.destination,
                            InstantMillis(arrival.0.saturating_add(offset)),
                            source_interface,
                        );
                    }
                    // On Dropped(_) or Reject we discard — see the
                    // held-cache module note on livelock avoidance.
                }
            }
        }
    }

    // Materialise per-due-directive fanout: for each rebroadcast whose
    // due_at <= now, build the positive `fire_on` list (registered
    // interfaces minus the source). Directives whose computed list is
    // empty are elided here (engine processed → host sees nothing) but
    // still drained on Drop so they don't re-fire next tick. The host
    // iterates the typed directives via `TickOutput::egress_directives`
    // and commits on Drop. Anything not-yet-due stays parked in
    // `pending_rebroadcasts` and surfaces on a later tick.
    let mut fanouts: HeaplessVec<DirectiveFanout, MAX_HELD_ANNOUNCES> = HeaplessVec::new();
    for scheduled in state
        .pending_rebroadcasts
        .iter()
        .filter(|sr| sr.due_at <= now)
    {
        let mut fire_on: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES> = HeaplessVec::new();
        for &iface in &state.interfaces {
            if iface != scheduled.source_interface {
                // Push is infallible: state.interfaces is also capped at
                // MAX_REGISTERED_INTERFACES, so the filter never produces
                // more than the destination's capacity.
                let _ = fire_on.push(iface);
            }
        }
        if fire_on.is_empty() {
            continue;
        }
        // Both caps match (MAX_HELD_ANNOUNCES == max due directives == max fanouts),
        // so push is infallible here too.
        let _ = fanouts.push(DirectiveFanout {
            destination: scheduled.destination,
            fire_on,
        });
    }

    TickOutput {
        state,
        now,
        recovered_from_held_count,
        fanouts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{Capabilities, InterfaceMode, MediumKind};
    use crate::wire::{
        DestinationHash, DestinationType, PacketType, PropagationType, WirePacketHeader, MTU,
    };

    /// Fixed entropy so determinism tests can compare two runs apples-to-apples;
    /// the engine treats entropy as opaque data, the value just has to be stable.
    const TEST_ENTROPY: u64 = 0xCAFE_F00D_DEAD_BEEF;

    /// What the tests need to assert against a tick, snapshotted to a
    /// value type so it can outlive the `TickOutput` borrow on state.
    /// `TickOutput` itself holds `&mut state` until drop (the commit),
    /// so we can't bubble it out of `tick_capture` — instead we drain
    /// the directives, copy their wire serialisation, and return both
    /// the counters and the captured bytes.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct TickSnapshot {
        egress_directive_count: usize,
        recovered_from_held_count: usize,
    }

    /// Test-side `tick` helper: runs one tick, serializes every due
    /// directive into its own owned wire buffer, and returns the
    /// captured bytes alongside a [`TickSnapshot`]. Tests that don't
    /// care about emission ignore the byte vec.
    fn tick_capture<R, A, H, D, const MAX_HELD_ANNOUNCES: usize>(
        state: &mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
        now: InstantMillis,
    ) -> (TickSnapshot, std::vec::Vec<std::vec::Vec<u8>>)
    where
        R: RouteColumns,
        A: RetainedAnnounceColumns,
        H: AnnounceIdHistory,
        D: RetainedAppData,
    {
        let tick_out = tick(state, now, TEST_ENTROPY);
        let snapshot = TickSnapshot {
            egress_directive_count: tick_out.egress_directive_count(),
            recovered_from_held_count: tick_out.recovered_from_held_count(),
        };
        let mut emitted = std::vec::Vec::new();
        let mut buf = [0u8; MTU];
        for directive in tick_out.egress_directives() {
            let n = directive.to_wire(&mut buf).expect("serialize directive");
            emitted.push(buf[..n].to_vec());
        }
        (snapshot, emitted)
    }

    #[test]
    fn tick_advances_count_deterministically() {
        let mut left: DefaultEngineState = DefaultEngineState::default();
        let mut right: DefaultEngineState = DefaultEngineState::default();

        let (left_out, left_bytes) = tick_capture(&mut left, InstantMillis(1_000));
        let (right_out, right_bytes) = tick_capture(&mut right, InstantMillis(1_000));

        assert_eq!(left, right);
        assert_eq!(left.tick_count(), 1);
        assert_eq!(left_out, right_out);
        assert_eq!(left_out.egress_directive_count, 0);
        assert!(left_bytes.is_empty());
        assert_eq!(left_bytes, right_bytes);
    }

    #[test]
    fn ingest_counts_the_batch_without_a_clock() {
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let batch = [
            InboundPacket {
                arrived_at: InstantMillis(10),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &[1, 2, 3],
            },
            InboundPacket {
                arrived_at: InstantMillis(20),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &[4],
            },
        ];

        let out = ingest(&mut state, &batch, TEST_ENTROPY);
        assert_eq!(out.processed_packet_count(), 2);
        assert_eq!(state.ingested_packet_count(), 2);

        // Empty batch is valid and does not move state.
        let empty = ingest(&mut state, &[], TEST_ENTROPY);
        assert_eq!(empty.processed_packet_count(), 0);
        assert_eq!(state.ingested_packet_count(), 2);
    }

    // A genuine RNS 1.3.1 announce (the same vector the announce module validates).
    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    struct StaticInterface {
        id: InterfaceId,
        capabilities: Capabilities,
        state: ConnectionState,
    }

    impl StaticInterface {
        fn new(id: InterfaceId) -> Self {
            Self {
                id,
                capabilities: Capabilities {
                    receives: true,
                    transmits: true,
                    forwards: true,
                    repeats: false,
                },
                state: ConnectionState::Connected,
            }
        }

        fn with_state(mut self, state: ConnectionState) -> Self {
            self.state = state;
            self
        }

        fn without_transmit(mut self) -> Self {
            self.capabilities.transmits = false;
            self
        }
    }

    impl Interface for StaticInterface {
        type Error = core::convert::Infallible;

        fn id(&self) -> InterfaceId {
            self.id
        }

        fn capabilities(&self) -> Capabilities {
            self.capabilities
        }

        fn mode(&self) -> InterfaceMode {
            InterfaceMode::Full
        }

        fn medium_kind(&self) -> MediumKind {
            MediumKind::Loopback
        }

        fn state(&self) -> ConnectionState {
            self.state
        }

        fn try_read(&mut self, _buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
            Ok(None)
        }

        fn write(&mut self, _packet: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn register_static_interface(state: &mut DefaultEngineState, id: InterfaceId) {
        let iface = StaticInterface::new(id);
        state.register_routable_interface(&iface).unwrap();
    }

    #[test]
    fn register_routable_interface_uses_the_interface_contract() {
        let id = InterfaceId::new([0xAB; 16]);
        let iface = StaticInterface::new(id);
        let mut state: DefaultEngineState = DefaultEngineState::default();

        assert_eq!(state.register_routable_interface(&iface), Ok(()));
        assert_eq!(state.registered_interfaces(), &[id]);
    }

    #[test]
    fn register_routable_interface_accepts_degraded_transmitting_interfaces() {
        let id = InterfaceId::new([0xBC; 16]);
        let iface = StaticInterface::new(id).with_state(ConnectionState::Degraded);
        let mut state: DefaultEngineState = DefaultEngineState::default();

        assert_eq!(state.register_routable_interface(&iface), Ok(()));
        assert_eq!(state.registered_interfaces(), &[id]);
    }

    #[test]
    fn register_routable_interface_rejects_non_transmitting_interfaces() {
        let iface = StaticInterface::new(InterfaceId::new([0xCD; 16])).without_transmit();
        let mut state: DefaultEngineState = DefaultEngineState::default();

        assert_eq!(
            state.register_routable_interface(&iface),
            Err(RegisterInterfaceError::NotTransmitting)
        );
        assert!(state.registered_interfaces().is_empty());
    }

    #[test]
    fn register_routable_interface_rejects_unroutable_connection_states() {
        for (idx, connection_state) in [
            ConnectionState::Initializing,
            ConnectionState::Reconnecting,
            ConnectionState::Failed,
            ConnectionState::Disconnected,
        ]
        .into_iter()
        .enumerate()
        {
            let iface = StaticInterface::new(InterfaceId::new([idx as u8; 16]))
                .with_state(connection_state);
            let mut state: DefaultEngineState = DefaultEngineState::default();

            assert_eq!(
                state.register_routable_interface(&iface),
                Err(RegisterInterfaceError::NotRoutable {
                    state: connection_state
                })
            );
            assert!(state.registered_interfaces().is_empty());
        }
    }

    #[test]
    fn ingest_accepts_a_real_announce_then_rejects_its_replay() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state: DefaultEngineState = DefaultEngineState::default();

        let first = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(first.accepted_announce_count(), 1);
        assert_eq!(state.route_count(), 1);

        // The identical announce again is a known-route replay: rejected, no new path.
        let second = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(second.processed_packet_count(), 1);
        assert_eq!(second.accepted_announce_count(), 0);
        assert_eq!(state.route_count(), 1);
    }

    #[test]
    fn received_hops_are_incremented_so_the_reach_boundary_matches_pathfinder_m() {
        // RNS increments hops on receive, then accepts only `incremented <
        // PATHFINDER_M+1`. So 127 on the wire (128 after the increment) is the
        // last acceptable value, and 128 on the wire (129 after) is beyond reach.
        // The hop byte lives in the header, not the signed payload, so editing it
        // leaves the announce's signature intact.
        let mut at_limit = hx(RAW_ANNOUNCE);
        at_limit[1] = 127;
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let out = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &at_limit,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);

        let mut beyond = hx(RAW_ANNOUNCE);
        beyond[1] = 128;
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let out = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &beyond,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 0);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn an_accepted_announce_is_retained_for_faithful_rebroadcast() {
        let raw = hx(RAW_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let destination =
            DestinationHash::from_slice(&raw[2..18]).expect("16-byte destination hash");

        let mut state: DefaultEngineState = DefaultEngineState::default();
        let out = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);

        // The structured retained announce reproduces the wire payload exactly
        // via to_wire (so the signature still validates on re-emission), and
        // hops are incremented on receive.
        let retained = state
            .routing_table
            .retained_announce_for(&destination)
            .expect("the accepted announce is on hand");
        assert_eq!(retained.hops, header.hops + 1);
        let mut buf = [0u8; 500];
        let n = retained.announce.to_wire(&mut buf).unwrap();
        assert_eq!(&buf[..n], payload);
    }

    #[test]
    fn ingest_processes_but_does_not_accept_non_announce_bytes() {
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let junk = InboundPacket {
            arrived_at: InstantMillis(1),
            source_interface: InterfaceId::new([0u8; 16]),
            bytes: &[0x00, 0x00, 0x01, 0x02, 0x03],
        };
        let out = ingest(&mut state, &[junk], TEST_ENTROPY);
        assert_eq!(out.processed_packet_count(), 1);
        assert_eq!(out.accepted_announce_count(), 0);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn arena_full_drops_park_the_inbound_bytes_for_retry() {
        // Arena tuned to 8 bytes — smaller than the real announce's 14-byte
        // app_data ("hello-personal") — so upsert returns Dropped(PayloadArenaFull).
        let raw = hx(RAW_ANNOUNCE);
        let mut state = DefaultEngineState::<4, 64, 8>::default();

        let out = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );

        assert_eq!(out.accepted_announce_count(), 0);
        assert_eq!(out.held_for_retry_count(), 1);
        assert_eq!(state.route_count(), 0);
        assert_eq!(state.held_announce_count(), 1);
    }

    #[test]
    fn tick_retries_one_held_entry_and_discards_it_when_the_arena_is_still_full() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state = DefaultEngineState::<4, 64, 8>::default();
        let _ = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.held_announce_count(), 1);

        // Arena state unchanged → retry hits Dropped(PayloadArenaFull) again
        // and the held entry is discarded. We don't re-park (livelock).
        let (out, _bytes) = tick_capture(&mut state, InstantMillis(2_000));
        assert_eq!(out.recovered_from_held_count, 0);
        assert_eq!(state.held_announce_count(), 0);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn a_capable_host_can_widen_the_routing_table_at_the_type_level() {
        // The const-generic lever: a roomier table is just a different type, and
        // ingest is generic over it — same engine, no heap, no API change. (Very
        // large widths belong on the heap; this inline default lives on the stack.)
        let raw = hx(RAW_ANNOUNCE);
        let mut state = DefaultEngineState::<64, 128>::default();
        let out = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);
        assert_eq!(state.route_count(), 1);
    }

    #[test]
    fn accepted_announces_schedule_a_rebroadcast_and_tick_emits_them() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state: DefaultEngineState = DefaultEngineState::default();
        // Register a peer so fanout has a target (source is [0u8;16]; the
        // engine's fire_on = registered minus source = [peer]).
        register_static_interface(&mut state, InterfaceId::new([0xFE; 16]));

        let arrival = InstantMillis(1_000);
        let out = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);
        assert_eq!(out.scheduled_rebroadcast_count(), 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        // Far past the jitter window: the rebroadcast is due and tick emits it.
        let (tick_out, emitted) = tick_capture(
            &mut state,
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        );
        assert_eq!(tick_out.egress_directive_count, 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);

        // The emitted bytes round-trip back to the same announce, with the
        // hop count incremented (received_hops becomes emit hops). Same
        // signature, so the on-wire packet would re-validate on any peer.
        assert_eq!(emitted.len(), 1);
        let wire = &emitted[0];
        let (header, payload) = WirePacketHeader::parse(wire).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.propagation, PropagationType::Broadcast);
        let original = WirePacketHeader::parse(&raw).unwrap().0;
        assert_eq!(header.hops, original.hops + 1);
        assert_eq!(header.destination, original.destination);
        // And the body bytes are byte-for-byte the same as the original wire
        // payload — `Announce::to_wire(from_wire(payload)) == payload`.
        let original_payload = WirePacketHeader::parse(&raw).unwrap().1;
        assert_eq!(payload, original_payload);
    }

    #[test]
    fn pending_rebroadcasts_are_not_emitted_before_their_due_time() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let arrival = InstantMillis(1_000);
        let _ = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        // `now < arrival` is strictly before any due_at — the offset is
        // non-negative so `due_at >= arrival > now`, and nothing emits.
        let (tick_out, emitted) = tick_capture(&mut state, InstantMillis(arrival.0 - 1));
        assert_eq!(tick_out.egress_directive_count, 0);
        assert!(emitted.is_empty());
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);
    }

    #[test]
    fn same_inputs_produce_byte_identical_emissions_on_two_engines() {
        // Determinism: two engines fed the same packets + same entropy emit
        // the same wire bytes at the same tick. The whole point of "entropy
        // as data" — no hidden RNG state moves results around.
        let raw = hx(RAW_ANNOUNCE);
        let now = InstantMillis(5_000);
        let arrival = InstantMillis(1_000);

        let mut left: DefaultEngineState = DefaultEngineState::default();
        let mut right: DefaultEngineState = DefaultEngineState::default();

        for state in [&mut left, &mut right] {
            // Identical registries: byte-identical emissions depend on
            // both engines computing the same fanout target sets.
            register_static_interface(state, InterfaceId::new([0xFE; 16]));
            let _ = ingest(
                state,
                &[InboundPacket {
                    arrived_at: arrival,
                    source_interface: InterfaceId::new([0u8; 16]),
                    bytes: &raw,
                }],
                TEST_ENTROPY,
            );
        }
        let (left_tick, left_bytes) = tick_capture(&mut left, now);
        let (right_tick, right_bytes) = tick_capture(&mut right, now);

        assert_eq!(left, right);
        assert_eq!(left_tick, right_tick);
        assert_eq!(left_bytes, right_bytes);
        assert_eq!(left_bytes.len(), 1);
    }

    #[test]
    fn held_retry_that_fails_does_not_schedule_a_rebroadcast() {
        // Arena stays full across both calls so the held-cache retry inside
        // `tick` also fails. The schedule should not move: only successful
        // accepts schedule. (The successful held-recovery case is exercised
        // once eviction lands and a follow-up packet can free arena space.)
        let raw = hx(RAW_ANNOUNCE);
        let mut state = DefaultEngineState::<4, 64, 8, 4, 16, 4>::default();
        let _ = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.held_announce_count(), 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);

        let (tick_out, bytes) = tick_capture(&mut state, InstantMillis(2_000));
        assert_eq!(tick_out.recovered_from_held_count, 0);
        assert_eq!(tick_out.egress_directive_count, 0);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);
        assert!(bytes.is_empty());
    }

    /// Engine elides empty-fanout directives. Drives a real announce
    /// through a single LoopbackInterface pair, registers only that
    /// engine half, and verifies the engine accepts + schedules the
    /// announce but produces zero directives (fanout = registered −
    /// source = empty). The rebroadcast still drains on tick commit,
    /// so it does not re-fire on a later tick.
    #[cfg(feature = "alloc")]
    #[test]
    fn engine_elides_directives_with_only_the_source_registered() {
        use crate::interfaces::{Interface, InterfaceId, LoopbackInterface};

        let raw = hx(RAW_ANNOUNCE);

        // Two halves of a paired loopback. `seed_half` is what an
        // upstream peer would hold; `engine_half` is what the engine
        // pulls from and writes back to.
        let (mut seed_half, mut engine_half) =
            LoopbackInterface::pair(InterfaceId::new([0x01; 16]), InterfaceId::new([0x02; 16]));
        let engine_iface_id = engine_half.id();

        // == Phase 1: upstream peer sends an announce in ==
        seed_half.write(&raw).unwrap();

        // == Phase 2: engine drains the interface ==
        // `read_inbound` is a default trait method on
        // PointToPointInterface — wraps `try_read` and stamps the
        // InboundPacket with the interface's own id().
        let arrived_at = InstantMillis(1_000);
        let mut read_buf = [0u8; MTU];
        let packet = engine_half
            .read_inbound(&mut read_buf, arrived_at)
            .unwrap()
            .expect("seeded packet ready to read");
        assert_eq!(packet.source_interface, engine_iface_id);

        // == Phase 3: engine ingests, with the one engine-owned interface
        //             registered. Fanout = registered - source = empty,
        //             so the engine elides the directive and the host
        //             sees nothing this tick. ==
        let mut state: DefaultEngineState = DefaultEngineState::default();
        state.register_routable_interface(&engine_half).unwrap();
        let ingest_out = ingest(&mut state, &[packet], TEST_ENTROPY);
        assert_eq!(ingest_out.accepted_announce_count(), 1);
        assert_eq!(ingest_out.scheduled_rebroadcast_count(), 1);
        assert_eq!(state.route_count(), 1);

        // == Phase 4: tick past the jitter window — engine elides the
        //             empty-fanout directive but still drains it. ==
        let now = InstantMillis(arrived_at.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
        {
            let tick_out = tick(&mut state, now, TEST_ENTROPY);
            assert_eq!(tick_out.egress_directive_count(), 0);
            assert!(tick_out.egress_directives().next().is_none());
        }
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);

        // == Phase 5: the upstream peer sees nothing — the engine had no
        //             other interface to fan out to. ==
        assert_eq!(seed_half.try_read(&mut read_buf).unwrap(), None);
    }

    /// Multi-interface forcing-function test, Stage 3 edition. A real
    /// announce arrives on interface A; the engine — which owns both
    /// registered interfaces — computes `fire_on = [B]` (positive,
    /// source excluded) and the host writes the bytes to B with no
    /// exclusion logic of its own. The test is the canonical proof
    /// that Stage 3 holds: the host code that used to have
    /// `if source != engine_X_id { … }` filters is GONE.
    #[cfg(feature = "alloc")]
    #[test]
    fn engine_computed_fire_on_drives_fanout_with_no_host_side_filter() {
        use crate::interfaces::{Interface, InterfaceId, LoopbackInterface};

        let raw = hx(RAW_ANNOUNCE);

        // Two paired loopbacks. Engine "owns" the right halves; the
        // upstream peers (test code) own the left halves.
        let (mut seed_a, mut engine_a) =
            LoopbackInterface::pair(InterfaceId::new([0xA1; 16]), InterfaceId::new([0xA2; 16]));
        let (mut seed_b, mut engine_b) =
            LoopbackInterface::pair(InterfaceId::new([0xB1; 16]), InterfaceId::new([0xB2; 16]));
        let engine_a_id = engine_a.id();
        let engine_b_id = engine_b.id();

        // Phase 1: peer reachable via A sends an announce.
        seed_a.write(&raw).unwrap();

        // Phase 2: engine drains BOTH interfaces, builds one batch.
        // Separate scratch buf per interface — a shared buf would let
        // the second drain clobber the first's contents (the
        // InboundPackets borrow from the bufs).
        let arrived_at = InstantMillis(1_000);
        let mut buf_a = [0u8; MTU];
        let mut buf_b = [0u8; MTU];
        let mut state: DefaultEngineState = DefaultEngineState::default();
        state.register_routable_interface(&engine_a).unwrap();
        state.register_routable_interface(&engine_b).unwrap();
        {
            let mut batch = std::vec::Vec::new();
            if let Some(p) = engine_a.read_inbound(&mut buf_a, arrived_at).unwrap() {
                batch.push(p);
            }
            if let Some(p) = engine_b.read_inbound(&mut buf_b, arrived_at).unwrap() {
                batch.push(p);
            }
            assert_eq!(batch.len(), 1, "only A had a packet queued");
            assert_eq!(batch[0].source_interface, engine_a_id);

            // Phase 3: ingest the unified batch.
            let ingest_out = ingest(&mut state, &batch, TEST_ENTROPY);
            assert_eq!(ingest_out.accepted_announce_count(), 1);
            assert_eq!(ingest_out.scheduled_rebroadcast_count(), 1);
            assert_eq!(state.route_count(), 1);
            // batch (and the buf borrows it holds) drops at end of block
        }

        // Phase 4 + 5: tick past the jitter window — engine produces one
        // directive with `fire_on = [engine_b_id]`. The host writes the
        // bytes to each id in fire_on, no source filter. Both phases
        // live inside the tick_out scope so the borrow on state is
        // released before Phase 6/7 reads.
        let now = InstantMillis(arrived_at.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
        let mut wire_buf = [0u8; MTU];
        {
            let tick_out = tick(&mut state, now, TEST_ENTROPY);
            assert_eq!(tick_out.egress_directive_count(), 1);

            let mut wrote = 0usize;
            for directive in tick_out.egress_directives() {
                assert_eq!(
                    directive.fire_on(),
                    &[engine_b_id],
                    "engine must compute fire_on as registered interfaces minus source"
                );
                let n = directive
                    .to_wire(&mut wire_buf)
                    .expect("serialize directive");

                // HOST is now a pure pump: write to each fire_on id.
                // No `if source != X` filter — the engine already did it.
                for target in directive.fire_on() {
                    if *target == engine_a_id {
                        engine_a.write(&wire_buf[..n]).unwrap();
                    } else if *target == engine_b_id {
                        engine_b.write(&wire_buf[..n]).unwrap();
                    }
                }
                wrote += 1;
            }
            assert_eq!(wrote, 1);
        }

        // Phase 6: A's peer should NOT have received the rebroadcast
        // (engine excluded A from fire_on).
        assert_eq!(
            seed_a.try_read(&mut buf_a).unwrap(),
            None,
            "A is the source — engine-computed fire_on excludes A"
        );

        // Phase 7: B's peer SHOULD have received the rebroadcast, with
        // hop count incremented (received_hops = orig + 1 on the
        // engine's receive, re-emitted at that value).
        let n = seed_b
            .try_read(&mut buf_b)
            .unwrap()
            .expect("B should receive the fan-out");
        let rebroadcast_bytes = &buf_b[..n];
        let (orig_header, _) = WirePacketHeader::parse(&raw).unwrap();
        let (rebroadcast_header, _) = WirePacketHeader::parse(rebroadcast_bytes).unwrap();
        assert_eq!(rebroadcast_header.hops, orig_header.hops + 1);
        assert_eq!(rebroadcast_header.destination, orig_header.destination);
    }
}
