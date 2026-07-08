//! RNS 1.3.5 `Resource.accept` plus the receiver's half of the link dispatch.
//! The strategy gate runs before a single part moves: the advertisement declares size and kind up front, so refusing is free.

use crate::engine::Journaled;
use crate::engine::{
    CommandId, CommandOutcome, PacketReceiptDelivered, SetResourceStrategy,
    SetResourceStrategyRejection, Settlement,
};
use crate::engine::{Directive, EngineReaction, EngineState, InstantMillis};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::delivery::receipts::{ReceiptColumns, Receipts};
use crate::routing::ingress::{DataPacket, IgnoreReason, IngestPacketOutcome};
use crate::routing::links::data::write_link_packet;
use crate::routing::links::data::write_link_raw_packet;
use crate::routing::links::data::{link_data_frame_ceiling, link_raw_frame_ceiling, LINK_MDU};
use crate::routing::links::request::{parse_request_plaintext, RequestId};
use crate::routing::links::resources::advertisement::{
    parse_hashmap_update_plaintext, ResourceAdvertisement,
};
use crate::routing::links::resources::assemble_incoming::{
    match_part_in_window, open_transfer, verify_and_prove,
};
use crate::routing::links::resources::assembly::{AssemblyProgress, SegmentFit};
use crate::routing::links::resources::control::{
    parse_cancel_plaintext, write_part_request_plaintext, write_proof_plaintext,
    PROOF_PLAINTEXT_LEN,
};
use crate::routing::links::resources::table::IncomingResourceState;
use crate::routing::links::resources::table::{AcceptedResource, IncomingResourceStatus};
use crate::routing::links::resources::{
    resource_sdu, ResourceCompression, ResourceCorrelation, ResourceHash, ResourceStrategy,
    ESTABLISHMENT_COST_ESTIMATE_BYTES, FAST_RATE_THRESHOLD, MAP_HASH_LEN, MAX_EFFICIENT_SIZE,
    PART_REQUEST_MAX_RETRIES, PART_TIMEOUT_FACTOR_AFTER_RTT, PER_RETRY_DELAY_MS,
    RATE_FAST_BYTES_PER_SECOND, RATE_VERY_SLOW_BYTES_PER_SECOND, RETRY_GRACE_MS,
    VERY_SLOW_RATE_THRESHOLD, WINDOW_FLEXIBILITY, WINDOW_MAX, WINDOW_MAX_VERY_SLOW,
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::{DestinationType, PacketType, WireContext};

impl<S: StorageLayout> EngineState<S> {
    pub(crate) fn ingest_set_resource_strategy(
        &mut self,
        id: CommandId,
        set: SetResourceStrategy,
    ) -> CommandOutcome {
        use crate::routing::links::table::LinkActivationError;
        match self.links.set_resource_strategy(&set.link_id, set.strategy) {
            Ok(()) => CommandOutcome::ResourceStrategySet { id },
            Err(LinkActivationError::UnknownLink) => CommandOutcome::SetResourceStrategyRejected {
                id,
                rejection: SetResourceStrategyRejection::NoSuchLink,
            },
            Err(LinkActivationError::WrongPhase) => CommandOutcome::SetResourceStrategyRejected {
                id,
                rejection: SetResourceStrategyRejection::LinkNotActive,
            },
        }
    }

    /// RNS 1.3.5 `Resource.accept`; refusals are silent, like a reference receiver that never accepts.
    /// Request-correlated and pending-response advertisements bypass the strategy, exactly the  reference's `Link.receive` `RESOURCE_ADV` ladder (its strategy arms only ever see unsolicited resources).
    /// Still-deferred shapes are refused here: metadata, compressed splits.
    /// Advertisements stay behind the duplicate filter (only `RESOURCE_REQ`/`RESOURCE`/`RESOURCE_PRF` are exempt in the reference).
    pub(crate) fn classify_resource_advertisement<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(LinkPhase::Active {
            key,
            mtu,
            resource_strategy,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let resource_strategy = *resource_strategy;
        let mtu = *mtu;
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
        let Ok(advertisement) = ResourceAdvertisement::parse(plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        if !advertisement.flags.encrypted
            || advertisement.flags.has_metadata
            || advertisement.hashmap.is_empty()
            || advertisement.total_segments == 0
            || advertisement.segment_index == 0
            || advertisement.segment_index > advertisement.total_segments
            || advertisement.flags.split != (advertisement.total_segments > 1)
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        let correlation = match (
            advertisement.flags.is_request,
            advertisement.flags.is_response,
            advertisement.request_id,
        ) {
            (true, false, Some(id)) => ResourceCorrelation::Request(id),
            (false, true, Some(id)) => ResourceCorrelation::Response(id),
            _ => ResourceCorrelation::Unsolicited,
        };
        let bypasses_strategy = advertisement.total_segments == 1
            && match correlation {
                ResourceCorrelation::Response(id) => {
                    self.receipts.has_pending_request(id.as_bytes())
                }
                ResourceCorrelation::Request(_) => true,
                ResourceCorrelation::Unsolicited => false,
            };
        let (max_uncompressed_len, accept_compressed) = if bypasses_strategy {
            (MAX_EFFICIENT_SIZE as u64, true)
        } else {
            match resource_strategy {
                ResourceStrategy::Accept {
                    max_uncompressed_len,
                    accept_compressed,
                } => (max_uncompressed_len, accept_compressed),
                ResourceStrategy::AcceptNone => {
                    return IngestPacketOutcome::Ignored(IgnoreReason::NotForUs)
                }
            }
        };
        let compression = ResourceCompression::from_wire_flag(advertisement.flags.compressed);
        if compression == ResourceCompression::Bz2 && !accept_compressed {
            return IngestPacketOutcome::Ignored(IgnoreReason::NotForUs);
        }
        let multi_segment = advertisement.total_segments > 1;
        if multi_segment && compression == ResourceCompression::Bz2 {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        if multi_segment
            && advertisement.segment_index > 1
            && self.incoming_assemblies.fit(
                &link_id,
                &advertisement.original_hash,
                advertisement.segment_index,
            ) == SegmentFit::Unexpected
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        if advertisement.data_size > max_uncompressed_len {
            return IngestPacketOutcome::Ignored(IgnoreReason::CapacityExhausted);
        }
        let Ok(sealed_transfer_len) = usize::try_from(advertisement.transfer_size) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        let sdu = resource_sdu(mtu);
        let part_count = sealed_transfer_len.div_ceil(sdu);
        if part_count == 0 {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        let accepted = AcceptedResource {
            hash: advertisement.hash,
            salt_nonce: advertisement.salt_nonce,
            compression,
            uncompressed_data_len: advertisement.data_size,
            segment_index: advertisement.segment_index,
            total_segments: advertisement.total_segments,
            sealed_transfer_len,
            part_count,
            sdu,
            correlation,
            initial_names: advertisement.hashmap,
        };
        let inherited = match self.links.phase_for(&link_id) {
            Some(LinkPhase::Active {
                last_resource_window,
                last_resource_eifr,
                ..
            }) => (*last_resource_window, *last_resource_eifr),
            _ => (None, None),
        };
        let Ok(index) = self.incoming_resources.accept(link_id, accepted) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::CapacityExhausted);
        };
        {
            let state = self.incoming_resources.state_mut(index);
            state.retries_left = PART_REQUEST_MAX_RETRIES;
            if let Some(window) = inherited.0 {
                state.window = window;
            }
            state.inherited_eifr = inherited.1;
        }
        if multi_segment && advertisement.segment_index == 1 {
            self.incoming_assemblies.begin(
                link_id,
                advertisement.original_hash,
                advertisement.total_segments,
            );
        }
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::OwesResourcePull {
            link_id,
            hash: advertisement.hash,
        }
    }

    /// RNS 1.3.5 `Resource.request_next`; the request flags hashmap-exhausted,
    /// carrying the last known name, when the window runs past the names received.
    pub(crate) fn emit_resource_pull<F>(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let Some(index) = self.incoming_resources.lookup(link_id, hash) else {
            return;
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
            return;
        }
        let names = self.incoming_resources.names_flat(index);
        let last = state.hashmap_height.saturating_sub(1);
        let Ok(last_known) =
            <[u8; MAP_HASH_LEN]>::try_from(&names[last * MAP_HASH_LEN..(last + 1) * MAP_HASH_LEN])
        else {
            return;
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
            return;
        };
        let mtu = *mtu;
        let fire_on = *attached_interface;
        let rtt_ms = rtt.millis();
        let mut iv = [0u8; 16];
        fill_entropy(&mut iv);
        let mut request_wire_len = 0u64;
        let mut fill = |slot: &mut [u8]| -> Option<usize> {
            let mut plaintext = [0u8; 1 + MAP_HASH_LEN + 32 + WINDOW_MAX * MAP_HASH_LEN];
            let plaintext_len = write_part_request_plaintext(
                hash,
                exhausted.then_some(&last_known),
                &requested[..requested_count * MAP_HASH_LEN],
                &mut plaintext,
            )
            .ok()?;
            let wire_len = write_link_packet(
                link_id,
                key,
                mtu,
                WireContext::ResourceRequest,
                &plaintext[..plaintext_len],
                &iv,
                slot,
            )
            .ok()?;
            request_wire_len = wire_len as u64;
            Some(wire_len)
        };
        sink(EngineReaction::Directive(Directive::EmitFrame {
            target: fire_on,
            size_hint: link_data_frame_ceiling(LINK_MDU),
            fill: &mut fill,
        }));
        {
            let state = self.incoming_resources.state_mut(index);
            state.request_sent_at = Some(now);
            state.request_sent_byte_len = request_wire_len;
            state.received_byte_count_at_request = state.received_byte_count;
            state.awaiting_round_first_response = true;
        }
        let state = *self.incoming_resources.state(index);
        self.incoming_resources
            .set_timeout_at(index, Some(part_round_deadline(&state, rtt_ms, now)));
    }
}

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.3.5's link dispatch for context `RESOURCE`: a part names no transfer and
    /// carries no index, so every incoming transfer tries to place it by its salted
    /// name; exempt from duplicate filtering (a resent part is byte-identical). We
    /// count the part's payload plus the request's frame where the reference counts
    /// both whole frames: nineteen header bytes that never move a kilobyte-scale
    /// threshold.
    pub(crate) fn classify_resource_part<'p>(
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
            let at = match_part_in_window(
                part,
                &state.salt_nonce,
                self.incoming_resources.names_flat(index),
                scan_from,
                state.window,
            );
            if let Some(at) = at {
                if self.incoming_resources.place_part(index, at, part) {
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
                state.awaiting_round_first_response = false;
                state.part_timeout_factor = PART_TIMEOUT_FACTOR_AFTER_RTT;
                if let Some(sent_at) = state.request_sent_at {
                    let round_trip_ms = arrived_at.0.saturating_sub(sent_at.0);
                    state.measured_rtt_ms = Some(match state.measured_rtt_ms {
                        None => link_rtt_ms,
                        Some(rtt) if round_trip_ms < rtt => {
                            (rtt - rtt * 5 / 100).max(round_trip_ms)
                        }
                        Some(rtt) if round_trip_ms > rtt => {
                            (rtt + rtt * 5 / 100).min(round_trip_ms)
                        }
                        Some(rtt) => rtt,
                    });
                    let round_cost =
                        (part.len() as u64).saturating_add(state.request_sent_byte_len);
                    if let Some(rate) = round_cost.saturating_mul(1_000).checked_div(round_trip_ms)
                    {
                        state.request_response_byte_rate = rate;
                        if state.request_response_byte_rate > RATE_FAST_BYTES_PER_SECOND
                            && state.fast_rate_rounds < FAST_RATE_THRESHOLD
                        {
                            state.fast_rate_rounds += 1;
                            if state.fast_rate_rounds == FAST_RATE_THRESHOLD {
                                state.window_max = WINDOW_MAX;
                            }
                        }
                    }
                }
            }
            state.retries_left = PART_REQUEST_MAX_RETRIES;
            let state = *state;
            self.incoming_resources.set_timeout_at(
                index,
                Some(part_round_deadline(&state, link_rtt_ms, arrived_at)),
            );
        }
        let hash = *self.incoming_resources.hash_at(index);
        let state = *self.incoming_resources.state(index);
        if state.received_part_count == state.part_count {
            return IngestPacketOutcome::OwesResourceAssembly { link_id, hash };
        }
        if state.outstanding_part_count == 0 && !state.waiting_for_hmu {
            let state = self.incoming_resources.state_mut(index);
            if state.window < state.window_max {
                state.window += 1;
                if state.window - state.window_min > WINDOW_FLEXIBILITY - 1 {
                    state.window_min += 1;
                }
            }
            if let Some(sent_at) = state.request_sent_at {
                let elapsed_ms = arrived_at.0.saturating_sub(sent_at.0);
                let transferred = state
                    .received_byte_count
                    .saturating_sub(state.received_byte_count_at_request);
                if let Some(rate) = transferred.saturating_mul(1_000).checked_div(elapsed_ms) {
                    state.data_byte_rate = rate;
                    if state.data_byte_rate > RATE_FAST_BYTES_PER_SECOND
                        && state.fast_rate_rounds < FAST_RATE_THRESHOLD
                    {
                        state.fast_rate_rounds += 1;
                        if state.fast_rate_rounds == FAST_RATE_THRESHOLD {
                            state.window_max = WINDOW_MAX;
                        }
                    }
                    if state.fast_rate_rounds == 0
                        && state.data_byte_rate < RATE_VERY_SLOW_BYTES_PER_SECOND
                        && state.very_slow_rate_rounds < VERY_SLOW_RATE_THRESHOLD
                    {
                        state.very_slow_rate_rounds += 1;
                        if state.very_slow_rate_rounds == VERY_SLOW_RATE_THRESHOLD {
                            state.window_max = WINDOW_MAX_VERY_SLOW;
                        }
                    }
                }
            }
            return IngestPacketOutcome::OwesResourcePull { link_id, hash };
        }
        IngestPacketOutcome::ResourceDeadlineAdvanced
    }

    /// RNS 1.3.5 `Resource.hashmap_update_packet`. A segment that misfits the register
    /// cancels the transfer where the reference would crash its link thread.
    pub(crate) fn classify_resource_hashmap_update<'p>(
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
            Err(_) => {
                self.retire_incoming_resource(&link_id, &update.hash);
                IngestPacketOutcome::ResourceConcludedFailed {
                    link_id,
                    hash: update.hash,
                }
            }
        }
    }

    /// RNS 1.3.5's link dispatch for `RESOURCE_ICL`: sealed, and behind the duplicate
    /// filter like the advertisement.
    pub(crate) fn classify_resource_cancel<'p>(
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
        let Ok(hash) = parse_cancel_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        if self.incoming_resources.lookup(&link_id, &hash).is_none() {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        }
        self.retire_incoming_resource(&link_id, &hash);
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::ResourceConcludedFailed { link_id, hash }
    }

    /// RNS 1.3.5 `Resource.assemble` + `prove`: verify the salted hash, send the
    /// 64-byte proof back raw. A compressed transfer stops at AwaitingDecompression;
    /// the host owns the inflate.
    pub(crate) fn conclude_resource(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let Some(index) = self.incoming_resources.lookup(link_id, hash) else {
            return;
        };
        let state = *self.incoming_resources.state(index);
        let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            rtt,
            ..
        }) = self.links.phase_for(link_id)
        else {
            return;
        };
        let mtu = *mtu;
        let fire_on = *attached_interface;
        let link_rtt = *rtt;

        if state.compression == ResourceCompression::Bz2 {
            let mut opened = false;
            {
                let transfer = self.incoming_resources.sealed_transfer_mut(index);
                if let Ok(stream) = open_transfer(key, transfer) {
                    sink(EngineReaction::Journaled(
                        Journaled::ResourceNeedsDecompression {
                            link_id: *link_id,
                            hash: *hash,
                            stream,
                            uncompressed_data_len: state.uncompressed_data_len,
                        },
                    ));
                    opened = true;
                }
            }
            if opened {
                self.incoming_resources.state_mut(index).status =
                    IncomingResourceStatus::AwaitingDecompression;
                self.incoming_resources.set_timeout_at(index, None);
            } else {
                self.retire_incoming_resource(link_id, hash);
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id: *link_id,
                    hash: *hash,
                }));
            }
            return;
        }

        let multi_segment = state.total_segments > 1;
        let original_hash = self
            .incoming_assemblies
            .original_hash(link_id)
            .unwrap_or(*hash);
        let mut delivered_segment_bytes = None;
        {
            let transfer = self.incoming_resources.sealed_transfer_mut(index);
            if let Ok(plaintext) = open_transfer(key, transfer) {
                if let Ok(proof) = verify_and_prove(plaintext, &state.salt_nonce, hash) {
                    if let Some(prove) = proof_emission(link_id, hash, &proof, mtu) {
                        emit_proof(prove, fire_on, sink);
                        if multi_segment {
                            sink(EngineReaction::Journaled(
                                Journaled::ResourceSegmentReceived {
                                    link_id: *link_id,
                                    original_hash,
                                    segment_index: state.segment_index,
                                    total_segments: state.total_segments,
                                    data: plaintext,
                                },
                            ));
                        } else {
                            deliver_single_segment(
                                &mut self.receipts,
                                AssembledSingleSegment {
                                    link_id,
                                    hash,
                                    correlation: state.correlation,
                                    link_rtt,
                                    plaintext,
                                },
                                now,
                                sink,
                            );
                        }
                        delivered_segment_bytes = Some(plaintext.len() as u64);
                    }
                }
            }
        }
        self.retire_incoming_resource(link_id, hash);
        match delivered_segment_bytes {
            None => sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                link_id: *link_id,
                hash: *hash,
            })),
            Some(segment_bytes) if multi_segment => {
                if let Some(AssemblyProgress::Complete { total_size }) =
                    self.incoming_assemblies.advance(link_id, segment_bytes)
                {
                    sink(EngineReaction::Journaled(Journaled::ResourceAssembled {
                        link_id: *link_id,
                        original_hash,
                        total_size,
                    }));
                    self.incoming_assemblies.clear(link_id);
                }
            }
            Some(_) => {}
        }
    }

    /// Verified exactly like an uncompressed assembly; the host signals its own
    /// inflate failure with an empty slice. A borrow-taking entry point beside the
    /// command queue (a mebibyte never rides an enum).
    pub fn provide_decompressed(
        &mut self,
        link_id: LinkId,
        hash: ResourceHash,
        plaintext: &[u8],
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules {
        let mut wake_schedule_changes = crate::engine::WakeSchedules::UNCHANGED;
        let Some(index) = self.incoming_resources.lookup(&link_id, &hash) else {
            return wake_schedule_changes;
        };
        let state = *self.incoming_resources.state(index);
        if state.status != IncomingResourceStatus::AwaitingDecompression {
            return wake_schedule_changes;
        }
        self.retire_incoming_resource(&link_id, &hash);
        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();

        let verified = u64::try_from(plaintext.len()) == Ok(state.uncompressed_data_len)
            && self.links.phase_for(&link_id).is_some();
        let proven = verified
            .then(|| verify_and_prove(plaintext, &state.salt_nonce, &hash).ok())
            .flatten();
        let emission = proven.and_then(|proof| {
            let LinkPhase::Active {
                mtu,
                attached_interface,
                rtt,
                ..
            } = self.links.phase_for(&link_id)?
            else {
                return None;
            };
            Some((proof, *mtu, *attached_interface, *rtt))
        });
        match emission {
            Some((proof, mtu, fire_on, link_rtt)) => {
                if let Some(prove) = proof_emission(&link_id, &hash, &proof, mtu) {
                    emit_proof(prove, fire_on, sink);
                }
                deliver_single_segment(
                    &mut self.receipts,
                    AssembledSingleSegment {
                        link_id: &link_id,
                        hash: &hash,
                        correlation: state.correlation,
                        link_rtt,
                        plaintext,
                    },
                    now,
                    sink,
                );
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            None => {
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id,
                    hash,
                }));
            }
        }
        wake_schedule_changes
    }
}

