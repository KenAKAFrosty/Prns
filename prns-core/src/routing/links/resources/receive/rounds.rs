//! The request rounds: parts land against the salted register, hashmap updates extend it, and the next pull goes out. Each round also feeds the rate, RTT, and window dynamics.

use crate::engine::{Directive, EngineReaction, EngineState, InstantMillis};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::ingress::{DataPacket, IgnoreReason, IngestPacketOutcome};
use crate::routing::links::data::write_link_packet;
use crate::routing::links::data::{link_data_frame_ceiling, LINK_MDU};
use crate::routing::links::resources::advertisement::parse_hashmap_update_plaintext;
use crate::routing::links::resources::assemble_incoming::match_part_in_window;
use crate::routing::links::resources::control::write_part_request_plaintext;
use crate::routing::links::resources::streamed_open::{
    OpenProgress, ResourceOpenLane, StreamedOpen,
};
use crate::routing::links::resources::table::IncomingResourceState;
use crate::routing::links::resources::table::{IncomingResourceStatus, PlacePartOutcome};
use crate::routing::links::resources::{
    ResourceFailureCause, ResourceHash, ESTABLISHMENT_COST_ESTIMATE_BYTES, FAST_RATE_THRESHOLD,
    MAP_HASH_LEN, PART_REQUEST_MAX_RETRIES, PART_TIMEOUT_FACTOR_AFTER_RTT, PER_RETRY_DELAY_MS,
    RATE_FAST_BYTES_PER_SECOND, RATE_VERY_SLOW_BYTES_PER_SECOND, RETRY_GRACE_MS,
    VERY_SLOW_RATE_THRESHOLD, WINDOW_FLEXIBILITY, WINDOW_MAX, WINDOW_MAX_VERY_SLOW,
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::{DestinationType, PacketType, WireContext};

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.4.2 `Resource.request_next`; the request flags hashmap-exhausted, carrying the last known name, when the window runs past the names received.
    pub(crate) fn emit_resource_pull<F>(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        now: InstantMillis,
        fill_random: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> EmitResourcePullOutcome
    where
        F: FnMut(&mut [u8]),
    {
        let Some(index) = self.incoming_resources.lookup(link_id, hash) else {
            return EmitResourcePullOutcome::NotTracked;
        };
        let state = *self.incoming_resources.state(index);

        let mut requested = [0u8; WINDOW_MAX * MAP_HASH_LEN];
        let mut requested_count = 0;
        let mut exhausted = false;
        {
            let received = self.incoming_resources.received_flags(index);
            let names = self.incoming_resources.names_flat(index);
            let mut position = state.consecutive_completed.map_or(0, |height| height + 1);
            let mut scanned = 0;
            while position < state.part_count && scanned < state.window {
                if !received[position] {
                    if position < state.hashmap_height {
                        requested[requested_count * MAP_HASH_LEN..][..MAP_HASH_LEN]
                            .copy_from_slice(&names[position * MAP_HASH_LEN..][..MAP_HASH_LEN]);
                        requested_count += 1;
                    } else {
                        exhausted = true;
                        break;
                    }
                }
                position += 1;
                scanned += 1;
            }
        }
        if requested_count == 0 && !exhausted {
            return EmitResourcePullOutcome::NothingToRequest;
        }
        let names = self.incoming_resources.names_flat(index);
        let last = state.hashmap_height.saturating_sub(1);
        let Ok(last_known) =
            <[u8; MAP_HASH_LEN]>::try_from(&names[last * MAP_HASH_LEN..(last + 1) * MAP_HASH_LEN])
        else {
            return EmitResourcePullOutcome::NothingToRequest;
        };
        {
            let state = self.incoming_resources.state_mut(index);
            state.outstanding_part_count = requested_count;
            state.waiting_for_hmu = exhausted;
        }

        let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            rtt,
            ..
        }) = self.links.phase_for(link_id)
        else {
            return EmitResourcePullOutcome::LinkNotActive;
        };
        let mtu = *mtu;
        let fire_on = *attached_interface;
        let rtt_millis = rtt.millis();
        let mut iv = [0u8; 16];
        fill_random(&mut iv);
        let mut request_wire_len = 0u64;
        {
            let mut fill = |slot: &mut [u8]| -> Option<usize> {
                let mut plaintext = [0u8; 1 + MAP_HASH_LEN + 32 + WINDOW_MAX * MAP_HASH_LEN];
                let plaintext_len = write_part_request_plaintext(
                    hash,
                    exhausted.then_some(&last_known),
                    &requested[..requested_count * MAP_HASH_LEN],
                    &mut plaintext,
                )
                .ok()?;
                let wire_bytes = write_link_packet(
                    link_id,
                    key,
                    mtu,
                    WireContext::ResourceRequest,
                    &plaintext[..plaintext_len],
                    &iv,
                    slot,
                )
                .ok()?;
                request_wire_len = wire_bytes as u64;
                Some(wire_bytes)
            };
            sink(EngineReaction::Directive(Directive::EmitFrame {
                target: fire_on,
                size_hint: link_data_frame_ceiling(LINK_MDU),
                fill: &mut fill,
            }));
        }
        if request_wire_len > 0 {
            self.links.note_outbound(link_id, now);
        }
        {
            let state = self.incoming_resources.state_mut(index);
            state.request_sent_at = Some(now);
            state.request_sent_bytes = request_wire_len;
            state.received_byte_count_at_request = state.received_byte_count;
            state.awaiting_round_first_response = true;
        }
        let state = *self.incoming_resources.state(index);
        self.incoming_resources
            .set_timeout_at(index, Some(part_round_deadline(&state, rtt_millis, now)));
        EmitResourcePullOutcome::Requested
    }

    /// RNS 1.4.2's link dispatch for context `RESOURCE`.
    ///
    /// A part packet is nothing but sealed part bytes.
    /// So every in-flight transfer on the link tries to claim it by its salted name: `full_hash(part ‖ salt)` truncated to the 4-byte map hash, scanned within the transfer's open request window.
    ///
    /// Parts are exempt from the duplicate filter (RNS 1.4.2 `Transport.packet_filter` exempts `RESOURCE`/`RESOURCE_REQ`/`RESOURCE_PRF` the same way) because a re-requested part is retransmitted byte-identical, so the hashlist would refuse exactly the retries we ask for.
    ///
    /// Rate accounting counts the part's payload plus the request's whole frame, where the reference counts both whole frames. This means the nineteen header bytes are not counted in our case, but since the goal is to classify a 25x-apart threshold (250 bytes/sec for very slow detector and 6,250 bytes/sec for fast link detector), this is negligible and would require extra plumbing and accounting that doesn't justify itself for our implementation.
    pub(crate) fn ingest_resource_part<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::from_address(data.header.address);
        if !matches!(
            self.links.phase_for(&link_id),
            Some(LinkPhase::Active { .. }),
        ) {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        }
        let part: &[u8] = data.payload;
        let mut placed = None;
        for index in 0..self.incoming_resources.len() {
            if self.incoming_resources.link_at(index) != &link_id {
                continue;
            }
            let state = *self.incoming_resources.state(index);
            if state.status != IncomingResourceStatus::Transferring {
                continue;
            }
            let scan_from = state.consecutive_completed.map_or(0, |height| height + 1);
            let maybe_at = match_part_in_window(
                part,
                &state.salt_nonce,
                self.incoming_resources.names_flat(index),
                scan_from,
                state.window,
            );
            if let Some(at) = maybe_at {
                if self.incoming_resources.place_part(index, at, part) == PlacePartOutcome::Placed {
                    placed = Some(index);
                }
            }
        }
        let Some(index) = placed else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        };
        self.links.note_inbound(&link_id, arrived_at);
        let Some(LinkPhase::Active { rtt, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let link_rtt_ms = rtt.millis();

        {
            let state = self.incoming_resources.state_mut(index);
            state.received_byte_count = state.received_byte_count.saturating_add(part.len() as u64);
            if state.awaiting_round_first_response {
                absorb_round_first_response(state, part.len(), arrived_at, link_rtt_ms);
            }
            state.retries_left = PART_REQUEST_MAX_RETRIES;
            let state = *state;
            self.incoming_resources.set_timeout_at(
                index,
                Some(part_round_deadline(&state, link_rtt_ms, arrived_at)),
            );
        }
        self.advance_streamed_open(index);
        let hash = *self.incoming_resources.hash_at(index);
        let state = *self.incoming_resources.state(index);
        if state.received_part_count == state.part_count {
            return IngestPacketOutcome::OwesResourceAssembly { link_id, hash };
        }
        if state.outstanding_part_count == 0 && !state.waiting_for_hmu {
            absorb_completed_round(self.incoming_resources.state_mut(index), arrived_at);
            return IngestPacketOutcome::OwesResourcePull { link_id, hash };
        }
        IngestPacketOutcome::ResourceDeadlineAdvanced
    }

    /// Walk the [`StreamedOpen`] up to the consecutive frontier the placement just extended — or, when the chew is the pool's, only make sure it has begun: the runtime walks the chews through [`owed_open_span`](EngineState::owed_open_span) and its pool's verdicts.
    /// An intentional deviation in timing only: RNS 1.4.2 opens the joined transfer whole at assembly, we spread the same work under the part arrivals it was waiting on.
    fn advance_streamed_open(&mut self, index: usize) {
        let state = *self.incoming_resources.state(index);
        let Some(height) = state.consecutive_completed else {
            return;
        };
        let link_id = *self.incoming_resources.link_at(index);
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return;
        };
        let chews_here = match self.resource_open_lane {
            ResourceOpenLane::Inline => true,
            ResourceOpenLane::PoolWhenContended => !self.receiving_concurrently(),
        };
        let contiguous_byte_len = ((height + 1) * state.sdu).min(state.sealed_transfer_bytes);
        let (transfer, slot) = self
            .incoming_resources
            .transfer_and_streamed_open_mut(index);
        if matches!(slot, OpenProgress::NotBegun) {
            if contiguous_byte_len < 16 {
                return;
            }
            if let Some(open) = StreamedOpen::begin(key, transfer, state.compression) {
                *slot = OpenProgress::Parked(open);
            }
        }
        if chews_here {
            if let OpenProgress::Parked(open) = slot {
                open.advance(transfer, contiguous_byte_len);
            }
        }
    }

    /// The contention signal for [`ResourceOpenLane::PoolWhenContended`]: a second incoming
    /// transfer is in flight, so a worker's chew can overlap the manifold's ingest of the others.
    /// Row existence is the signal, deliberately not slot state: a fast wire lands a whole
    /// segment in one ingest burst, so concurrent transfers' begun-open phases serialize with
    /// the sweeps and rarely coexist even while the transfers themselves do.
    fn receiving_concurrently(&self) -> bool {
        self.incoming_resources.len() > 1
    }

    /// RNS 1.4.2's `Resource.hashmap_update_packet` with an intentional deviation: A segment that misfits the register cancels the transfer, where the reference would crash its link thread.
    pub(crate) fn ingest_resource_hashmap_update<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data.header.address,
            data.header.context,
            data.payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => {
                return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate)
            }
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::DecryptFailed);
        };
        let Ok(update) = parse_hashmap_update_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        let Some(index) = self.incoming_resources.lookup(&link_id, &update.hash) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        };
        self.links.note_inbound(&link_id, arrived_at);
        match self
            .incoming_resources
            .apply_hashmap_update(index, update.segment, update.hashmap)
        {
            Ok(_) => {
                self.incoming_resources.state_mut(index).retries_left = PART_REQUEST_MAX_RETRIES;
                IngestPacketOutcome::OwesResourcePull {
                    link_id,
                    hash: update.hash,
                }
            }
            Err(refusal) => {
                let settled_request = self.settle_response_claim(&link_id, &update.hash);
                self.retire_incoming_resource(&link_id, &update.hash);
                IngestPacketOutcome::IncomingResourceFailed {
                    link_id,
                    hash: update.hash,
                    cause: ResourceFailureCause::RefusedHashmapUpdate(refusal),
                    settled_request,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitResourcePullOutcome {
    NotTracked,
    /// Every part in the window is either placed or beyond the names received so far, and the register is not exhausted. The reference would send an empty request; we skip it (intentional deviation, strictly less wasted processing).
    NothingToRequest,
    LinkNotActive,
    Requested,
}

/// RNS 1.4.2 `Resource.update_eifr`. Never zero because the deadline arithmetic divides by it.
pub(super) fn expected_inflight_bits_per_second(
    state: &IncomingResourceState,
    link_rtt_ms: u64,
) -> u64 {
    let eifr = if state.data_bytes_per_second > 0 {
        state.data_bytes_per_second.saturating_mul(8)
    } else if let Some(inherited) = state.inherited_eifr {
        inherited
    } else {
        let rtt_millis = state.measured_rtt_ms.unwrap_or(link_rtt_ms).max(1);
        ESTABLISHMENT_COST_ESTIMATE_BYTES.saturating_mul(8_000) / rtt_millis
    };
    eifr.max(1)
}

/// RNS 1.4.2's watchdog TRANSFERRING arithmetic: an HMU allowance of x3.5 (as x7/2)  when waiting on names or idle.
///
/// Until a round has measured a rate, the wait covers three sdu of flight, the reference's unmeasured fallback.
fn part_round_deadline(
    state: &IncomingResourceState,
    link_rtt_ms: u64,
    now: InstantMillis,
) -> InstantMillis {
    let eifr = expected_inflight_bits_per_second(state, link_rtt_ms);
    let retries_used = (PART_REQUEST_MAX_RETRIES.saturating_sub(state.retries_left)) as u64;
    let extra_wait_ms = retries_used.saturating_mul(PER_RETRY_DELAY_MS);
    let sdu_bits = (state.sdu as u64).saturating_mul(8);
    let wait_ms = if state.request_response_bytes_per_second == 0 {
        state
            .part_timeout_factor
            .saturating_mul(sdu_bits.saturating_mul(3_000) / eifr)
    } else {
        let flight_bits = (state.outstanding_part_count as u64).saturating_mul(sdu_bits);
        let time_of_flight_ms = flight_bits.saturating_mul(1_000) / eifr;
        let hmu_wait_ms = if state.waiting_for_hmu || state.outstanding_part_count == 0 {
            sdu_bits.saturating_mul(7_000) / 2 / eifr
        } else {
            0
        };
        state
            .part_timeout_factor
            .saturating_mul(time_of_flight_ms)
            .saturating_add(hmu_wait_ms)
    };
    InstantMillis(
        now.0
            .saturating_add(wait_ms)
            .saturating_add(RETRY_GRACE_MS)
            .saturating_add(extra_wait_ms),
    )
}

/// RNS 1.4.2 `Resource.receive_part`'s first-response bookkeeping. The round's RTT lands (stepped, never adopted raw) and the request/response byte rate feeds the fast-link detector.
fn absorb_round_first_response(
    state: &mut IncomingResourceState,
    part_len: usize,
    arrived_at: InstantMillis,
    link_rtt_ms: u64,
) {
    state.awaiting_round_first_response = false;
    state.part_timeout_factor = PART_TIMEOUT_FACTOR_AFTER_RTT;
    let Some(sent_at) = state.request_sent_at else {
        return;
    };
    let round_trip_ms = arrived_at.0.saturating_sub(sent_at.0);
    state.measured_rtt_ms = Some(rtt_stepped_toward(
        state.measured_rtt_ms,
        round_trip_ms,
        link_rtt_ms,
    ));
    let round_cost = (part_len as u64).saturating_add(state.request_sent_bytes);
    if let Some(rate) = round_cost.saturating_mul(1_000).checked_div(round_trip_ms) {
        state.request_response_bytes_per_second = rate;
        note_fast_rate_round(state, rate);
    }
}

/// RNS 1.4.2's per-round RTT tracker: the measurement moves at most 5% toward each new sample, so one outlier round cannot yank the deadline arithmetic; the first sample adopts the link's own RTT.
fn rtt_stepped_toward(measured_ms: Option<u64>, sample_ms: u64, link_rtt_ms: u64) -> u64 {
    match measured_ms {
        None => link_rtt_ms,
        Some(rtt) if sample_ms < rtt => (rtt - rtt * 5 / 100).max(sample_ms),
        Some(rtt) if sample_ms > rtt => (rtt + rtt * 5 / 100).min(sample_ms),
        Some(rtt) => rtt,
    }
}

/// RNS 1.4.2 `Resource.receive_part` when a round comes home with nothing outstanding. The window grows, and the round's data rate feeds the fast and very-slow window-ceiling detectors.
fn absorb_completed_round(state: &mut IncomingResourceState, arrived_at: InstantMillis) {
    grow_window_after_full_round(state);
    let Some(sent_at) = state.request_sent_at else {
        return;
    };
    let elapsed_ms = arrived_at.0.saturating_sub(sent_at.0);
    let transferred = state
        .received_byte_count
        .saturating_sub(state.received_byte_count_at_request);
    let Some(rate) = transferred.saturating_mul(1_000).checked_div(elapsed_ms) else {
        return;
    };
    state.data_bytes_per_second = rate;
    note_fast_rate_round(state, rate);
    if state.fast_rate_rounds == 0
        && rate < RATE_VERY_SLOW_BYTES_PER_SECOND
        && state.very_slow_rate_rounds < VERY_SLOW_RATE_THRESHOLD
    {
        state.very_slow_rate_rounds += 1;
        if state.very_slow_rate_rounds == VERY_SLOW_RATE_THRESHOLD {
            state.window_max = WINDOW_MAX_VERY_SLOW;
        }
    }
}

/// RNS 1.4.2's fast-link detector: enough fast rounds lift the window ceiling to [`WINDOW_MAX`], once, permanently for this transfer.
fn note_fast_rate_round(state: &mut IncomingResourceState, rate: u64) {
    if rate > RATE_FAST_BYTES_PER_SECOND && state.fast_rate_rounds < FAST_RATE_THRESHOLD {
        state.fast_rate_rounds += 1;
        if state.fast_rate_rounds == FAST_RATE_THRESHOLD {
            state.window_max = WINDOW_MAX;
        }
    }
}

/// RNS 1.4.2 `Resource.receive_part`'s window growth. A fully-answered round widens the request window by one, and once the window outgrows the floor by the whole flexibility band, the floor steps up behind it.
fn grow_window_after_full_round(state: &mut IncomingResourceState) {
    if state.window < state.window_max {
        state.window += 1;
        if state.window - state.window_min > WINDOW_FLEXIBILITY - 1 {
            state.window_min += 1;
        }
    }
}

/// RNS 1.4.2's watchdog retry back-off, its three coupled steps verbatim:
/// - a silent round narrows the request window by one
/// - the ceiling follows it down, and
/// - while the ceiling still sits more than the flexibility band above the narrowed window, it drops once more.
///
/// Silence walks window and ceiling toward the floor together, so a recovering transfer re-earns its headroom round by round. This exactly mirrors [`grow_window_after_full_round`].
pub(super) fn shrink_window_after_silent_round(state: &mut IncomingResourceState) {
    if state.window > state.window_min {
        state.window -= 1;
        if state.window_max > state.window_min {
            state.window_max -= 1;
            if (state.window_max - state.window) > (WINDOW_FLEXIBILITY - 1) {
                state.window_max -= 1;
            }
        }
    }
}

#[cfg(test)]
mod loop_tests {
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::CommandId;
    use crate::engine::IngestIo;
    use crate::engine::Settlement;
    use crate::interfaces::AttachedInterfaces;
    use crate::interfaces::InterfaceId;
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::resources::advertisement::write_hashmap_update_plaintext;
    use crate::routing::links::resources::advertisement::ResourceAdvertisement;
    use crate::routing::links::resources::control::write_part_request_plaintext;
    use crate::routing::links::resources::receive::tests_support::*;
    use crate::routing::links::resources::SaltNonce;
    use crate::routing::links::resources::{
        ResourceBody, ResourceMetadata, ResourceSegment, ResourceSend,
    };
    use crate::wire::{PacketType as WirePacketType, WirePacketHeader, BROADCAST_MTU};

    fn eight_part_payload() -> std::vec::Vec<u8> {
        b"closing the resource loop one window at a time! ".repeat(75)
    }

    #[test]
    fn a_full_uncompressed_transfer_crosses_two_live_engines() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &data,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        assert_eq!(
            pull.frames.len(),
            1,
            "the receiver asks for the first window"
        );

        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(
            serve.frames.len(),
            4,
            "the sender streams every requested part"
        );

        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.received.is_empty() || !capture.frames.is_empty() {
                conclusion = Some(capture);
            }
        }
        let conclusion = conclusion.expect("the last part concludes the transfer");
        assert_eq!(conclusion.received.len(), 1);
        assert_eq!(
            conclusion.received[0].1, data,
            "the journaled plaintext is the original payload",
        );
        assert!(
            receiver.incoming_resources.is_empty(),
            "a delivered transfer retires its row",
        );
        assert_eq!(conclusion.frames.len(), 1, "and the proof goes back");

        let settled = feed(&mut sender, &conclusion.frames[0].1, 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
        assert!(sender.outgoing_resources.is_empty());
    }

    #[test]
    fn a_metadata_transfer_crosses_two_live_engines() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();
        let packed = bytes_from_hex(META_PACKED);

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(21),
                link_id: link_id(),
                body: ResourceBody {
                    data: &data,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::Packed(&packed),
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        assert_eq!(
            pull.frames.len(),
            1,
            "the receiver accepts the metadata advertisement and pulls",
        );

        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.received.is_empty() || !capture.frames.is_empty() {
                conclusion = Some(capture);
            }
        }
        let conclusion = conclusion.expect("the last part concludes the transfer");
        assert_eq!(
            conclusion.received[0].1, data,
            "the delivered data is the original payload, block stripped",
        );
        assert_eq!(
            conclusion.received_metadata[0].1, packed,
            "the packed metadata rides the delivery",
        );

        let settled = feed(&mut sender, &conclusion.frames[0].1, 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(21), Settlement::SendResource(Ok(()))),
        ));
    }

    #[test]
    fn a_two_segment_metadata_transfer_carries_the_block_on_segment_one_only() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let segment_one = four_part_payload();
        let segment_two = another_four_part_payload();
        let packed = bytes_from_hex(META_PACKED);
        let data_total = (segment_one.len() + segment_two.len()) as u64;
        let block_len = metadata_block(&packed).len() as u64;

        let adv_one = send_segment_carrying(
            &mut sender,
            CommandId(31),
            &segment_one,
            ResourceMetadata::Packed(&packed),
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: data_total,
            },
            1_000,
        )
        .expect("segment one advertises");
        with_advertisement(&adv_one, |adv| {
            assert!(adv.flags.has_metadata, "the flag travels on segment one");
            assert!(adv.flags.split);
            assert_eq!(
                adv.data_bytes,
                data_total + block_len,
                "d includes the block"
            );
        });
        let pull = feed(&mut receiver, &adv_one, 1_100);
        let serve = feed(&mut sender, &pull.frames[0].1, 1_200);
        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 1_300 + arrived as u64);
            if !capture.segments.is_empty() || !capture.frames.is_empty() {
                conclusion = Some(capture);
            }
        }
        let concluded_one = conclusion.expect("segment one concludes");
        assert_eq!(concluded_one.segments[0].1, 1);
        assert_eq!(
            concluded_one.segments[0].2, segment_one,
            "segment one's data arrives block-stripped",
        );
        assert_eq!(
            concluded_one.segment_metadata[0].2, packed,
            "and the packed block rides the segment event",
        );
        feed(&mut sender, &concluded_one.frames[0].1, 1_900);

        let adv_two = send_segment_carrying(
            &mut sender,
            CommandId(32),
            &segment_two,
            ResourceMetadata::SentInFirstSegment {
                packed_len: packed.len() as u32,
            },
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: data_total,
            },
            2_000,
        )
        .expect("segment two advertises");
        with_advertisement(&adv_two, |adv| {
            assert!(
                adv.flags.has_metadata,
                "the flag still travels on segment two",
            );
            assert_eq!(
                adv.data_bytes,
                data_total + block_len,
                "and d still includes the block",
            );
        });
        let pull = feed(&mut receiver, &adv_two, 2_100);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_200);
        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_300 + arrived as u64);
            if !capture.segments.is_empty() || !capture.frames.is_empty() {
                conclusion = Some(capture);
            }
        }
        let concluded_two = conclusion.expect("segment two concludes");
        assert_eq!(
            concluded_two.segments[0].2, segment_two,
            "no block to strip past segment one",
        );
        assert!(concluded_two.segment_metadata.is_empty());
        assert_eq!(
            concluded_two.assembled[0].1,
            block_len + data_total,
            "the assembly total counts the whole stream, block included",
        );
    }

    #[test]
    fn a_misplaced_metadata_block_settles_rejected() {
        use crate::engine::{Journaled, SendResourceFailure, SendResourceRejection};
        let mut sender = engine_with_active_link();
        let packed = bytes_from_hex(META_PACKED);
        let data = four_part_payload();
        let cases: [(ResourceMetadata<'_>, u64); 2] = [
            (ResourceMetadata::Packed(&packed), 2),
            (
                ResourceMetadata::SentInFirstSegment {
                    packed_len: packed.len() as u32,
                },
                1,
            ),
        ];
        for (metadata, segment_index) in cases {
            let mut settlement = None;
            sender.ingest_send_resource_segment_into(
                &ResourceSend {
                    id: CommandId(40),
                    link_id: link_id(),
                    body: ResourceBody {
                        data: &data,
                        compressed_candidate: None,
                        metadata,
                    },
                    correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
                },
                ResourceSegment {
                    index: segment_index,
                    total_segments: 2,
                    total_data_bytes: (2 * data.len()) as u64,
                },
                InstantMillis(1_000),
                &mut |bytes: &mut [u8]| bytes.fill(0xA5),
                &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::CommandSettled {
                        settlement: settled,
                        ..
                    }) = reaction
                    {
                        settlement = Some(settled);
                    }
                },
            );
            assert_eq!(
                settlement,
                Some(Settlement::SendResource(Err(
                    SendResourceFailure::Rejected(SendResourceRejection::MetadataMisplaced)
                ))),
            );
            assert!(sender.outgoing_resources.is_empty(), "nothing tracks");
        }
    }

    fn another_four_part_payload() -> std::vec::Vec<u8> {
        b"every part of the second segment now!".repeat(41)
    }

    fn pump_one_segment<S: StorageLayout>(
        sender: &mut EngineState<S>,
        receiver: &mut EngineState<S>,
        command_id: CommandId,
        data: &[u8],
        segment: ResourceSegment,
        base_time: u64,
    ) -> (InboundCapture, InboundCapture) {
        let mut advertisement = None;
        sender.ingest_send_resource_segment_into(
            &ResourceSend {
                id: command_id,
                link_id: link_id(),
                body: ResourceBody {
                    data,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            segment,
            InstantMillis(base_time),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let pull = feed(receiver, &advertisement.unwrap(), base_time + 100);
        let serve = feed(sender, &pull.frames[0].1, base_time + 200);
        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(receiver, part, base_time + 300 + arrived as u64);
            if !capture.segments.is_empty() || !capture.frames.is_empty() {
                conclusion = Some(capture);
            }
        }
        let conclusion = conclusion.expect("the last part concludes the segment");
        let settle = feed(sender, &conclusion.frames[0].1, base_time + 900);
        (conclusion, settle)
    }

    #[test]
    fn a_two_segment_transfer_assembles_across_two_live_engines() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let segment_one = four_part_payload();
        let segment_two = another_four_part_payload();
        let total = (segment_one.len() + segment_two.len()) as u64;

        let (concluded_one, settled_one) = pump_one_segment(
            &mut sender,
            &mut receiver,
            CommandId(11),
            &segment_one,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: total,
            },
            2_000,
        );
        assert_eq!(concluded_one.segments.len(), 1);
        let original_hash = concluded_one.segments[0].0;
        assert_eq!(concluded_one.segments[0].1, 1, "the first segment's index");
        assert_eq!(concluded_one.segments[0].2, segment_one);
        assert!(
            concluded_one.assembled.is_empty(),
            "the assembly does not complete on the first segment",
        );
        assert!(matches!(
            settled_one.settlements[0],
            (CommandId(11), Settlement::SendResource(Ok(()))),
        ));
        assert!(
            sender.outgoing_resources.is_empty(),
            "segment one's slot retires on its proof",
        );
        assert!(
            sender
                .outgoing_assemblies
                .original_hash(&link_id())
                .is_some(),
            "but the send chain persists for segment two",
        );

        let (concluded_two, settled_two) = pump_one_segment(
            &mut sender,
            &mut receiver,
            CommandId(12),
            &segment_two,
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: total,
            },
            4_000,
        );
        assert_eq!(concluded_two.segments.len(), 1);
        assert_eq!(
            concluded_two.segments[0].0, original_hash,
            "every segment re-advertises the chain's original hash",
        );
        assert_eq!(concluded_two.segments[0].1, 2, "the second segment's index");
        assert_eq!(concluded_two.segments[0].2, segment_two);
        assert_eq!(
            concluded_two.assembled.len(),
            1,
            "the last segment completes the assembly",
        );
        assert_eq!(concluded_two.assembled[0].0, original_hash);
        assert_eq!(
            concluded_two.assembled[0].1,
            (segment_one.len() + segment_two.len()) as u64,
            "the assembly reports the running byte total",
        );
        assert!(matches!(
            settled_two.settlements[0],
            (CommandId(12), Settlement::SendResource(Ok(()))),
        ));
        assert!(sender.outgoing_resources.is_empty());
        assert!(
            sender
                .outgoing_assemblies
                .original_hash(&link_id())
                .is_none(),
            "the last segment's proof clears the send chain",
        );
        assert!(
            receiver
                .incoming_assemblies
                .original_hash(&link_id())
                .is_none(),
            "and the receiver's chain retires with the completed assembly",
        );
    }

    fn send_segment<S: StorageLayout>(
        sender: &mut EngineState<S>,
        command_id: CommandId,
        data: &[u8],
        segment_index: u64,
        total_segments: u64,
        total_data_bytes: u64,
        at: u64,
    ) -> std::vec::Vec<u8> {
        send_segment_carrying(
            sender,
            command_id,
            data,
            ResourceMetadata::None,
            ResourceSegment {
                index: segment_index,
                total_segments,
                total_data_bytes,
            },
            at,
        )
        .expect("the sender advertises the segment")
    }

    fn send_segment_carrying<S: StorageLayout>(
        sender: &mut EngineState<S>,
        command_id: CommandId,
        data: &[u8],
        metadata: ResourceMetadata<'_>,
        segment: ResourceSegment,
        at: u64,
    ) -> Option<std::vec::Vec<u8>> {
        let mut frame = None;
        sender.ingest_send_resource_segment_into(
            &ResourceSend {
                id: command_id,
                link_id: link_id(),
                body: ResourceBody {
                    data,
                    compressed_candidate: None,
                    metadata,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            segment,
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    frame = filled_frame(fill);
                }
            },
        );
        frame
    }

    fn with_advertisement(frame: &[u8], assert: impl FnOnce(&ResourceAdvertisement<'_>)) {
        let (_, payload) = WirePacketHeader::parse(frame).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        assert(&ResourceAdvertisement::parse(opened).unwrap());
    }

    #[test]
    fn a_single_shot_send_stays_one_unsplit_segment() {
        let mut sender = engine_with_active_link();
        let frame = advertise_from(&mut sender, &four_part_payload(), None);
        let own = *sender.outgoing_resources.hash_at(0).unwrap();
        let state = sender.outgoing_resources.state(0);
        assert_eq!(state.segment_index, 1);
        assert_eq!(state.total_segments, 1);
        assert_eq!(
            state.original_hash, own,
            "a whole resource is its own original"
        );
        assert!(
            sender
                .outgoing_assemblies
                .original_hash(&link_id())
                .is_none(),
            "a single-shot send opens no chain",
        );
        with_advertisement(&frame, |adv| {
            assert!(!adv.flags.split, "and it advertises unsplit");
            assert_eq!(adv.segment_index, 1);
            assert_eq!(adv.total_segments, 1);
            assert_eq!(adv.original_hash, own);
        });
    }

    #[test]
    fn segment_one_of_a_split_opens_the_chain_with_its_own_hash() {
        let mut sender = engine_with_active_link();
        let total = (3 * four_part_payload().len()) as u64;
        let frame = send_segment(
            &mut sender,
            CommandId(11),
            &four_part_payload(),
            1,
            3,
            total,
            1_500,
        );
        let own = *sender.outgoing_resources.hash_at(0).unwrap();
        let state = sender.outgoing_resources.state(0);
        assert_eq!(state.segment_index, 1);
        assert_eq!(state.total_segments, 3);
        assert_eq!(
            state.original_hash, own,
            "segment one's original is its own hash"
        );
        assert_eq!(
            sender.outgoing_assemblies.original_hash(&link_id()),
            Some(own),
            "and the chain remembers it for the segments to come",
        );
        with_advertisement(&frame, |adv| {
            assert!(adv.flags.split);
            assert_eq!(adv.segment_index, 1);
            assert_eq!(adv.total_segments, 3);
            assert_eq!(adv.original_hash, own);
            assert_eq!(
                adv.data_bytes, total,
                "RNS 1.4.2 parity: every segment advertises the original total, not its own size",
            );
        });
    }

    #[test]
    fn a_later_segment_advertises_the_chains_original_hash_not_its_own() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let total = (3 * four_part_payload().len()) as u64;
        pump_one_segment(
            &mut sender,
            &mut receiver,
            CommandId(11),
            &four_part_payload(),
            ResourceSegment {
                index: 1,
                total_segments: 3,
                total_data_bytes: total,
            },
            2_000,
        );
        let original = sender
            .outgoing_assemblies
            .original_hash(&link_id())
            .expect("the chain is open after segment one");

        let frame = send_segment(
            &mut sender,
            CommandId(12),
            &another_four_part_payload(),
            2,
            3,
            total,
            4_000,
        );
        let own = *sender.outgoing_resources.hash_at(0).unwrap();
        let state = sender.outgoing_resources.state(0);
        assert_eq!(
            state.original_hash, original,
            "segment two re-advertises the chain's original hash",
        );
        assert_ne!(state.original_hash, own, "which is its own hash no longer");
        with_advertisement(&frame, |adv| {
            assert_eq!(adv.original_hash, original);
            assert_eq!(adv.hash, own, "while its own hash names the segment itself");
            assert_eq!(adv.segment_index, 2);
            assert_eq!(adv.total_segments, 3);
            assert!(adv.flags.split);
            assert_eq!(
                adv.data_bytes, total,
                "and re-advertises the original total, not this segment's size",
            );
        });
    }

    #[test]
    fn tearing_down_a_link_clears_an_open_send_chain() {
        let mut sender = engine_with_active_link();
        send_segment(
            &mut sender,
            CommandId(11),
            &four_part_payload(),
            1,
            2,
            (2 * four_part_payload().len()) as u64,
            1_500,
        );
        assert!(
            sender
                .outgoing_assemblies
                .original_hash(&link_id())
                .is_some(),
            "the chain opens with segment one",
        );
        let mut buf = [0u8; BROADCAST_MTU];
        sender
            .write_owed_link_close(
                &link_id(),
                crate::engine::LinkClosedReason::LocallyClosed,
                &[0u8; 16],
                &mut buf,
                &mut |_| {},
            )
            .unwrap();
        assert!(
            sender
                .outgoing_assemblies
                .original_hash(&link_id())
                .is_none(),
            "and a link teardown clears it with the rest of the link state",
        );
    }

    #[test]
    fn a_split_segment_with_no_open_chain_settles_predecessor_failed() {
        let mut sender = engine_with_active_link();
        let frame = send_segment_carrying(
            &mut sender,
            CommandId(11),
            &four_part_payload(),
            ResourceMetadata::None,
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: (2 * four_part_payload().len()) as u64,
            },
            1_500,
        );
        assert!(
            frame.is_none(),
            "a continuation with no chain to join never advertises",
        );
        assert!(sender.outgoing_resources.is_empty());
    }

    #[test]
    fn a_link_packet_on_a_foreign_interface_is_dropped_and_surfaced() {
        let foreign = InterfaceId::new([0x11; 8]);
        let advertisement = advertisement_frame(&four_part_payload(), None);

        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let accepted = feed(&mut receiver, &advertisement, 2_000);
        assert!(accepted.mismatched.is_empty());
        assert_eq!(
            accepted.frames.len(),
            1,
            "the link's own interface earns the first pull"
        );
        assert!(!receiver.incoming_resources.is_empty());

        let mut guarded = engine_with_active_link();
        accept_everything(&mut guarded);
        let blocked = feed_on(&mut guarded, &advertisement, foreign, 2_000);
        assert_eq!(blocked.mismatched, std::vec![(lane(), foreign)]);
        assert!(
            blocked.frames.is_empty(),
            "no pull leaves for a foreign-interface packet",
        );
        assert!(
            guarded.incoming_resources.is_empty(),
            "and the transfer is never opened",
        );
    }

    #[test]
    fn a_drained_window_grows_and_pulls_the_next_slice() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = eight_part_payload();

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &data,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(serve.frames.len(), 4, "window four to start");

        let mut next_pull = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.frames.is_empty() {
                next_pull = Some(capture);
            }
        }
        let next_pull = next_pull.expect("the drained window re-pulls");

        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.window, 5, "an emptied window grows by one");
        assert_eq!(state.consecutive_completed, Some(3));
        assert!(
            matches!(
                receiver
                    .incoming_resources
                    .transfer_and_streamed_open(index)
                    .1,
                OpenProgress::Parked(_),
            ),
            "the placed parts opened under the frontier, not at the conclusion",
        );

        let (_, request) = &next_pull.frames[0];
        let (_, payload) = WirePacketHeader::parse(request).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let request =
            crate::routing::links::resources::control::parse_part_request_plaintext(opened)
                .unwrap();
        assert_eq!(
            request.requested,
            &receiver.incoming_resources.names_flat(index)[4 * MAP_HASH_LEN..8 * MAP_HASH_LEN],
            "the next pull asks for the remaining four parts",
        );
    }

    #[test]
    fn a_mid_window_part_recomputes_the_resource_lane() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = eight_part_payload();

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &data,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(serve.frames.len(), 4, "window four to start");

        let mut raw = serve.frames[0].1.clone();
        let delta = receiver.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[
                    crate::engine::test_support::routable_descriptor(lane()),
                ]),
                now: InstantMillis(2_200),
                fill_random: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |_| {},
            },
        );
        assert!(
            !receiver.incoming_resources.is_empty(),
            "the transfer is still in flight after a single mid-window part",
        );
        assert_ne!(
            delta.resource_deadlines,
            crate::engine::WakeSchedule::Unchanged,
            "a mid-window part must recompute the resource lane, not leave it untouched",
        );
        assert_eq!(
            delta.resource_deadlines,
            receiver.resource_deadlines_wake(),
            "the recomputed lane delta matches the freshly-set part-round deadline",
        );
    }

    #[test]
    fn a_full_window_accepts_its_far_edge_when_parts_reorder() {
        let mut sender = active_engine::<crate::storage::GrowableHeap>();
        let mut receiver = active_engine::<crate::storage::GrowableHeap>();
        accept_everything(&mut receiver);
        let data = b"out-of-order resource windows still owe the edge! ".repeat(140);

        let advertisement = advertise_from(&mut sender, &data, None);
        let first_pull = feed(&mut receiver, &advertisement, 2_000);
        let first_serve = feed(&mut sender, &first_pull.frames[0].1, 2_100);
        assert_eq!(first_serve.frames.len(), 4);

        let hash = *receiver.incoming_resources.hash_at(0);
        let mut next_pull = None;
        for (arrived, (_, part)) in first_serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.frames.is_empty() {
                next_pull = Some(capture);
            }
        }
        let next_pull = next_pull.expect("the first window drains");
        let next_serve = feed(&mut sender, &next_pull.frames[0].1, 2_300);
        assert_eq!(next_serve.frames.len(), 5, "the grown window is full");

        let (_, far_edge) = next_serve.frames.last().expect("far-edge part");
        let reordered = feed(&mut receiver, far_edge, 2_400);
        assert!(reordered.frames.is_empty());
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.consecutive_completed, Some(3));
        assert_eq!(state.received_part_count, 5);
        assert_eq!(state.outstanding_part_count, 4);
        assert!(
            receiver.incoming_resources.received_flags(index)[8],
            "the far edge of the requested window lands even before 4..7",
        );

        let mut after_gap = None;
        for (arrived, (_, part)) in next_serve.frames[..4].iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_500 + arrived as u64);
            if !capture.frames.is_empty() {
                after_gap = Some(capture);
            }
        }
        let after_gap = after_gap.expect("filling the gap drains the request");
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        assert_eq!(
            receiver
                .incoming_resources
                .state(index)
                .consecutive_completed,
            Some(8),
        );
        assert_eq!(after_gap.frames.len(), 1, "the next pull goes out promptly");
    }

    fn crafted_partial_advertisement(names: &[u8], part_count: usize, iv: u8) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::advertisement::{
            ResourceAdvertisement, ResourceFlags,
        };
        let advertisement = ResourceAdvertisement {
            transfer_bytes: (part_count * 464) as u64,
            data_bytes: 2_700,
            part_count: part_count as u64,
            hash: ResourceHash::new([0xAB; 32]),
            salt_nonce: SaltNonce::new([0x61; 4]),
            original_hash: ResourceHash::new([0xAB; 32]),
            segment_index: 1,
            total_segments: 1,
            request_id: None,
            flags: ResourceFlags {
                encrypted: true,
                compressed: false,
                split: false,
                is_request: false,
                is_response: false,
                has_metadata: false,
            },
            hashmap: names,
        };
        let mut plaintext = [0u8; 431];
        let plaintext_len = advertisement.write(&mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[iv; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_bytes].to_vec()
    }

    fn ingest<S: StorageLayout>(
        receiver: &mut EngineState<S>,
        frame: &[u8],
        at: u64,
    ) -> IngestPacketOutcome<'static> {
        let mut raw = frame.to_vec();
        let (header, tail) = WirePacketHeader::parse(&raw).unwrap();
        let payload_start = raw.len() - tail.len();
        match receiver.ingest_resource_advertisement(
            DataPacket {
                header,
                payload: &mut raw[payload_start..],
            },
            InstantMillis(at),
        ) {
            IngestPacketOutcome::OwesResourcePull { link_id, hash } => {
                IngestPacketOutcome::OwesResourcePull { link_id, hash }
            }
            IngestPacketOutcome::Ignored(reason) => IngestPacketOutcome::Ignored(reason),
            other => panic!("unexpected advertisement outcome: {other:?}"),
        }
    }

    fn sealed_hashmap_update(segment: u64, names: &[u8], iv: u8) -> std::vec::Vec<u8> {
        let mut plaintext = [0u8; 431];
        let plaintext_len = write_hashmap_update_plaintext(
            &ResourceHash::new([0xAB; 32]),
            segment,
            names,
            &mut plaintext,
        )
        .unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceHashUpdate,
            &plaintext[..plaintext_len],
            &[iv; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_bytes].to_vec()
    }

    fn six_names() -> std::vec::Vec<u8> {
        let mut names = std::vec::Vec::new();
        for i in 1u32..=6 {
            names.extend_from_slice(&i.to_be_bytes());
        }
        names
    }

    #[test]
    fn a_reencrypted_readvertisement_refreshes_the_active_pull() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let names = six_names();
        feed(
            &mut receiver,
            &crafted_partial_advertisement(&names, 6, 0xD1),
            2_000,
        );
        assert_eq!(receiver.incoming_resources.len(), 1);

        let re_encrypted_retry = crafted_partial_advertisement(&names, 6, 0xD2);
        assert_eq!(
            ingest(&mut receiver, &re_encrypted_retry, 2_100),
            IngestPacketOutcome::OwesResourcePull {
                link_id: link_id(),
                hash: ResourceHash::new([0xAB; 32]),
            },
        );
        assert_eq!(receiver.incoming_resources.len(), 1);
    }

    #[test]
    fn hashmap_names_past_the_part_count_are_malformed_not_exhausted_capacity() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);

        let six_names_for_four_parts = crafted_partial_advertisement(&six_names(), 4, 0xD1);
        assert_eq!(
            ingest(&mut receiver, &six_names_for_four_parts, 2_000),
            IngestPacketOutcome::Ignored(IgnoreReason::Malformed),
        );
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn an_exhausted_pull_resumes_when_the_hashmap_update_lands() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);

        let names = six_names();
        let pull = feed(
            &mut receiver,
            &crafted_partial_advertisement(&names[..8], 6, 0xD1),
            2_000,
        );
        assert_eq!(pull.frames.len(), 1);
        let (_, payload) = WirePacketHeader::parse(&pull.frames[0].1).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let request =
            crate::routing::links::resources::control::parse_part_request_plaintext(opened)
                .unwrap();
        assert_eq!(
            request.requested,
            &names[..8],
            "only two parts are nameable"
        );
        assert_eq!(
            request.last_known_map_hash,
            Some(names[4..8].try_into().unwrap()),
            "the request flags exhaustion at the last known name",
        );

        let resumed = feed(
            &mut receiver,
            &sealed_hashmap_update(0, &names, 0xD2),
            2_100,
        );
        assert_eq!(resumed.frames.len(), 1, "the pull resumes");
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &ResourceHash::new([0xAB; 32]))
            .unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.hashmap_height, 6);
        assert!(!state.waiting_for_hmu);
    }

    #[test]
    fn a_hashmap_update_refills_the_retry_budget_like_the_reference() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let names = six_names();
        feed(
            &mut receiver,
            &crafted_partial_advertisement(&names[..8], 6, 0xD1),
            2_000,
        );
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &ResourceHash::new([0xAB; 32]))
            .unwrap();
        receiver.incoming_resources.state_mut(index).retries_left = 3;

        feed(
            &mut receiver,
            &sealed_hashmap_update(0, &names, 0xD2),
            2_100,
        );
        assert_eq!(
            receiver.incoming_resources.state(index).retries_left,
            PART_REQUEST_MAX_RETRIES,
            "new names refill the budget, like the reference's hashmap_update",
        );
    }

    #[test]
    fn a_misfit_hashmap_update_cancels_the_transfer() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let names = six_names();
        feed(
            &mut receiver,
            &crafted_partial_advertisement(&names[..8], 6, 0xD1),
            2_000,
        );

        let cancelled = feed(
            &mut receiver,
            &sealed_hashmap_update(5, &names, 0xD3),
            2_100,
        );
        assert!(cancelled.frames.is_empty());
        assert_eq!(
            cancelled.failed,
            [(
                ResourceHash::new([0xAB; 32]),
                ResourceFailureCause::RefusedHashmapUpdate(
                    crate::routing::links::resources::table::ApplyHashmapUpdateError::BeyondPartCount,
                ),
            )],
        );
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn a_transfer_advertised_under_a_false_hash_fails_and_never_proves() {
        use crate::routing::links::resources::advertisement::ResourceAdvertisement;

        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &data,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let advertisement = advertisement.unwrap();
        let (_, payload) = WirePacketHeader::parse(&advertisement).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let genuine = ResourceAdvertisement::parse(opened).unwrap();

        let mut lying = genuine;
        let mut wrong = *genuine.hash.as_bytes();
        wrong[0] ^= 1;
        lying.hash = ResourceHash::new(wrong);
        let mut plaintext = [0u8; 431];
        let plaintext_len = lying.write(&mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[0xD4; 16],
            &mut frame,
        )
        .unwrap();
        feed(&mut receiver, &frame[..wire_bytes], 2_000);

        let mut request_plaintext = [0u8; 337];
        let request_len = write_part_request_plaintext(
            &genuine.hash,
            None,
            genuine.hashmap,
            &mut request_plaintext,
        )
        .unwrap();
        let mut request_frame = [0u8; BROADCAST_MTU];
        let request_wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceRequest,
            &request_plaintext[..request_len],
            &[0xD5; 16],
            &mut request_frame,
        )
        .unwrap();
        let serve = feed(&mut sender, &request_frame[..request_wire_len], 2_100);
        assert_eq!(serve.frames.len(), 4);

        let mut outcome = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.failed.is_empty() || !capture.frames.is_empty() {
                outcome = Some(capture);
            }
        }
        let outcome = outcome.expect("the last part concludes");
        assert!(outcome.frames.is_empty(), "no proof for a corrupt transfer");
        assert!(outcome.received.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].1, ResourceFailureCause::TransferCorrupt);
        assert!(receiver.incoming_resources.is_empty());

        let _ = WirePacketType::Proof;
    }
}

