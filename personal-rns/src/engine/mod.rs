//! Pure protocol engine boundary.
//!
//! The engine has two verbs. `ingest` takes a batch of inbound packets, each
//! frozen with the instant it arrived, and is clock-free. `tick` advances the
//! engine's periodic work to a caller-supplied `now`. Neither reads clocks,
//! sockets, or storage directly.

use crate::routing::announce::{Announce, AnnounceAcceptanceDecision, AnnounceAcceptanceInput};
use crate::routing::held_cache::{HeldAnnouncesCache, DEFAULT_HELD_CACHE_CAPACITY};
use crate::routing::storage::{
    AnnounceIdHistory, FixedArrayRetainedAnnounceColumns, FixedArrayRouteColumns,
    PackedAppDataArena, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
    TieredAnnounceIdHistory,
};
use crate::routing::{
    DropCause, RoutingTable, UpsertRouteOutcome, DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
    DEFAULT_HISTORY_CAP_PER_DESTINATION, DEFAULT_HISTORY_FLOOR_PER_DESTINATION,
    DEFAULT_HISTORY_OVERFLOW_CAPACITY, DEFAULT_MAX_TRACKED_DESTINATIONS,
};
use crate::wire::WirePacketHeader;

/// Monotonic timestamp supplied by the host, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

/// One inbound packet, frozen with the instant it arrived. The host stamps
/// `arrival` when it enqueues the packet, so `ingest` processes a fixed record
/// and never needs to read a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboundPacket<'a> {
    pub arrival: InstantMillis,
    pub bytes: &'a [u8],
}

/// One outbound packet the engine wants transmitted. A semantic wrapper over
/// the bytes; the host decides which transport carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundPacket<'a> {
    pub bytes: &'a [u8],
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
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestOutput {
    processed_packet_count: usize,
    accepted_announce_count: usize,
    held_for_retry_count: usize,
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
/// arrival instant, so the result is a pure function of `(state, packets)`. An
/// empty batch is valid and a no-op.
///
/// Each packet is decoded to a header and, if it is a valid announce, run
/// through the acceptance predicate; accepted announces install or refresh a
/// path. Bytes that don't parse, or aren't announces, are counted as processed
/// and otherwise ignored — this slice acts only on announces.
#[must_use]
pub fn ingest<R, A, H, D, const HELD: usize>(
    state: &mut EngineState<R, A, H, D, HELD>,
    packets: &[InboundPacket<'_>],
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
            arrived_at: packet.arrival,
        }
        .determine_acceptance();

        if matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
            let outcome =
                state
                    .routing_table
                    .upsert_route(received_hops, packet.arrival, &announce);
            match outcome {
                UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated => {
                    accepted_announce_count += 1;
                }
                UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull) => {
                    // Park the structured announce; retry on tick will
                    // re-evaluate against current arena state. Park can
                    // return CacheFull (cap reached, dropped) — we count
                    // only the successful parks.
                    use crate::routing::held_cache::{HoldReason, ParkOutcome};
                    match state.held_cache.park(
                        &announce,
                        packet.arrival,
                        received_hops,
                        HoldReason::RoutingArenaPressure,
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
    }
}

/// Advance the engine's periodic work to `now`. Retries up to one held
/// announce per tick, selecting the lowest-hop entry (RNS parity). Failed
/// retries are discarded rather than re-parked.
#[must_use]
pub fn tick<R, A, H, D, const HELD: usize>(
    state: &mut EngineState<R, A, H, D, HELD>,
    _now: InstantMillis,
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
                    }
                    // On Dropped(_) or Reject we discard — see the
                    // held-cache module note on livelock avoidance.
                }
            }
        }
    }

    TickOutput {
        emitted_packet_count: 0,
        recovered_from_held_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::DestinationHash;

    #[test]
    fn tick_advances_count_deterministically() {
        let mut left: DefaultEngineState = DefaultEngineState::default();
        let mut right: DefaultEngineState = DefaultEngineState::default();

        let left_out = tick(&mut left, InstantMillis(1_000));
        let right_out = tick(&mut right, InstantMillis(1_000));

        assert_eq!(left, right);
        assert_eq!(left.tick_count(), 1);
        assert_eq!(left_out, right_out);
        assert_eq!(left_out.emitted_packet_count(), 0);
    }

    #[test]
    fn ingest_counts_the_batch_without_a_clock() {
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let batch = [
            InboundPacket {
                arrival: InstantMillis(10),
                bytes: &[1, 2, 3],
            },
            InboundPacket {
                arrival: InstantMillis(20),
                bytes: &[4],
            },
        ];

        let out = ingest(&mut state, &batch);
        assert_eq!(out.processed_packet_count(), 2);
        assert_eq!(state.ingested_packet_count(), 2);

        // Empty batch is valid and does not move state.
        let empty = ingest(&mut state, &[]);
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
                arrival: InstantMillis(1_000),
                bytes: &raw,
            }],
        );
        assert_eq!(first.accepted_announce_count(), 1);
        assert_eq!(state.route_count(), 1);

        // The identical announce again is a known-route replay: rejected, no new path.
        let second = ingest(
            &mut state,
            &[InboundPacket {
                arrival: InstantMillis(2_000),
                bytes: &raw,
            }],
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
                arrival: InstantMillis(1_000),
                bytes: &at_limit,
            }],
        );
        assert_eq!(out.accepted_announce_count(), 1);

        let mut beyond = hx(RAW_ANNOUNCE);
        beyond[1] = 128;
        let mut state: DefaultEngineState = DefaultEngineState::default();
        let out = ingest(
            &mut state,
            &[InboundPacket {
                arrival: InstantMillis(1_000),
                bytes: &beyond,
            }],
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
                arrival: InstantMillis(1_000),
                bytes: &raw,
            }],
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
            arrival: InstantMillis(1),
            bytes: &[0x00, 0x00, 0x01, 0x02, 0x03],
        };
        let out = ingest(&mut state, &[junk]);
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
                arrival: InstantMillis(1_000),
                bytes: &raw,
            }],
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
                arrival: InstantMillis(1_000),
                bytes: &raw,
            }],
        );
        assert_eq!(state.held_count(), 1);

        // Arena state unchanged → retry hits Dropped(PayloadArenaFull) again
        // and the held entry is discarded. We don't re-park (livelock).
        let out = tick(&mut state, InstantMillis(2_000));
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
                arrival: InstantMillis(1_000),
                bytes: &raw,
            }],
        );
        assert_eq!(out.accepted_announce_count(), 1);
        assert_eq!(state.route_count(), 1);
    }
}