struct AssembledSingleSegment<'a> {
    link_id: &'a LinkId,
    hash: &'a ResourceHash,
    correlation: ResourceCorrelation,
    link_rtt: RttMillis,
    plaintext: &'a [u8],
}

fn deliver_single_segment<C: ReceiptColumns>(
    receipts: &mut Receipts<C>,
    segment: AssembledSingleSegment<'_>,
    now: InstantMillis,
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    let AssembledSingleSegment {
        link_id,
        hash,
        correlation,
        link_rtt,
        plaintext,
    } = segment;
    match correlation {
        ResourceCorrelation::Response(id) => {
            if let Some(proven) = receipts.settle_by_request_id(id.as_bytes()) {
                sink(EngineReaction::Journaled(Journaled::ResponseReceived {
                    command_id: proven.command_id,
                    link_id: *link_id,
                    request_id: id,
                    data: plaintext,
                }));
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id: proven.command_id,
                    settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                        rtt: RttMillis::measured_between(proven.sent_at, now),
                    })),
                }));
            } else {
                sink(EngineReaction::Journaled(Journaled::ResourceReceived {
                    link_id: *link_id,
                    hash: *hash,
                    data: plaintext,
                }));
            }
        }
        ResourceCorrelation::Request(_) => {
            if let Ok(parsed) = parse_request_plaintext(plaintext) {
                sink(EngineReaction::Journaled(Journaled::RequestReceived {
                    link_id: *link_id,
                    request_id: RequestId::of_request_data(plaintext),
                    path_hash: parsed.path_hash,
                    requested_at: parsed.requested_at,
                    rtt: link_rtt,
                    data: parsed.data,
                }));
            }
        }
        ResourceCorrelation::Unsolicited => {
            sink(EngineReaction::Journaled(Journaled::ResourceReceived {
                link_id: *link_id,
                hash: *hash,
                data: plaintext,
            }));
        }
    }
}

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.3.5's watchdog TRANSFERRING branch. A receiver that gives up goes
    /// silent, like the reference; the sender discovers through its own watchdog.
    pub(crate) fn fire_due_incoming_resources<F>(
        &mut self,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        while let Some(index) = self.incoming_resources.due_index(now) {
            let link_id = *self.incoming_resources.link_at(index);
            let hash = *self.incoming_resources.hash_at(index);
            let state = *self.incoming_resources.state(index);
            if state.retries_left == 0 || self.links.phase_for(&link_id).is_none() {
                self.retire_incoming_resource(&link_id, &hash);
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id,
                    hash,
                }));
                continue;
            }
            {
                let state = self.incoming_resources.state_mut(index);
                if state.window > state.window_min {
                    state.window -= 1;
                    if state.window_max > state.window_min {
                        state.window_max -= 1;
                        if (state.window_max - state.window) > (WINDOW_FLEXIBILITY - 1) {
                            state.window_max -= 1;
                        }
                    }
                }
                state.waiting_for_hmu = false;
                state.outstanding_part_count = 0;
                state.retries_left -= 1;
            }
            self.emit_resource_pull(&link_id, &hash, now, fill_entropy, sink);
        }
    }

    /// RNS 1.3.5 `Link.resource_concluded` stores the final window and expected rate
    /// for the next transfer to inherit, however this one ended.
    pub(crate) fn retire_incoming_resource(&mut self, link_id: &LinkId, hash: &ResourceHash) {
        if let Some(index) = self.incoming_resources.lookup(link_id, hash) {
            let state = *self.incoming_resources.state(index);
            let link_rtt_ms = match self.links.phase_for(link_id) {
                Some(LinkPhase::Active { rtt, .. }) => rtt.millis(),
                _ => 1,
            };
            let eifr = expected_inflight_bits_per_second(&state, link_rtt_ms);
            self.links
                .note_resource_concluded(link_id, state.window, eifr);
        }
        self.incoming_resources.remove(link_id, hash);
    }

    /// Drain both registers' due deadlines — the
    /// [`crate::engine::WakeReason::ResourceDeadlines`] arm.
    pub fn fire_due_resource_deadlines<F>(
        &mut self,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        self.fire_due_outgoing_resources(now, fill_entropy, sink);
        self.fire_due_incoming_resources(now, fill_entropy, sink);
        let mut wake_schedule_changes = crate::engine::WakeSchedules::UNCHANGED;
        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
        wake_schedule_changes
    }
}