#[cfg(test)]
mod dynamics_tests {
    use super::*;
    use crate::routing::links::resources::receive::tests_support::*;
    use crate::routing::links::resources::table::IncomingResourceStatus;
    use crate::routing::links::resources::{WINDOW_MAX, WINDOW_MAX_SLOW, WINDOW_MAX_VERY_SLOW};
    use crate::storage::GrowableHeap;

    struct RoundOutcome {
        concluded: bool,
    }

    fn run_rounds(
        round_trip_ms: u64,
        rounds: usize,
        data: &[u8],
    ) -> (
        crate::engine::EngineState<GrowableHeap>,
        ResourceHash,
        RoundOutcome,
    ) {
        let mut sender = active_engine::<GrowableHeap>();
        let mut receiver = active_engine::<GrowableHeap>();
        accept_everything(&mut receiver);
        let advertisement = advertise_from(&mut sender, data, None);

        let mut now = 2_000u64;
        let mut pull = feed(&mut receiver, &advertisement, now);
        let hash = *receiver.incoming_resources.hash_at(0);
        let mut concluded = false;
        for _ in 0..rounds {
            let Some((_, request)) = pull.frames.first() else {
                break;
            };
            let serve = feed(&mut sender, request, now + 10);
            now += round_trip_ms;
            let mut next = InboundCapture {
                frames: std::vec::Vec::new(),
                settlements: std::vec::Vec::new(),
                received: std::vec::Vec::new(),
                received_metadata: std::vec::Vec::new(),
                segment_metadata: std::vec::Vec::new(),
                failed: std::vec::Vec::new(),
                segments: std::vec::Vec::new(),
                response_segments: std::vec::Vec::new(),
                assembled: std::vec::Vec::new(),
                mismatched: std::vec::Vec::new(),
                requests: std::vec::Vec::new(),
            };
            for (_, part) in &serve.frames {
                let capture = feed(&mut receiver, part, now);
                if !capture.frames.is_empty() || !capture.received.is_empty() {
                    next = capture;
                }
            }
            if !next.received.is_empty() {
                concluded = true;
                break;
            }
            pull = next;
        }
        (receiver, hash, RoundOutcome { concluded })
    }

