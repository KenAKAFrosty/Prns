//! Pure protocol engine boundary.
//!
//! The engine has two verbs. `ingest` takes a batch of inbound packets, each
//! frozen with the instant it arrived, and is clock-free. `tick` advances the
//! engine's periodic work to a caller-supplied `now`. Neither reads clocks,
//! sockets, or storage directly.

use crate::interfaces::InterfaceId;
use crate::outbox::{Outbox, OutboxFull};
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
use crate::wire::{
    Context, ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WirePacketHeader,
    HEADER_LEN,
};

/// Monotonic timestamp supplied by the host, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

/// One inbound packet, frozen with the instant it arrived and tagged with
/// the interface it came in on. The host stamps `arrived_at` when it
/// enqueues the packet so `ingest` processes a fixed record and never
/// needs to read a clock; `source_interface` carries through to emission
/// so the engine can apply RNS's "don't gossip an announce back to its
/// source interface" rule (and any future medium-aware policy keyed on
/// the source).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboundPacket<'a> {
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub bytes: &'a [u8],
}

/// One outbound packet the engine wants transmitted. The bytes are the
/// finished wire packet; `maybe_source_interface` carries the identity
/// of the interface that originally delivered the announce we're
/// re-emitting (the canonical use: the host's fanout skips that
/// interface). `None` means the engine originated this packet — for
/// example a future origin-side announce — and the host should fan to
/// every interface it owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundPacket<'a> {
    pub bytes: &'a [u8],
    pub maybe_source_interface: Option<InterfaceId>,
}

/// Retained engine state. **Purely abstract** in its type parameters — does
/// not name a preset. The no_std stack-resident preset lives in
/// [`DefaultEngineState`]; that's the canonical embedded entry point. A
/// capable host substitutes alternate routing-storage backends at the type
/// parameters directly.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EngineState<R, A, H, D, const HELD: usize>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    tick_count: u64,
    ingested_packet_count: u64,
    routing_table: RoutingTable<R, A, H, D>,
    held_cache: HeldAnnouncesCache<HELD>,
    // The pending-rebroadcast set is capped at `HELD`, reusing the held-cache
    // dial: one slot per destination, and the realistic per-tick burst of
    // unique accepts is the same order of magnitude as `HELD` (we already park
    // up to `HELD` arena-pressure overflows per tick). Hosts that genuinely
    // see larger bursts widen `HELD` and get both columns at once. A separate
    // `MAX_PENDING` dial is the obvious next iteration if that assumption
    // breaks.
    pending_rebroadcasts: PendingRebroadcasts<HELD>,
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