/// RNS 1.3.5 `Resource.update_eifr`. Never zero: the deadline arithmetic divides
/// by it.
fn expected_inflight_bits_per_second(state: &IncomingResourceState, link_rtt_ms: u64) -> u64 {
    let eifr = if state.data_byte_rate > 0 {
        state.data_byte_rate.saturating_mul(8)
    } else if let Some(inherited) = state.inherited_eifr {
        inherited
    } else {
        let rtt_ms = state.measured_rtt_ms.unwrap_or(link_rtt_ms).max(1);
        ESTABLISHMENT_COST_ESTIMATE_BYTES.saturating_mul(8_000) / rtt_ms
    };
    eifr.max(1)
}

/// RNS 1.3.5's watchdog TRANSFERRING arithmetic: an HMU allowance of x3.5 (as x7/2)
/// when waiting on names or idle; until a round has measured a rate, the wait covers
/// three sdu of flight, the reference's unmeasured fallback.
fn part_round_deadline(
    state: &IncomingResourceState,
    link_rtt_ms: u64,
    now: InstantMillis,
) -> InstantMillis {
    let eifr = expected_inflight_bits_per_second(state, link_rtt_ms);
    let retries_used = (PART_REQUEST_MAX_RETRIES.saturating_sub(state.retries_left)) as u64;
    let extra_wait_ms = retries_used.saturating_mul(PER_RETRY_DELAY_MS);
    let sdu_bits = (state.sdu as u64).saturating_mul(8);
    let wait_ms = if state.request_response_byte_rate != 0 {
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
    } else {
        state
            .part_timeout_factor
            .saturating_mul(sdu_bits.saturating_mul(3_000) / eifr)
    };
    InstantMillis(
        now.0
            .saturating_add(wait_ms)
            .saturating_add(RETRY_GRACE_MS)
            .saturating_add(extra_wait_ms),
    )
}