    fn twenty_four_part_payload() -> std::vec::Vec<u8> {
        b"rate dynamics earn the window its ceiling!! ".repeat(248)
    }

    #[test]
    fn four_fast_rounds_lift_the_window_ceiling() {
        let data = twenty_four_part_payload();
        let (receiver, hash, outcome) = run_rounds(50, 4, &data);
        assert!(!outcome.concluded, "four rounds leave parts outstanding");
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.fast_rate_rounds, 4);
        assert_eq!(
            state.window_max, WINDOW_MAX,
            "fifty-millisecond windows of whole parts run far past RATE_FAST",
        );
        assert_eq!(
            state.part_timeout_factor, 2,
            "a measured round trip tightens the timeout factor",
        );
        assert_eq!(
            state.measured_rtt_ms,
            Some(216),
            "the first measurement adopts the link rtt (250), then eases five percent \
             toward the real round trip each round: 250, 238, 227, 216",
        );
    }

    #[test]
    fn two_very_slow_rounds_drop_the_window_ceiling() {
        let data = twenty_four_part_payload();
        let (receiver, hash, _) = run_rounds(60_000, 2, &data);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.fast_rate_rounds, 0);
        assert_eq!(state.very_slow_rate_rounds, 2);
        assert_eq!(state.window_max, WINDOW_MAX_VERY_SLOW);
    }

    #[test]
    fn a_concluded_transfer_leaves_the_link_its_window_and_rate() {
        let data = b"inheritance crosses transfers on one link! ".repeat(80);
        let (mut receiver, _, outcome) = run_rounds(50, 8, &data);
        assert!(
            outcome.concluded,
            "an eight-part transfer concludes within the budget"
        );
        assert!(receiver.incoming_resources.is_empty());

        let mut second_sender = active_engine::<GrowableHeap>();
        let advertisement = advertise_from(&mut second_sender, &twenty_four_part_payload(), None);
        feed(&mut receiver, &advertisement, 90_000);
        let index = 0;
        let state = receiver.incoming_resources.state(index);
        assert_eq!(
            state.window, 5,
            "the inherited window starts where the last transfer ended — \
             grown once when its first round drained",
        );
        assert!(
            state.inherited_eifr.is_some_and(|eifr| eifr > 0),
            "the inherited rate seeds the first deadline",
        );
        assert_eq!(state.window_max, WINDOW_MAX_SLOW);
        assert_eq!(state.status, IncomingResourceStatus::Transferring);
    }
}