impl<R, A, H, D, const HELD: usize> EngineState<R, A, H, D, HELD>
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

    /// Number of destinations the engine currently has a path to.
    pub fn route_count(&self) -> usize {
        self.routing_table.route_count()
    }

    /// Number of announces currently parked for retry after a transient
    /// arena-full bail. Drains as `tick()` retries them.
    pub fn held_count(&self) -> usize {
        self.held_cache.len()
    }

    /// Number of accepted destinations currently waiting to be re-emitted on
    /// a future tick. Drains as `tick()` emits each due rebroadcast.
    pub fn pending_rebroadcast_count(&self) -> usize {
        self.pending_rebroadcasts.pending_count()
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
    /// Every inbound packet the batch looked at, parseable or not.
    pub const fn processed_packet_count(&self) -> usize {
        self.processed_packet_count
    }

    /// How many of those were valid announces accepted into the routing table.
    pub const fn accepted_announce_count(&self) -> usize {
        self.accepted_announce_count
    }

    /// How many announces this batch parked in the held-cache after a
    /// transient `Dropped(PayloadArenaFull)`. They retry on subsequent ticks.
    pub const fn held_for_retry_count(&self) -> usize {
        self.held_for_retry_count
    }

    /// How many destinations this batch scheduled for re-emission. Equal to
    /// `accepted_announce_count` whenever every accept finds a slot in the
    /// pending-rebroadcast set.
    pub const fn scheduled_rebroadcast_count(&self) -> usize {
        self.scheduled_rebroadcast_count
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickOutput {
    emitted_packet_count: usize,
    recovered_from_held_count: usize,
}

impl TickOutput {
    pub const fn emitted_packet_count(&self) -> usize {
        self.emitted_packet_count
    }

    /// How many held announces this tick installed into the routing table on
    /// retry. Each tick attempts at most one — the cache typically drains
    /// across several ticks as routine traffic frees arena space.
    pub const fn recovered_from_held_count(&self) -> usize {
        self.recovered_from_held_count
    }
}

/// Process a batch of inbound packets. Clock-free: each packet carries its own
/// arrival instant, so the result is a pure function of `(state, packets,
/// entropy)`. An empty batch is valid and a no-op.
///
/// Each packet is decoded to a header and, if it is a valid announce, run
/// through the acceptance predicate; accepted announces install or refresh a
/// path and schedule a jittered re-emission. `entropy` seeds the per-(entropy,
/// destination) jitter so the same input batch from two hosts with the same
/// entropy schedules identically — determinism is a property of the inputs,
/// not of a hidden RNG. Bytes that don't parse, or aren't announces, are
/// counted as processed and otherwise ignored — this slice acts only on
/// announces.
#[must_use]
pub fn ingest<R, A, H, D, const HELD: usize>(
    state: &mut EngineState<R, A, H, D, HELD>,
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

    let mut accepted_announce_count = 0;
    let mut held_for_retry_count = 0;
    let mut scheduled_rebroadcast_count = 0;
    for packet in packets {
        let Ok((header, payload)) = WirePacketHeader::parse(packet.bytes) else {
            continue;
        };
        let Ok(announce) = Announce::from_wire(&header, payload) else {
            continue;
        };

        // Self-check the parse↔serialize round-trip on every accepted announce
        // in debug builds: if `to_wire` ever drifts from `from_wire`, we'd
        // silently re-emit a signature-broken packet on rebroadcast. Cheap in
        // debug (one MTU-sized scratch copy + compare), zero in release.
        debug_assert!(
            {
                let mut scratch = [0u8; 500];
                announce
                    .to_wire(&mut scratch)
                    .map(|n| &scratch[..n] == payload)
                    .unwrap_or(false)
            },
            "Announce::to_wire(from_wire(payload)) must equal payload"
        );

        // RNS increments a packet's hop count on receipt, before both the
        // acceptance gate and storing the path, so every downstream comparison
        // and the reported hop count use the incremented value.
        // https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L1455
        // Unconditional while we have no interfaces; RNS decrements for
        // local-client / shared-instance hops, which we don't model yet.
        let received_hops = header.hops.saturating_add(1);

        let decision = AnnounceAcceptanceInput {
            packet_hops: received_hops,
            announce_id: announce.announce_id,
            // No local identities yet, so no announce is ever for us.
            destination_is_local: false,
            existing_route: state
                .routing_table
                .existing_route_for(&announce.destination),
            arrived_at: packet.arrived_at,
        }
        .determine_acceptance();

        if matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
            let outcome =
                state
                    .routing_table
                    .upsert_route(received_hops, packet.arrived_at, &announce);
            match outcome {
                UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated => {
                    accepted_announce_count += 1;
                    let offset = jitter_offset_for(
                        entropy,
                        &announce.destination,
                        DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                    );
                    state.pending_rebroadcasts.schedule(
                        announce.destination,
                        InstantMillis(packet.arrived_at.0.saturating_add(offset)),
                        packet.source_interface,
                    );
                    scheduled_rebroadcast_count += 1;
                }
                UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull) => {
                    // Park the structured announce; retry on tick will
                    // re-evaluate against current arena state. Park can
                    // return CacheFull (cap reached, dropped) — we count
                    // only the successful parks.
                    use crate::routing::held_cache::{HoldReason, ParkOutcome};
                    match state.held_cache.park(
                        &announce,
                        packet.arrived_at,
                        received_hops,
                        HoldReason::RoutingArenaPressure,
                        packet.source_interface,
                    ) {
                        ParkOutcome::Parked | ParkOutcome::Overwrote => {
                            held_for_retry_count += 1;
                        }
                        ParkOutcome::CacheFull | ParkOutcome::AppDataTooLarge => {}
                    }
                }
                UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull) => {
                    // Nowhere to retry to until route eviction exists.
                }
            }
        }
    }

    IngestOutput {
        processed_packet_count: packets.len(),
        accepted_announce_count,
        held_for_retry_count,
        scheduled_rebroadcast_count,
    }
}