struct ProofEmission {
    link_id: LinkId,
    plaintext: [u8; PROOF_PLAINTEXT_LEN],
    mtu: usize,
}

fn proof_emission(
    link_id: &LinkId,
    hash: &ResourceHash,
    proof: &crate::routing::links::resources::ResourceProof,
    mtu: usize,
) -> Option<ProofEmission> {
    let mut plaintext = [0u8; PROOF_PLAINTEXT_LEN];
    write_proof_plaintext(hash, proof, &mut plaintext).ok()?;
    Some(ProofEmission {
        link_id: *link_id,
        plaintext,
        mtu,
    })
}

fn emit_proof(
    prove: ProofEmission,
    fire_on: crate::interfaces::InterfaceId,
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    let mut fill = |slot: &mut [u8]| -> Option<usize> {
        write_link_raw_packet(
            &prove.link_id,
            PacketType::Proof,
            WireContext::ResourceProof,
            prove.mtu,
            &prove.plaintext,
            slot,
        )
        .ok()
    };
    sink(EngineReaction::Directive(Directive::EmitFrame {
        target: fire_on,
        size_hint: link_raw_frame_ceiling(prove.plaintext.len()),
        fill: &mut fill,
    }));
}

#[cfg(test)]
mod tests_support {
    use super::*;
    use crate::crypto::{
        x25519_diffie_hellman, Ed25519PublicKey, Ed25519SecretKey, X25519PublicKey, X25519SecretKey,
    };
    use crate::engine::test_support::{filled_frame, routable_descriptor, TestStorageLayout};
    use crate::engine::IngestIo;
    use crate::engine::Journaled;
    use crate::engine::{EngineCommand, IssuedCommand, Settlement};
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::links::resources::{ResourceBody, ResourceSend};
    use crate::routing::links::table::InitiatedLink;
    use crate::routing::links::table::LinkActivation;
    use crate::routing::links::LinkKey;
    use crate::wire::{DestinationHash, BROADCAST_MTU};

    pub(crate) fn bytes_from_hex(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    pub(crate) const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    pub(crate) const INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    pub(crate) const RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";
    pub(crate) const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";

    pub(crate) fn link_id() -> LinkId {
        LinkId::new(bytes_from_hex(LINK_ID).try_into().unwrap())
    }

    pub(crate) fn link_key() -> LinkKey {
        let scalar: [u8; 32] = bytes_from_hex(INITIATOR_SCALAR).try_into().unwrap();
        let public: [u8; 32] = bytes_from_hex(RESPONDER_PUBLIC).try_into().unwrap();
        let shared = x25519_diffie_hellman(&X25519SecretKey::new(scalar), &X25519PublicKey(public));
        LinkKey::derive(&link_id(), &shared)
    }

    pub(crate) fn lane() -> InterfaceId {
        InterfaceId::new([0xEE; 8])
    }

    pub(crate) fn engine_with_active_link() -> EngineState<TestStorageLayout> {
        active_engine::<TestStorageLayout>()
    }

    pub(crate) fn active_engine<S: StorageLayout>() -> EngineState<S> {
        let mut engine = EngineState::<S>::default();
        engine
            .links
            .track_initiated(InitiatedLink {
                link_id: link_id(),
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                link_signing: Ed25519SecretKey::new([0x33; 32]),
                requested_at: InstantMillis(500),
                timeout_at: InstantMillis(5_000),
                command_id: CommandId(1),
            })
            .unwrap();
        engine
            .links
            .activate_initiated(
                &link_id(),
                link_key(),
                &LinkActivation {
                    rtt: crate::units::RttMillis::new(250),
                    mtu: BROADCAST_MTU,
                    attached_interface: lane(),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(1_000),
            )
            .unwrap();
        engine
    }

    pub(crate) fn advertisement_frame(data: &[u8], candidate: Option<&[u8]>) -> std::vec::Vec<u8> {
        let mut sender = engine_with_active_link();
        advertise_from(&mut sender, data, candidate)
    }

    pub(crate) fn advertise_from<S: StorageLayout>(
        sender: &mut EngineState<S>,
        data: &[u8],
        candidate: Option<&[u8]>,
    ) -> std::vec::Vec<u8> {
        let mut frame = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data,
                    compressed_candidate: candidate,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    frame = filled_frame(fill);
                }
            },
        );
        frame.expect("the sender advertises")
    }

    pub(crate) struct InboundCapture {
        pub(crate) frames: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
        pub(crate) settlements: std::vec::Vec<(CommandId, Settlement)>,
        pub(crate) received: std::vec::Vec<(ResourceHash, std::vec::Vec<u8>)>,
        pub(crate) failed: std::vec::Vec<ResourceHash>,
        pub(crate) segments: std::vec::Vec<(ResourceHash, u64, std::vec::Vec<u8>)>,
        pub(crate) assembled: std::vec::Vec<(ResourceHash, u64)>,
        pub(crate) mismatched: std::vec::Vec<(InterfaceId, InterfaceId)>,
        pub(crate) requests: std::vec::Vec<(RequestId, std::vec::Vec<u8>)>,
    }

    pub(crate) fn feed<S: StorageLayout>(
        engine: &mut EngineState<S>,
        frame: &[u8],
        at: u64,
    ) -> InboundCapture {
        feed_on(engine, frame, lane(), at)
    }

    pub(crate) fn feed_on<S: StorageLayout>(
        engine: &mut EngineState<S>,
        frame: &[u8],
        source_interface: InterfaceId,
        at: u64,
    ) -> InboundCapture {
        let mut capture = InboundCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
            received: std::vec::Vec::new(),
            failed: std::vec::Vec::new(),
            segments: std::vec::Vec::new(),
            assembled: std::vec::Vec::new(),
            mismatched: std::vec::Vec::new(),
            requests: std::vec::Vec::new(),
        };
        let mut raw = frame.to_vec();
        engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(at),
                source_interface,
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: &[routable_descriptor(source_interface)],
                now: InstantMillis(at),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                        if let Some(frame) = filled_frame(fill) {
                            capture.frames.push((target, frame));
                        }
                    }
                    EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                        capture.settlements.push((id, settlement));
                    }
                    EngineReaction::Journaled(Journaled::ResourceReceived {
                        hash, data, ..
                    }) => {
                        capture.received.push((hash, data.to_vec()));
                    }
                    EngineReaction::Journaled(Journaled::ResourceFailed { hash, .. }) => {
                        capture.failed.push(hash);
                    }
                    EngineReaction::Journaled(Journaled::ResourceSegmentReceived {
                        original_hash,
                        segment_index,
                        data,
                        ..
                    }) => {
                        capture
                            .segments
                            .push((original_hash, segment_index, data.to_vec()));
                    }
                    EngineReaction::Journaled(Journaled::ResourceAssembled {
                        original_hash,
                        total_size,
                        ..
                    }) => {
                        capture.assembled.push((original_hash, total_size));
                    }
                    EngineReaction::Journaled(Journaled::LinkInterfaceMismatch {
                        attached_interface,
                        arrived_on,
                        ..
                    }) => {
                        capture.mismatched.push((attached_interface, arrived_on));
                    }
                    EngineReaction::Journaled(Journaled::RequestReceived {
                        request_id,
                        data,
                        ..
                    }) => {
                        capture.requests.push((request_id, data.to_vec()));
                    }
                    _ => {}
                },
            },
        );
        capture
    }

    pub(crate) fn accept_everything<S: StorageLayout>(engine: &mut EngineState<S>) {
        let mut settled = std::vec::Vec::new();
        engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_len: 1 << 20,
                        accept_compressed: true,
                    },
                }),
            },
            &[routable_descriptor(lane())],
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        assert!(matches!(
            settled[0],
            (CommandId(9), Settlement::SetResourceStrategy(Ok(()))),
        ));
    }

    pub(crate) fn four_part_payload() -> std::vec::Vec<u8> {
        b"resource parts ride raw on the wire! ".repeat(41)
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::test_support::{routable_descriptor, TestStorageLayout};
    use crate::engine::{EngineCommand, IssuedCommand, SetResourceStrategyFailure, Settlement};
    use crate::routing::links::resources::{ResourceBody, ResourceSend};

    use crate::engine::Journaled;
    use crate::routing::links::resources::control::parse_part_request_plaintext;
    use crate::wire::WirePacketHeader;

    #[test]
    fn the_default_strategy_ignores_advertisements() {
        let mut receiver = engine_with_active_link();
        let capture = feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        assert!(capture.frames.is_empty());
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn a_response_resource_settles_its_request_despite_the_default_strategy() {
        use crate::crypto::Ed25519PublicKey;
        use crate::engine::test_support::filled_frame;
        use crate::identity::IdentitySigningPublicKey;
        use crate::routing::dedup::PacketHash;
        use crate::routing::delivery::receipts::{OutstandingReceipt, ReceiptKind};
        use crate::routing::links::request::RequestId;
        use crate::wire::{DestinationType, PacketType, WireContext};

        let mut receiver = engine_with_active_link();
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &link_id().to_address(),
            WireContext::Request,
            &b"the request we sent"[..],
        );
        let request_id = RequestId::of_packet(&packet_hash);
        receiver.receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: CommandId(42),
            kind: ReceiptKind::SendRequest,
            peer_signing_key: IdentitySigningPublicKey::new(Ed25519PublicKey([0x99; 32])),
            sent_at: InstantMillis(1_800),
            timeout_at: InstantMillis(20_000),
        });

        let data = four_part_payload();
        let mut sender = engine_with_active_link();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &data,
                    compressed_candidate: None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Response(
                    request_id,
                ),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let advertisement = advertisement.expect("the responder advertises its response resource");

        let pull = feed(&mut receiver, &advertisement, 2_000);
        assert_eq!(
            pull.frames.len(),
            1,
            "a response to a request we sent is pulled, default strategy notwithstanding",
        );
        assert!(!receiver.incoming_resources.is_empty());

        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(serve.frames.len(), 4, "the peer streams every part");

        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.settlements.is_empty() || !capture.received.is_empty() {
                conclusion = Some(capture);
            }
        }
        let conclusion = conclusion.expect("the last part concludes the response");
        assert!(
            conclusion.received.is_empty(),
            "a response settles its request, not a bare ResourceReceived",
        );
        assert!(matches!(
            conclusion.settlements[0],
            (CommandId(42), Settlement::SendRequest(Ok(_))),
        ));
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn a_request_resource_dispatches_a_request_despite_the_default_strategy() {
        use crate::engine::test_support::filled_frame;
        use crate::routing::links::request::{write_request_plaintext, RequestId};
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::routing::request_handlers::RequestPathHash;

        let path_hash = RequestPathHash::new([0x44; 16]);
        let request_data = b"a request too fat for one packet, ".repeat(40);
        let mut packed = std::vec![0u8; request_data.len() + 64];
        let plain_len =
            write_request_plaintext(InstantMillis(1_400), &path_hash, &request_data, &mut packed)
                .unwrap();
        let packed_request = &packed[..plain_len];
        let request_id = RequestId::of_request_data(packed_request);

        let mut sender = engine_with_active_link();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: packed_request,
                    compressed_candidate: None,
                },
                correlation: ResourceCorrelation::Request(request_id),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let advertisement = advertisement.expect("the peer advertises its request resource");

        let mut receiver = engine_with_active_link();
        let pull = feed(&mut receiver, &advertisement, 2_000);
        assert_eq!(
            pull.frames.len(),
            1,
            "an inbound request is accepted and pulled, default strategy notwithstanding",
        );

        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.requests.is_empty() || !capture.received.is_empty() {
                conclusion = Some(capture);
            }
        }
        let conclusion = conclusion.expect("the last part concludes the request");
        assert!(
            conclusion.received.is_empty(),
            "a request resource dispatches a RequestReceived, not a bare ResourceReceived",
        );
        assert_eq!(conclusion.requests.len(), 1);
        assert_eq!(conclusion.requests[0].0, request_id);
        assert_eq!(conclusion.requests[0].1, request_data);
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn a_big_request_rides_a_resource_that_books_its_pending_row() {
        use crate::engine::test_support::filled_frame;
        use crate::routing::links::request::{write_request_plaintext, RequestId};
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::routing::request_handlers::RequestPathHash;

        let path_hash = RequestPathHash::new([0x55; 16]);
        let request_data = b"a request too fat for a packet, ".repeat(40);
        let mut packed = std::vec![0u8; request_data.len() + 64];
        let plain_len =
            write_request_plaintext(InstantMillis(1_400), &path_hash, &request_data, &mut packed)
                .unwrap();
        let packed_request = &packed[..plain_len];
        let request_id = RequestId::of_request_data(packed_request);

        let mut requester = engine_with_active_link();
        assert!(
            requester.request_fits_packet(&link_id(), b"small enough"),
            "a tiny request rides a packet",
        );
        assert!(
            !requester.request_fits_packet(&link_id(), packed_request),
            "a >MDU request does not fit a packet — it must ride a resource",
        );

        requester.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(55),
                link_id: link_id(),
                body: ResourceBody {
                    data: packed_request,
                    compressed_candidate: None,
                },
                correlation: ResourceCorrelation::Request(request_id),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    let _ = filled_frame(fill);
                }
            },
        );

        assert!(
            requester
                .receipts
                .has_pending_request(request_id.as_bytes()),
            "the request resource books the pending row its response will settle",
        );
    }

    fn crafted_split_advertisement(segment_index: u64, total_segments: u64) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::advertisement::{
            ResourceAdvertisement, ResourceFlags,
        };
        use crate::routing::links::resources::SaltNonce;
        use crate::wire::BROADCAST_MTU;
        let part_count = 4usize;
        let names = [0xCDu8; 16];
        let advertisement = ResourceAdvertisement {
            transfer_size: (part_count * 464) as u64,
            data_size: 1_000,
            part_count: part_count as u64,
            hash: ResourceHash::new([0xAB; 32]),
            salt_nonce: SaltNonce::new([0x61; 4]),
            original_hash: ResourceHash::new([0xAB; 32]),
            segment_index,
            total_segments,
            request_id: None,
            flags: ResourceFlags {
                encrypted: true,
                compressed: false,
                split: total_segments > 1,
                is_request: false,
                is_response: false,
                has_metadata: false,
            },
            hashmap: &names,
        };
        let mut plaintext = [0u8; 431];
        let plaintext_len = advertisement.write(&mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[0xD1; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    #[test]
    fn a_split_advertisement_opens_a_chain_keyed_by_original_hash() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        feed(&mut receiver, &crafted_split_advertisement(1, 3), 2_000);
        assert!(!receiver.incoming_resources.is_empty());
        assert_eq!(
            receiver.incoming_assemblies.original_hash(&link_id()),
            Some(ResourceHash::new([0xAB; 32])),
        );
    }

    #[test]
    fn a_segment_index_past_the_chain_length_is_refused() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        feed(&mut receiver, &crafted_split_advertisement(3, 2), 2_000);
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(receiver.incoming_assemblies.original_hash(&link_id()), None);
    }

    #[test]
    fn the_strategy_command_demands_an_active_link() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut settled = std::vec::Vec::new();
        engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::AcceptNone,
                }),
            },
            &[routable_descriptor(lane())],
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        assert!(matches!(
            settled[0],
            (
                CommandId(9),
                Settlement::SetResourceStrategy(Err(SetResourceStrategyFailure::Rejected(
                    SetResourceStrategyRejection::NoSuchLink,
                ))),
            ),
        ));
    }

    #[test]
    fn an_accepted_advertisement_registers_and_pulls_the_first_window() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let capture = feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );

        assert_eq!(capture.frames.len(), 1);
        let (target, frame) = &capture.frames[0];
        assert_eq!(*target, lane());
        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.context, WireContext::ResourceRequest);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let request = parse_part_request_plaintext(opened).unwrap();
        assert_eq!(request.last_known_map_hash, None);

        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &request.hash)
            .expect("the transfer is registered");
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.part_count, 4);
        assert_eq!(state.outstanding_part_count, 4);
        assert!(!state.waiting_for_hmu);
        assert_eq!(
            request.requested,
            receiver.incoming_resources.names_flat(index),
            "the first window asks for every part it can name",
        );
    }

    #[test]
    fn policy_refusals_are_silent() {
        let compressed = advertisement_frame(
            &b"reticulum resources ride the link ".repeat(40),
            Some(&bytes_from_hex(CASE1_BZ2)),
        );

        let mut no_compression = engine_with_active_link();
        let mut settled = std::vec::Vec::new();
        no_compression.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_len: 1 << 20,
                        accept_compressed: false,
                    },
                }),
            },
            &[routable_descriptor(lane())],
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        let capture = feed(&mut no_compression, &compressed, 2_000);
        assert!(capture.frames.is_empty());
        assert!(no_compression.incoming_resources.is_empty());

        let mut tiny_cap = engine_with_active_link();
        let mut settled = std::vec::Vec::new();
        tiny_cap.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_len: 100,
                        accept_compressed: true,
                    },
                }),
            },
            &[routable_descriptor(lane())],
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        let capture = feed(
            &mut tiny_cap,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        assert!(capture.frames.is_empty());
        assert!(tiny_cap.incoming_resources.is_empty());
    }

    #[test]
    fn a_duplicate_advertisement_is_filtered() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let advertisement = advertisement_frame(&four_part_payload(), None);
        let first = feed(&mut receiver, &advertisement, 2_000);
        let second = feed(&mut receiver, &advertisement, 2_100);

        assert_eq!(first.frames.len(), 1);
        assert!(second.frames.is_empty());
        assert_eq!(receiver.incoming_resources.len(), 1);
    }
}