/// Advance the engine's periodic work to `now`, draining due rebroadcasts
/// into a host-lent `outbox`. Retries up to one held announce per tick,
/// selecting the lowest-hop entry (RNS parity); a recovered held entry is
/// scheduled the same way a fresh accept is. Failed retries are discarded
/// rather than re-parked.
///
/// `entropy` is the same per-step value passed to `ingest`; reused here so a
/// held-recovery accept gets a deterministic jittered re-emission slot.
#[must_use]
pub fn tick<R, A, H, D, const HELD: usize, const ARENA: usize, const MAX_PACKETS: usize>(
    state: &mut EngineState<R, A, H, D, HELD>,
    now: InstantMillis,
    entropy: u64,
    outbox: &mut Outbox<ARENA, MAX_PACKETS>,
) -> TickOutput
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    state.tick_count = state.tick_count.saturating_add(1);

    let mut recovered_from_held_count = 0;
    if let Some(held) = state.held_cache.take_next() {
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

    let mut emitted_packet_count = 0;
    loop {
        let Some(scheduled) = state.pending_rebroadcasts.take_due(now) else {
            break;
        };

        // Look up + write the wire packet straight into the outbox. The
        // retained-announce borrow lives only inside this match arm so the
        // next loop pass can call `state.pending_rebroadcasts.*` again.
        let emit_outcome: EmitOutcome = match state
            .routing_table
            .retained_announce_for(&scheduled.destination)
        {
            // The route was evicted between scheduling and draining (future
            // eviction surface — today this never fires). Drop silently.
            None => EmitOutcome::RouteGone,
            Some(retained) => {
                let context_flag = if retained.announce.maybe_ratchet.is_some() {
                    ContextFlag::Set
                } else {
                    ContextFlag::Unset
                };
                let header = WirePacketHeader {
                    ifac_flag: IfacFlag::Open,
                    context_flag,
                    propagation: PropagationType::Broadcast,
                    destination_type: DestinationType::Single,
                    packet_type: PacketType::Announce,
                    hops: retained.hops,
                    transport_id: None,
                    destination: scheduled.destination,
                    context: Context::None,
                };
                let payload_len = retained.announce.wire_len();
                let total_len = HEADER_LEN + payload_len;
                match outbox.write_packet(total_len, Some(scheduled.source_interface), |buf| {
                    let _ = header
                        .write(&mut buf[..HEADER_LEN])
                        .expect("HEADER_LEN bytes always fit a Type-1 header");
                    let _ = retained
                        .announce
                        .to_wire(&mut buf[HEADER_LEN..])
                        .expect("payload_len bytes always fit the announce body");
                }) {
                    Ok(()) => EmitOutcome::Emitted,
                    Err(OutboxFull::Bytes | OutboxFull::Packets) => EmitOutcome::OutboxFull,
                }
            }
        };

        match emit_outcome {
            EmitOutcome::Emitted => emitted_packet_count += 1,
            EmitOutcome::RouteGone => continue,
            EmitOutcome::OutboxFull => {
                // Put it back at its original due_at + source so the host
                // can drain and we re-emit on a later tick.
                state.pending_rebroadcasts.schedule(
                    scheduled.destination,
                    scheduled.due_at,
                    scheduled.source_interface,
                );
                break;
            }
        }
    }

    TickOutput {
        emitted_packet_count,
        recovered_from_held_count,
    }
}

enum EmitOutcome {
    Emitted,
    RouteGone,
    OutboxFull,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::DestinationHash;

    /// Fixed entropy so determinism tests can compare two runs apples-to-apples;
    /// the engine treats entropy as opaque data, the value just has to be stable.
    const TEST_ENTROPY: u64 = 0xCAFE_F00D_DEAD_BEEF;

    /// Test-side `tick` helper: lends an outbox sized for any one
    /// announce-emission test and returns the captured outbound bytes
    /// alongside the `TickOutput`. Tests that don't care about emission just
    /// destructure the first element.
    fn tick_capture<R, A, H, D, const HELD: usize>(
        state: &mut EngineState<R, A, H, D, HELD>,
        now: InstantMillis,
    ) -> (TickOutput, std::vec::Vec<std::vec::Vec<u8>>)
    where
        R: RouteColumns,
        A: RetainedAnnounceColumns,
        H: AnnounceIdHistory,
        D: RetainedAppData,
    {
        let mut outbox = Outbox::<2048, 16>::new();
        let out = tick(state, now, TEST_ENTROPY, &mut outbox);
        let emitted = outbox.iter().map(|p| p.bytes.to_vec()).collect();
        (out, emitted)
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
        assert_eq!(left_out.emitted_packet_count(), 0);
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
        assert_eq!(state.held_count(), 1);
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
        assert_eq!(state.held_count(), 1);

        // Arena state unchanged → retry hits Dropped(PayloadArenaFull) again
        // and the held entry is discarded. We don't re-park (livelock).
        let (out, _bytes) = tick_capture(&mut state, InstantMillis(2_000));
        assert_eq!(out.recovered_from_held_count(), 0);
        assert_eq!(state.held_count(), 0);
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
        assert_eq!(state.pending_rebroadcast_count(), 1);

        // Far past the jitter window: the rebroadcast is due and tick emits it.
        let (tick_out, emitted) = tick_capture(
            &mut state,
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        );
        assert_eq!(tick_out.emitted_packet_count(), 1);
        assert_eq!(state.pending_rebroadcast_count(), 0);

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
        assert_eq!(state.pending_rebroadcast_count(), 1);

        // `now < arrival` is strictly before any due_at — the offset is
        // non-negative so `due_at >= arrival > now`, and nothing emits.
        let (tick_out, emitted) = tick_capture(&mut state, InstantMillis(arrival.0 - 1));
        assert_eq!(tick_out.emitted_packet_count(), 0);
        assert!(emitted.is_empty());
        assert_eq!(state.pending_rebroadcast_count(), 1);
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
        assert_eq!(state.held_count(), 1);
        assert_eq!(state.pending_rebroadcast_count(), 0);

        let (tick_out, bytes) = tick_capture(&mut state, InstantMillis(2_000));
        assert_eq!(tick_out.recovered_from_held_count(), 0);
        assert_eq!(tick_out.emitted_packet_count(), 0);
        assert_eq!(state.pending_rebroadcast_count(), 0);
        assert!(bytes.is_empty());
    }