#[cfg(test)]
mod loop_tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::IngestIo;
    use crate::engine::Settlement;
    use crate::interfaces::InterfaceId;
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::resources::advertisement::write_hashmap_update_plaintext;
    use crate::routing::links::resources::advertisement::ResourceAdvertisement;
    use crate::routing::links::resources::control::write_part_request_plaintext;
    use crate::routing::links::resources::SaltNonce;
    use crate::routing::links::resources::{ResourceBody, ResourceSegment, ResourceSend};
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
                total_data_size: total,
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
                total_data_size: total,
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
        total_data_size: u64,
        at: u64,
    ) -> std::vec::Vec<u8> {
        let mut frame = None;
        sender.ingest_send_resource_segment_into(
            &ResourceSend {
                id: command_id,
                link_id: link_id(),
                body: ResourceBody {
                    data,
                    compressed_candidate: None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            ResourceSegment {
                index: segment_index,
                total_segments,
                total_data_size,
            },
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    frame = filled_frame(fill);
                }
            },
        );
        frame.expect("the sender advertises the segment")
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
        let own = *sender.outgoing_resources.hash_at(0);
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
        let own = *sender.outgoing_resources.hash_at(0);
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
                adv.data_size, total,
                "RNS 1.3.5 parity: every segment advertises the original total, not its own size",
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
                total_data_size: total,
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
        let own = *sender.outgoing_resources.hash_at(0);
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
                adv.data_size, total,
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
            .write_owed_link_close(&link_id(), &[0u8; 16], &mut buf)
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
    fn a_split_segment_with_no_open_chain_falls_back_to_its_own_hash() {
        let mut sender = engine_with_active_link();
        send_segment(
            &mut sender,
            CommandId(11),
            &four_part_payload(),
            2,
            2,
            (2 * four_part_payload().len()) as u64,
            1_500,
        );
        let own = *sender.outgoing_resources.hash_at(0);
        let state = sender.outgoing_resources.state(0);
        assert_eq!(
            state.original_hash, own,
            "a later segment with no chain to read falls back to its own hash",
        );
        assert_eq!(state.segment_index, 2);
        assert_eq!(state.total_segments, 2);
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
                interfaces: &[crate::engine::test_support::routable_descriptor(lane())],
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
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

    fn crafted_partial_advertisement(names: &[u8], part_count: usize) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::advertisement::{
            ResourceAdvertisement, ResourceFlags,
        };
        let advertisement = ResourceAdvertisement {
            transfer_size: (part_count * 464) as u64,
            data_size: 2_700,
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
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[0xD1; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
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
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceHashUpdate,
            &plaintext[..plaintext_len],
            &[iv; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    fn six_names() -> std::vec::Vec<u8> {
        let mut names = std::vec::Vec::new();
        for i in 1u32..=6 {
            names.extend_from_slice(&i.to_be_bytes());
        }
        names
    }

    #[test]
    fn an_exhausted_pull_resumes_when_the_hashmap_update_lands() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);

        let names = six_names();
        let pull = feed(
            &mut receiver,
            &crafted_partial_advertisement(&names[..8], 6),
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
            &crafted_partial_advertisement(&names[..8], 6),
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
            &crafted_partial_advertisement(&names[..8], 6),
            2_000,
        );

        let cancelled = feed(
            &mut receiver,
            &sealed_hashmap_update(5, &names, 0xD3),
            2_100,
        );
        assert!(cancelled.frames.is_empty());
        assert_eq!(cancelled.failed.len(), 1);
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
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[0xD4; 16],
            &mut frame,
        )
        .unwrap();
        feed(&mut receiver, &frame[..wire_len], 2_000);

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
        assert!(receiver.incoming_resources.is_empty());

        let _ = WirePacketType::Proof;
    }
}

#[cfg(test)]
mod seam_tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::IngestIo;
    use crate::engine::Journaled;
    use crate::engine::Settlement;
    use crate::routing::links::resources::table::IncomingResourceStatus;
    use crate::routing::links::resources::{ResourceBody, ResourceSend};

    fn case1_plaintext() -> std::vec::Vec<u8> {
        b"reticulum resources ride the link ".repeat(40)
    }

    #[test]
    fn a_compressed_transfer_crosses_through_the_host_inflate_seam() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let plaintext = case1_plaintext();
        let candidate = bytes_from_hex(CASE1_BZ2);

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &plaintext,
                    compressed_candidate: Some(&candidate),
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
        assert_eq!(serve.frames.len(), 1, "the compressed stream is one part");

        let mut needs = None;
        let mut raw = serve.frames[0].1.clone();
        receiver.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: &[crate::engine::test_support::routable_descriptor(lane())],
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                        hash,
                        stream,
                        uncompressed_data_len,
                        ..
                    }) = reaction
                    {
                        needs = Some((hash, stream.to_vec(), uncompressed_data_len));
                    }
                },
            },
        );
        let (hash, stream, advertised_len) = needs.expect("the seam asks the host to inflate");
        assert_eq!(
            stream,
            bytes_from_hex(CASE1_BZ2),
            "the host receives exactly the bz2 stream the sender compressed",
        );
        assert_eq!(advertised_len, 1_360);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        assert_eq!(
            receiver.incoming_resources.state(index).status,
            IncomingResourceStatus::AwaitingDecompression,
        );

        let mut frames = std::vec::Vec::new();
        let mut received = std::vec::Vec::new();
        receiver.provide_decompressed(
            link_id(),
            hash,
            &plaintext,
            InstantMillis(2_400),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        frames.push(frame);
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceReceived { data, .. }) => {
                    received.push(data.to_vec());
                }
                _ => {}
            },
        );
        assert_eq!(received.len(), 1);
        assert_eq!(received[0], plaintext);
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(frames.len(), 1, "the proof rides back");

        let settled = feed(&mut sender, &frames[0], 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
        assert!(sender.outgoing_resources.is_empty());
    }

    #[test]
    fn a_wrong_or_empty_inflate_fails_the_transfer_by_hash() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let plaintext = case1_plaintext();
        let candidate = bytes_from_hex(CASE1_BZ2);

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &plaintext,
                    compressed_candidate: Some(&candidate),
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
        let conclusion = feed(&mut receiver, &serve.frames[0].1, 2_200);
        assert!(conclusion.received.is_empty());
        let hash = *receiver.incoming_resources.hash_at(0);

        let mut corrupted = plaintext.clone();
        corrupted[0] ^= 1;
        let mut failed = std::vec::Vec::new();
        let mut frames = 0usize;
        receiver.provide_decompressed(
            link_id(),
            hash,
            &corrupted,
            InstantMillis(2_400),
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::ResourceFailed { hash, .. }) => {
                    failed.push(hash);
                }
                EngineReaction::Directive(_) => frames += 1,
                _ => {}
            },
        );
        assert_eq!(failed.len(), 1);
        assert_eq!(frames, 0, "a failed inflate proves nothing");
        assert!(receiver.incoming_resources.is_empty());

        receiver.provide_decompressed(
            link_id(),
            hash,
            &plaintext,
            InstantMillis(2_500),
            &mut |_| {
                panic!("a retired transfer answers nothing");
            },
        );
    }

    #[test]
    fn the_seam_only_answers_transfers_awaiting_decompression() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let mut touched = false;
        receiver.provide_decompressed(
            link_id(),
            ResourceHash::new([0x42; 32]),
            b"anything",
            InstantMillis(2_400),
            &mut |_| touched = true,
        );
        assert!(!touched, "an unknown transfer answers nothing");
    }

    #[test]
    fn a_compressed_response_to_a_pending_request_is_accepted_and_settles_it() {
        use crate::routing::links::request::{write_request_plaintext, RequestId};
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::routing::request_handlers::RequestPathHash;

        let path_hash = RequestPathHash::new([0x55; 16]);
        let request_data = b"a request too fat for a packet, ".repeat(40);
        let mut packed = std::vec![0u8; request_data.len() + 64];
        let plain_len =
            write_request_plaintext(InstantMillis(1_400), &path_hash, &request_data, &mut packed)
                .unwrap();
        let request_id = RequestId::of_request_data(&packed[..plain_len]);

        let mut requester = engine_with_active_link();
        requester.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(55),
                link_id: link_id(),
                body: ResourceBody {
                    data: &packed[..plain_len],
                    compressed_candidate: None,
                },
                correlation: ResourceCorrelation::Request(request_id),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    let _ = filled_frame(fill);
                }
            },
        );
        assert!(requester
            .receipts
            .has_pending_request(request_id.as_bytes()));

        let mut responder = engine_with_active_link();
        let response = case1_plaintext();
        let candidate = b"pretend bz2, just visibly shorter".to_vec();
        let mut advertisement = None;
        responder.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(9),
                link_id: link_id(),
                body: ResourceBody {
                    data: &response,
                    compressed_candidate: Some(&candidate),
                },
                correlation: ResourceCorrelation::Response(request_id),
            },
            InstantMillis(1_600),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let pull = feed(&mut requester, &advertisement.unwrap(), 2_000);
        assert!(
            !requester.incoming_resources.is_empty(),
            "a compressed response to a pending request bypasses the strategy like the reference",
        );
        let serve = feed(&mut responder, &pull.frames[0].1, 2_100);
        let mut needs = None;
        let mut raw = serve.frames[0].1.clone();
        requester.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: &[crate::engine::test_support::routable_descriptor(lane())],
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                        hash,
                        stream,
                        ..
                    }) = reaction
                    {
                        needs = Some((hash, stream.to_vec()));
                    }
                },
            },
        );
        let (hash, stream) = needs.expect("the compressed response reaches the inflate seam");
        assert_eq!(stream, candidate);

        let mut proof_frames = 0usize;
        let mut responses = std::vec::Vec::new();
        let mut settled_ok = false;
        requester.provide_decompressed(
            link_id(),
            hash,
            &response,
            InstantMillis(2_400),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { .. }) => proof_frames += 1,
                EngineReaction::Journaled(Journaled::ResponseReceived {
                    command_id,
                    request_id: rid,
                    data,
                    ..
                }) => responses.push((command_id, rid, data.to_vec())),
                EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendRequest(Ok(_)),
                }) => settled_ok = id == CommandId(55),
                _ => {}
            },
        );
        assert_eq!(proof_frames, 1, "the proof rides back");
        assert_eq!(responses, std::vec![(CommandId(55), request_id, response)]);
        assert!(
            settled_ok,
            "the inflated response settles the pending request it answers",
        );
        assert!(!requester
            .receipts
            .has_pending_request(request_id.as_bytes()));
    }

    #[test]
    fn a_compressed_request_resource_is_accepted_and_delivered_as_a_request() {
        use crate::routing::links::request::{write_request_plaintext, RequestId};
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::routing::request_handlers::RequestPathHash;

        let path_hash = RequestPathHash::new([0x66; 16]);
        let request_data = b"a fat, highly compressible request ".repeat(40);
        let mut packed = std::vec![0u8; request_data.len() + 64];
        let plain_len =
            write_request_plaintext(InstantMillis(1_400), &path_hash, &request_data, &mut packed)
                .unwrap();
        let packed_request = packed[..plain_len].to_vec();
        let request_id = RequestId::of_request_data(&packed_request);
        let candidate = b"pretend bz2 for the request body".to_vec();

        let mut requester = engine_with_active_link();
        let mut advertisement = None;
        requester.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(56),
                link_id: link_id(),
                body: ResourceBody {
                    data: &packed_request,
                    compressed_candidate: Some(&candidate),
                },
                correlation: ResourceCorrelation::Request(request_id),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let mut responder = engine_with_active_link();
        let pull = feed(&mut responder, &advertisement.unwrap(), 2_000);
        assert!(
            !responder.incoming_resources.is_empty(),
            "a compressed request resource is accepted, like the reference's unconditional Resource.accept",
        );
        let serve = feed(&mut requester, &pull.frames[0].1, 2_100);
        let mut needs = None;
        let mut raw = serve.frames[0].1.clone();
        responder.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: &[crate::engine::test_support::routable_descriptor(lane())],
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                        hash,
                        ..
                    }) = reaction
                    {
                        needs = Some(hash);
                    }
                },
            },
        );
        let hash = needs.expect("the compressed request reaches the inflate seam");

        let mut requests = std::vec::Vec::new();
        responder.provide_decompressed(
            link_id(),
            hash,
            &packed_request,
            InstantMillis(2_400),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RequestReceived {
                    request_id: rid,
                    path_hash: ph,
                    data,
                    ..
                }) = reaction
                {
                    requests.push((rid, ph, data.to_vec()));
                }
            },
        );
        assert_eq!(requests, std::vec![(request_id, path_hash, request_data)]);
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::{SendResourceFailure, Settlement};
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::resources::control::write_cancel_plaintext;
    use crate::routing::links::resources::RESOURCE_HASH_LEN;
    use crate::routing::links::resources::{ResourceBody, ResourceSend};
    use crate::wire::BROADCAST_MTU;

    fn four_part_setup() -> (
        EngineState<crate::engine::test_support::TestStorageLayout>,
        EngineState<crate::engine::test_support::TestStorageLayout>,
        ResourceHash,
    ) {
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
        feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let hash = *receiver.incoming_resources.hash_at(0);
        (sender, receiver, hash)
    }

    fn sealed_cancel(hash: &ResourceHash, context: WireContext, iv: u8) -> std::vec::Vec<u8> {
        let mut plaintext = [0u8; RESOURCE_HASH_LEN];
        write_cancel_plaintext(hash, &mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            context,
            &plaintext,
            &[iv; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    #[test]
    fn the_senders_cancel_drops_the_receivers_transfer() {
        let (_, mut receiver, hash) = four_part_setup();
        let cancelled = feed(
            &mut receiver,
            &sealed_cancel(&hash, WireContext::ResourceInitiatorCancel, 0xE1),
            2_500,
        );
        assert_eq!(cancelled.failed.len(), 1);
        assert!(receiver.incoming_resources.is_empty());

        let again = feed(
            &mut receiver,
            &sealed_cancel(&hash, WireContext::ResourceInitiatorCancel, 0xE2),
            2_600,
        );
        assert!(
            again.failed.is_empty(),
            "a cancel for nothing journals nothing"
        );
    }

    #[test]
    fn the_receivers_reject_settles_the_send_by_its_name() {
        let (mut sender, _, hash) = four_part_setup();
        let rejected = feed(
            &mut sender,
            &sealed_cancel(&hash, WireContext::ResourceReceiverCancel, 0xE3),
            2_500,
        );
        assert!(matches!(
            rejected.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::RejectedByPeer)),
            ),
        ));
        assert!(sender.outgoing_resources.is_empty());

        let unknown = feed(
            &mut sender,
            &sealed_cancel(
                &ResourceHash::new([0x5A; 32]),
                WireContext::ResourceReceiverCancel,
                0xE4,
            ),
            2_600,
        );
        assert!(unknown.settlements.is_empty());
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::{Journaled, WakeSchedule};

    struct WatchCapture {
        frames: usize,
        failed: usize,
    }

    fn fire(
        engine: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        at: u64,
    ) -> WatchCapture {
        let mut capture = WatchCapture {
            frames: 0,
            failed: 0,
        };
        engine.fire_due_resource_deadlines(
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xF2),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if filled_frame(fill).is_some() {
                        capture.frames += 1;
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceFailed { .. }) => {
                    capture.failed += 1;
                }
                _ => {}
            },
        );
        capture
    }

    #[test]
    fn a_starved_pull_shrinks_its_window_and_asks_again() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let pull = feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        assert_eq!(pull.frames.len(), 1);
        let bootstrap_eifr = 287 * 8_000 / 250;
        let unmeasured_wait = 4 * (464 * 8 * 3_000 / bootstrap_eifr);
        assert_eq!(
            receiver.resource_deadlines_wake(),
            WakeSchedule::At(InstantMillis(2_000 + unmeasured_wait + 250)),
            "an unmeasured pull waits three sdu of flight at the establishment-bootstrapped rate",
        );

        let retried = fire(&mut receiver, 2_000 + unmeasured_wait + 250);
        assert_eq!(retried.frames, 1, "the pull goes out again");
        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.window, 3, "the window eases down");
        assert_eq!(state.window_max, 8, "and its ceiling follows twice");
        assert_eq!(state.retries_left, 15);
        assert_eq!(
            receiver.resource_deadlines_wake(),
            WakeSchedule::At(InstantMillis(
                2_000 + unmeasured_wait + 250 + unmeasured_wait + 250 + 500,
            )),
            "the next deadline stretches by one per-retry delay",
        );
    }

    #[test]
    fn a_received_part_refills_the_retry_budget_like_the_reference() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let pull = feed(
            &mut receiver,
            &advertise_from(&mut sender, &four_part_payload(), None),
            2_000,
        );

        let bootstrap_eifr = 287 * 8_000 / 250;
        let unmeasured_wait = 4 * (464 * 8 * 3_000 / bootstrap_eifr);
        fire(&mut receiver, 2_000 + unmeasured_wait + 250);
        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        assert_eq!(receiver.incoming_resources.state(index).retries_left, 15);

        let serve = feed(&mut sender, &pull.frames[0].1, 30_000);
        feed(&mut receiver, &serve.frames[0].1, 30_100);
        assert_eq!(
            receiver.incoming_resources.state(index).retries_left,
            PART_REQUEST_MAX_RETRIES,
            "a placed part refills the budget so only consecutive dead rounds exhaust it",
        );
    }

    #[test]
    fn a_receiver_out_of_retries_goes_silent_and_fails() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        receiver.incoming_resources.state_mut(index).retries_left = 0;

        let gave_up = fire(&mut receiver, 60_000);
        assert_eq!(
            gave_up.frames, 0,
            "giving up sends nothing, like the reference"
        );
        assert_eq!(gave_up.failed, 1);
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(receiver.resource_deadlines_wake(), WakeSchedule::Idle);
    }
}

#[cfg(test)]
mod dynamics_tests {
    use super::tests_support::*;
    use super::*;
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
                failed: std::vec::Vec::new(),
                segments: std::vec::Vec::new(),
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