    #[test]
    fn outbox_full_reschedules_so_emission_resumes_next_tick() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let _ = ingest(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.pending_rebroadcast_count(), 1);

        // Outbox too small to hold the wire packet (HEADER_LEN + 162 byte
        // payload). The emit attempts to write, fails on byte budget, and
        // re-schedules at the same due_at.
        let mut tiny_outbox = Outbox::<16, 4>::new();
        let now = InstantMillis(2_000);
        let tick_out = tick(&mut state, now, TEST_ENTROPY, &mut tiny_outbox);
        assert_eq!(tick_out.emitted_packet_count(), 0);
        assert_eq!(state.pending_rebroadcast_count(), 1);
        assert_eq!(tiny_outbox.len(), 0);

        // A roomy outbox later still drains the same scheduled entry.
        let mut roomy = Outbox::<2048, 4>::new();
        let tick_out = tick(&mut state, now, TEST_ENTROPY, &mut roomy);
        assert_eq!(tick_out.emitted_packet_count(), 1);
        assert_eq!(state.pending_rebroadcast_count(), 0);
    }

    /// End-to-end interface ↔ engine integration. Drives a real announce
    /// through a LoopbackInterface pair, using the `read_inbound` default
    /// trait method to bridge the interface read into an `InboundPacket`,
    /// and verifies the `source_interface` tag threads cleanly through
    /// ingest → schedule → tick → outbox → OutboundPacket.
    #[cfg(feature = "alloc")]
    #[test]
    fn an_announce_traverses_the_engine_via_a_loopback_interface() {
        use crate::interfaces::{Interface, InterfaceId, LoopbackInterface};
        use crate::wire::MTU;

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

        // == Phase 3: engine ingests ==
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let ingest_out = ingest(&mut state, &[packet], TEST_ENTROPY);
        assert_eq!(ingest_out.accepted_announce_count(), 1);
        assert_eq!(ingest_out.scheduled_rebroadcast_count(), 1);
        assert_eq!(state.route_count(), 1);

        // == Phase 4: engine ticks past the jitter window and emits ==
        let mut outbox = Outbox::<2048, 16>::new();
        let now = InstantMillis(arrived_at.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
        let tick_out = tick(&mut state, now, TEST_ENTROPY, &mut outbox);
        assert_eq!(tick_out.emitted_packet_count(), 1);

        // == Phase 5: emitted OutboundPacket carries the source tag end-to-end ==
        let emitted: std::vec::Vec<OutboundPacket<'_>> = outbox.iter().collect();
        assert_eq!(emitted.len(), 1);
        assert_eq!(
            emitted[0].maybe_source_interface,
            Some(engine_iface_id),
            "engine must thread maybe_source_interface from ingest through emission \
             so the host can apply the fanout exclusion"
        );

        // == Phase 6: engine writes the outbox bytes back to the interface ==
        engine_half.write(emitted[0].bytes).unwrap();

        // == Phase 7: upstream peer reads the rebroadcast ==
        let n = seed_half
            .try_read(&mut read_buf)
            .unwrap()
            .expect("rebroadcast available to upstream peer");
        let rebroadcast_bytes = &read_buf[..n];

        // The bytes the seed end receives are the re-emitted announce:
        // same destination, same payload, hop count incremented by 1.
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let (rebroadcast_header, rebroadcast_payload) =
            WirePacketHeader::parse(rebroadcast_bytes).unwrap();
        assert_eq!(rebroadcast_header.hops, orig_header.hops + 1);
        assert_eq!(rebroadcast_header.destination, orig_header.destination);
        assert_eq!(rebroadcast_payload, orig_payload);
    }
}
