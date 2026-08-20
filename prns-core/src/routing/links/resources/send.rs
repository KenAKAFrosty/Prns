//! RNS 1.4.2 `Resource(data, link)` plus `Resource.advertise`.

use crate::engine::{Directive, EngineReaction, EngineState, InstantMillis, Journaled};
use crate::engine::{RespondFailure, SendResourceFailure, SendResourceRejection, Settlement};
use crate::interfaces::InterfaceId;
use crate::rncp::write_file_metadata;
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::ingress::{DataPacket, IgnoreReason, IngestPacketOutcome};
use crate::routing::links::data::LINK_TRAFFIC_TIMEOUT_FACTOR;
use crate::routing::links::data::{
    link_data_frame_ceiling, link_raw_frame_ceiling, write_link_packet, write_link_raw_packet,
    LINK_MDU,
};
use crate::routing::links::request::{
    response_envelope_prefix, write_packed_binary_header, MAX_PACKED_BINARY_HEADER_LEN,
    RESPONSE_WIRE_OVERHEAD,
};
use crate::routing::links::resources::advertisement::{
    write_hashmap_update_plaintext, ResourceAdvertisement, ResourceFlags,
};
use crate::routing::links::resources::assembly::StaticResponseContinuation;
use crate::routing::links::resources::build_outgoing::{
    build_outgoing_resource_enveloped, outgoing_resource_buffer_shape, seal_staged_resource,
    winning_candidate, BuildOutgoingResourceError, SealedStagedResource, STAGED_STREAM_OFFSET,
};
use crate::routing::links::resources::control::{
    parse_cancel_plaintext, parse_part_request_plaintext, parse_proof_plaintext,
    write_cancel_plaintext,
};
use crate::routing::links::resources::serve_outgoing::{plan_hashmap_update, serve_part_indices};
use crate::routing::links::resources::table::{
    OutgoingResourceStatus, PartSendOutcome, TrackLane, TrackOutgoingResourceError, TrackedCommand,
};
use crate::routing::links::resources::{
    resource_sdu, ResourceBody, ResourceCorrelation, ResourceHash, ResourceMetadata,
    ResourcePartRequest, ResourceSegment, ResourceSend, HASHMAP_MAX_LEN, MAP_HASH_LEN,
    MAX_ADVERTISEMENT_RETRIES, PART_REQUEST_MAX_RETRIES, PER_RETRY_DELAY_MS, PROCESSING_GRACE_MS,
    PROOF_TIMEOUT_FACTOR, RESOURCE_HASH_LEN, RESOURCE_NONCE_LEN, SENDER_GRACE_MS,
};
use crate::routing::links::table::{ActiveLinkLookup, LinkPhase};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
#[cfg(test)]
use crate::wire::DestinationHash;
use crate::wire::{DestinationType, PacketType, WireContext};

/// Automatic static responses keep no more than this much plaintext in one resource window.
pub const STATIC_RESPONSE_SEGMENT_BYTES: usize = 256 * 1024;
const STATIC_FILE_METADATA_BYTES: usize = 6 + 2 + u8::MAX as usize;

/// A pool worker's finished seal, exactly as it returns: the identity that finds the row, the bytes that land on it, and the outcome that gates them.
pub struct OffloadedStagedSeal<'a> {
    pub link_id: LinkId,
    pub stream_nonce: [u8; RESOURCE_NONCE_LEN],
    pub nonce_prefixed_bytes: usize,
    pub sealed_bytes: &'a [u8],
    pub names: &'a [u8],
    pub outcome: Result<SealedStagedResource, BuildOutgoingResourceError>,
}

/// What a crypto-pool seal job copies before the row parks as `StagedSealing`.
pub struct StagedSealJobView<'a> {
    pub key: &'a crate::routing::links::LinkKey,
    pub sdu: usize,
    pub nonce_prefixed_bytes: usize,
    /// The worker's whole input: the reserved IV span, the stream nonce, and the parked raw stream.
    pub plaintext: &'a [u8],
}

/// How a landed segment is addressed for its post-landing patch: a built row by its hash, but a raw row only by index because its hash column is still the placeholder.
enum RowLanding {
    Built(ResourceHash),
    Raw(usize),
}

pub(crate) enum ResourceProofClassification {
    Resolved(IngestPacketOutcome<'static>),
    NotALocalLink,
}

pub(crate) fn resource_settlement(
    correlation: ResourceCorrelation,
    result: Result<(), SendResourceFailure>,
) -> Settlement {
    match correlation {
        ResourceCorrelation::Response(_) => {
            Settlement::Respond(result.map_err(RespondFailure::Resource))
        }
        ResourceCorrelation::Unsolicited | ResourceCorrelation::Request { .. } => {
            Settlement::SendResource(result)
        }
    }
}

fn static_response_stream_capacity(transfer_capacity: usize) -> usize {
    let mut stream_bytes = STATIC_RESPONSE_SEGMENT_BYTES
        .min(crate::routing::links::resources::MAX_EFFICIENT_SIZE)
        .min(transfer_capacity.saturating_sub(48));
    while stream_bytes > 0
        && crate::routing::links::resources::sealed_transfer_bytes(stream_bytes) > transfer_capacity
    {
        stream_bytes -= 1;
    }
    stream_bytes
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_send_resource_into<F>(
        &mut self,
        send: &ResourceSend<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        self.ingest_send_resource_segment_into(
            send,
            ResourceSegment::whole(send.body.data.len() as u64),
            now,
            fill_entropy,
            sink,
        )
    }

    pub fn ingest_send_static_response_into<F>(
        &mut self,
        id: crate::engine::CommandId,
        respond: &crate::engine::Respond,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let (data, file_name): (&'static [u8], Option<&'static str>) = match &respond.payload {
            crate::engine::RespondPayload::StaticBytes(data) => (*data, None),
            #[cfg(any(feature = "large-static-responses", test))]
            crate::engine::RespondPayload::StaticFile { name, bytes } => (*bytes, Some(*name)),
            crate::engine::RespondPayload::Packed(_) => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::Respond(Err(
                        crate::engine::RespondFailure::WriteFailed,
                    )),
                }));
                return crate::engine::WakeSchedules::UNCHANGED;
            }
        };
        let mut envelope = [0u8; RESPONSE_WIRE_OVERHEAD + MAX_PACKED_BINARY_HEADER_LEN];
        let envelope_len = match file_name {
            Some(_) => 0,
            None => {
                envelope[..RESPONSE_WIRE_OVERHEAD]
                    .copy_from_slice(&response_envelope_prefix(&respond.request_id));
                let Ok(binary_header_len) =
                    write_packed_binary_header(data.len(), &mut envelope[RESPONSE_WIRE_OVERHEAD..])
                else {
                    sink(EngineReaction::Journaled(Journaled::CommandSettled {
                        id,
                        settlement: Settlement::Respond(Err(
                            crate::engine::RespondFailure::WriteFailed,
                        )),
                    }));
                    return crate::engine::WakeSchedules::UNCHANGED;
                };
                RESPONSE_WIRE_OVERHEAD + binary_header_len
            }
        };
        let mut metadata_buffer = [0u8; STATIC_FILE_METADATA_BYTES];
        let metadata = match file_name {
            Some(name) => {
                let Ok(packed_len) = write_file_metadata(name.as_bytes(), &mut metadata_buffer)
                else {
                    sink(EngineReaction::Journaled(Journaled::CommandSettled {
                        id,
                        settlement: Settlement::Respond(Err(
                            crate::engine::RespondFailure::WriteFailed,
                        )),
                    }));
                    return crate::engine::WakeSchedules::UNCHANGED;
                };
                ResourceMetadata::Packed(&metadata_buffer[..packed_len])
            }
            None => ResourceMetadata::None,
        };
        let segment_stream_bytes =
            static_response_stream_capacity(self.outgoing_resources.transfer_capacity());
        let first_overhead = envelope_len + metadata.block_len();
        if segment_stream_bytes <= first_overhead {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id,
                settlement: Settlement::Respond(Err(crate::engine::RespondFailure::Resource(
                    SendResourceFailure::Rejected(SendResourceRejection::Build(
                        BuildOutgoingResourceError::DataTooLarge,
                    )),
                ))),
            }));
            return crate::engine::WakeSchedules::UNCHANGED;
        }
        let first_bytes = data.len().min(segment_stream_bytes - first_overhead);
        let remaining = data.len() - first_bytes;
        let following_segments = remaining.div_ceil(segment_stream_bytes);
        let Ok(total_segments) = u64::try_from(following_segments + 1) else {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id,
                settlement: Settlement::Respond(Err(crate::engine::RespondFailure::WriteFailed)),
            }));
            return crate::engine::WakeSchedules::UNCHANGED;
        };
        if total_segments > 1 && !self.outgoing_assemblies.supports_static_continuations() {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id,
                settlement: Settlement::Respond(Err(crate::engine::RespondFailure::Resource(
                    SendResourceFailure::Rejected(SendResourceRejection::Build(
                        BuildOutgoingResourceError::DataTooLarge,
                    )),
                ))),
            }));
            return crate::engine::WakeSchedules::UNCHANGED;
        }
        let Ok(total_data_bytes) = u64::try_from(envelope_len + data.len()) else {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id,
                settlement: Settlement::Respond(Err(crate::engine::RespondFailure::WriteFailed)),
            }));
            return crate::engine::WakeSchedules::UNCHANGED;
        };
        let wake = self.ingest_send_resource_segment_enveloped(
            &ResourceSend {
                id,
                link_id: respond.link_id,
                body: ResourceBody {
                    data: &data[..first_bytes],
                    compressed_candidate: None,
                    metadata,
                },
                correlation: ResourceCorrelation::Response(respond.request_id),
            },
            ResourceSegment {
                index: 1,
                total_segments,
                total_data_bytes,
            },
            &envelope[..envelope_len],
            now,
            fill_entropy,
            sink,
        );
        if total_segments > 1 {
            let metadata_packed_len = match metadata {
                ResourceMetadata::Packed(packed) => packed.len() as u32,
                ResourceMetadata::None | ResourceMetadata::SentInFirstSegment { .. } => 0,
            };
            let _ = self.outgoing_assemblies.set_static_continuation(
                &respond.link_id,
                StaticResponseContinuation {
                    command_id: id,
                    request_id: respond.request_id,
                    bytes: data,
                    next_offset: first_bytes,
                    next_segment_index: 2,
                    total_segments,
                    total_data_bytes,
                    metadata_packed_len,
                    segment_stream_bytes,
                },
            );
        }
        wake
    }

    pub(crate) fn continue_static_response_into<F>(
        &mut self,
        link_id: &LinkId,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let Some(mut continuation) = self.outgoing_assemblies.static_continuation(link_id) else {
            return crate::engine::WakeSchedules::UNCHANGED;
        };
        let end = continuation
            .next_offset
            .saturating_add(continuation.segment_stream_bytes)
            .min(continuation.bytes.len());
        if end <= continuation.next_offset
            || continuation.next_segment_index > continuation.total_segments
        {
            self.outgoing_assemblies.clear(link_id);
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id: continuation.command_id,
                settlement: Settlement::Respond(Err(crate::engine::RespondFailure::Resource(
                    SendResourceFailure::Sequencing,
                ))),
            }));
            return crate::engine::WakeSchedules::UNCHANGED;
        }
        let metadata = if continuation.metadata_packed_len == 0 {
            ResourceMetadata::None
        } else {
            ResourceMetadata::SentInFirstSegment {
                packed_len: continuation.metadata_packed_len,
            }
        };
        let wake = self.ingest_send_resource_segment_into(
            &ResourceSend {
                id: continuation.command_id,
                link_id: *link_id,
                body: ResourceBody {
                    data: &continuation.bytes[continuation.next_offset..end],
                    compressed_candidate: None,
                    metadata,
                },
                correlation: ResourceCorrelation::Response(continuation.request_id),
            },
            ResourceSegment {
                index: continuation.next_segment_index,
                total_segments: continuation.total_segments,
                total_data_bytes: continuation.total_data_bytes,
            },
            now,
            fill_entropy,
            sink,
        );
        continuation.next_offset = end;
        continuation.next_segment_index += 1;
        if self.outgoing_resources.is_empty() {
            self.outgoing_assemblies.clear(link_id);
        } else {
            let _ = self
                .outgoing_assemblies
                .set_static_continuation(link_id, continuation);
        }
        wake
    }

    /// Segment 1 of a split records its hash as the chain's `original_hash`; every later segment re-advertises it, so the host threads no hashes of its own.
    ///
    /// `total_data_bytes` is the whole transfer's uncompressed DATA length. The engine adds the metadata block on top, and RNS 1.4.2 advertises the sum (the `d` field) on every segment, never the segment's own size.
    ///
    /// A continuation whose live segment failed on the wire before this command reached the engine settles `PredecessorFailed` without advertising. A pipelining host therefore cannot revive a dead transfer's tail.
    pub fn ingest_send_resource_segment_into<F>(
        &mut self,
        send: &ResourceSend<'_>,
        segment: ResourceSegment,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        self.ingest_send_resource_segment_enveloped(send, segment, &[], now, fill_entropy, sink)
    }

    fn ingest_send_resource_segment_enveloped<F>(
        &mut self,
        send: &ResourceSend<'_>,
        segment: ResourceSegment,
        envelope: &[u8],
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let &ResourceSend {
            id,
            link_id,
            body,
            correlation,
        } = send;
        let ResourceSegment {
            index: segment_index,
            total_segments,
            total_data_bytes,
        } = segment;
        let data = body.data;
        let mut wake_schedule_changes = crate::engine::WakeSchedules::UNCHANGED;
        let settle = |sink: &mut dyn FnMut(EngineReaction<'_>), failure| {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id,
                settlement: resource_settlement(correlation, Err(failure)),
            }));
        };
        if segment_index == 0 || total_segments == 0 || segment_index > total_segments {
            settle(sink, SendResourceFailure::Sequencing);
            return wake_schedule_changes;
        }
        let Some(uncompressed_data_bytes) = u64::try_from(body.metadata.block_len())
            .ok()
            .and_then(|metadata_len| total_data_bytes.checked_add(metadata_len))
        else {
            settle(
                sink,
                SendResourceFailure::Rejected(SendResourceRejection::Build(
                    BuildOutgoingResourceError::DataTooLarge,
                )),
            );
            return wake_schedule_changes;
        };
        let metadata_placement_valid = match body.metadata {
            ResourceMetadata::None => true,
            ResourceMetadata::Packed(_) => segment_index == 1,
            ResourceMetadata::SentInFirstSegment { .. } => segment_index > 1,
        };
        if !metadata_placement_valid {
            settle(
                sink,
                SendResourceFailure::Rejected(SendResourceRejection::MetadataMisplaced),
            );
            return wake_schedule_changes;
        }
        let chain_is_dead =
            segment_index > 1 && self.outgoing_assemblies.original_hash(&link_id).is_none();
        if chain_is_dead {
            settle(sink, SendResourceFailure::PredecessorFailed);
            return wake_schedule_changes;
        }
        let (key, mtu, fire_on, rtt_millis) = match self.links.active_view(&link_id) {
            ActiveLinkLookup::Active(link) => (
                link.key,
                link.mtu,
                link.attached_interface,
                link.rtt.millis(),
            ),
            ActiveLinkLookup::Inactive => {
                settle(
                    sink,
                    SendResourceFailure::Rejected(SendResourceRejection::LinkNotActive),
                );
                return wake_schedule_changes;
            }
            ActiveLinkLookup::Absent => {
                settle(
                    sink,
                    SendResourceFailure::Rejected(SendResourceRejection::NoSuchLink),
                );
                return wake_schedule_changes;
            }
        };

        let sdu = resource_sdu(mtu);
        let reject = |sink: &mut dyn FnMut(EngineReaction<'_>),
                      error: TrackOutgoingResourceError| {
            let rejection = match error {
                TrackOutgoingResourceError::TableFull => SendResourceRejection::TableFull,
                TrackOutgoingResourceError::LinkBusy => SendResourceRejection::LinkBusy,
                TrackOutgoingResourceError::Build(build) => SendResourceRejection::Build(build),
            };
            settle(sink, SendResourceFailure::Rejected(rejection));
        };
        let lane = match self.outgoing_resources.lane_for(&link_id, &segment) {
            Ok(lane) => lane,
            Err(error) => {
                reject(sink, error);
                return wake_schedule_changes;
            }
        };

        let command = TrackedCommand {
            link_id,
            sdu,
            command_id: id,
            correlation,
            segment,
        };
        let raw_stages = lane == TrackLane::Staged
            && winning_candidate(body.compressed_candidate, envelope.len() + data.len()).is_none();
        let tracked = if raw_stages {
            let mut stream_nonce = [0u8; RESOURCE_NONCE_LEN];
            fill_entropy(&mut stream_nonce);
            self.outgoing_resources
                .stage_raw(
                    command,
                    body.metadata.travels(),
                    envelope.len() + data.len(),
                    |transfer| {
                        transfer[16..STAGED_STREAM_OFFSET].copy_from_slice(&stream_nonce);
                        let stream_start = STAGED_STREAM_OFFSET + envelope.len();
                        transfer[STAGED_STREAM_OFFSET..stream_start].copy_from_slice(envelope);
                        transfer[stream_start..stream_start + data.len()].copy_from_slice(data);
                    },
                )
                .map(RowLanding::Raw)
        } else {
            let shape = match outgoing_resource_buffer_shape(envelope.len(), &body, sdu) {
                Ok(shape) => shape,
                Err(error) => {
                    reject(sink, TrackOutgoingResourceError::Build(error));
                    return wake_schedule_changes;
                }
            };
            let mut seal_iv = [0u8; 16];
            fill_entropy(&mut seal_iv);
            self.outgoing_resources
                .track_built(command, lane, shape, |regions| {
                    build_outgoing_resource_enveloped(
                        envelope,
                        &body,
                        key,
                        &seal_iv,
                        || {
                            let mut nonce = [0u8; RESOURCE_NONCE_LEN];
                            fill_entropy(&mut nonce);
                            nonce
                        },
                        sdu,
                        regions,
                    )
                })
                .map(RowLanding::Built)
        };
        let landing = match tracked {
            Ok(landing) => landing,
            Err(error) => {
                reject(sink, error);
                return wake_schedule_changes;
            }
        };

        let chain_original = (segment_index > 1)
            .then(|| self.outgoing_assemblies.original_hash(&link_id))
            .flatten();
        let row_index = match landing {
            RowLanding::Built(hash) => self.outgoing_resources.lookup(&link_id, &hash),
            RowLanding::Raw(index) => Some(index),
        };
        if let Some(index) = row_index {
            let state = self.outgoing_resources.state_mut(index);
            state.uncompressed_data_bytes = uncompressed_data_bytes;
            if let Some(original) = chain_original {
                state.original_hash = original;
            }
        }
        if lane == TrackLane::Staged {
            return wake_schedule_changes;
        }
        let RowLanding::Built(hash) = landing else {
            return wake_schedule_changes;
        };
        if total_segments > 1 && segment_index == 1 {
            self.outgoing_assemblies.begin(link_id, hash);
        }

        let mut adv_iv = [0u8; 16];
        fill_entropy(&mut adv_iv);
        match emit_resource_advertisement(
            &self.outgoing_resources,
            &link_id,
            &hash,
            &AdvertisementLane { key, mtu, fire_on },
            &adv_iv,
            sink,
        ) {
            AdvertisementWriteOutcome::Wrote => {
                self.links.note_outbound(&link_id, now);
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
                if let Some(index) = self.outgoing_resources.lookup(&link_id, &hash) {
                    self.outgoing_resources.state_mut(index).retries_left =
                        MAX_ADVERTISEMENT_RETRIES;
                    self.outgoing_resources
                        .set_timeout_at(index, Some(advertised_deadline(now, rtt_millis)));
                }
                if let ResourceCorrelation::Request {
                    response_timeout,
                    maximum_response_bytes,
                    ..
                } = correlation
                {
                    if segment_index == 1 {
                        self.book_request_resource_receipt(
                            id,
                            &link_id,
                            data,
                            response_timeout,
                            maximum_response_bytes,
                            now,
                        );
                        wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                    }
                }
            }
            AdvertisementWriteOutcome::DidNotWrite => {
                self.outgoing_resources.remove(&link_id, &hash);
                settle(sink, SendResourceFailure::WriteFailed);
            }
        }
        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
        wake_schedule_changes
    }

    /// Note that RNS 1.4.2 `Transport.packet_filter` exempts `RESOURCE_REQ` from duplicate filtering because a receiver's retry is byte-identical by design.
    pub(crate) fn ingest_resource_request<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::DecryptFailed);
        };
        let plaintext: &'p [u8] = plaintext;
        let Ok(parsed) = parse_part_request_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        let advertised = self
            .outgoing_resources
            .lookup(&link_id, &parsed.hash)
            .is_some_and(|index| !self.outgoing_resources.state(index).status.is_staged());
        if !advertised {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        }
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::OwesResourceParts(ResourcePartRequest {
            link_id,
            hash: parsed.hash,
            requested: parsed.requested,
            last_known_map_hash: parsed.last_known_map_hash,
        })
    }

    /// RNS 1.4.2 `Resource.validate_proof`. Note `RESOURCE_PRF` is exempt from duplicate filtering, like the request.
    pub(crate) fn ingest_resource_proof(
        &mut self,
        link_id: LinkId,
        payload: &[u8],
        arrived_at: InstantMillis,
    ) -> ResourceProofClassification {
        use ResourceProofClassification::{NotALocalLink, Resolved};
        if self.links.phase_for(&link_id).is_none() {
            return NotALocalLink;
        }
        let Ok((hash, proof)) = parse_proof_plaintext(payload) else {
            return Resolved(IngestPacketOutcome::Ignored(IgnoreReason::Malformed));
        };
        let Some(index) = self.outgoing_resources.lookup(&link_id, &hash) else {
            return Resolved(IngestPacketOutcome::Ignored(IgnoreReason::Superseded));
        };
        let state = self.outgoing_resources.state(index);
        if state.status.is_staged() {
            return Resolved(IngestPacketOutcome::Ignored(IgnoreReason::Superseded));
        }
        if proof != state.expected_proof {
            return Resolved(IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid));
        }
        let id = state.command_id;
        let correlation = state.correlation;
        let last_segment = state.segment_index >= state.total_segments;
        self.outgoing_resources.remove(&link_id, &hash);
        if last_segment {
            self.outgoing_assemblies.clear(&link_id);
        }
        self.links.note_inbound(&link_id, arrived_at);
        Resolved(IngestPacketOutcome::ResourceDelivered {
            id,
            link_id,
            correlation,
            last_segment,
        })
    }

    /// RNS 1.4.2 `Resource._rejected`; sealed, and behind the duplicate filter.
    pub(crate) fn ingest_resource_receiver_cancel<'p>(
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
        let Some(index) = self.outgoing_resources.lookup(&link_id, &hash) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        };
        if self.outgoing_resources.state(index).status.is_staged() {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        }
        let state = self.outgoing_resources.state(index);
        let id = state.command_id;
        let correlation = state.correlation;
        self.outgoing_resources.remove(&link_id, &hash);
        self.outgoing_assemblies.clear(&link_id);
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::ResourceRejectedByPeer {
            id,
            link_id,
            correlation,
        }
    }

    /// RNS 1.4.2 `Resource.request`: parts go back raw (slices of the sealed stream,  no token around them).
    ///
    /// A request that breaks the segment sequencing cancels the transfer as the reference does, except we settle the command with the failure's name.
    pub(crate) fn serve_resource_request<F>(
        &mut self,
        request: &ResourcePartRequest<'_>,
        fire_on: InterfaceId,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let &ResourcePartRequest {
            ref link_id,
            ref hash,
            requested,
            last_known_map_hash,
        } = request;
        let Some(index) = self.outgoing_resources.lookup(link_id, hash) else {
            return;
        };
        let ActiveLinkLookup::Active(link) = self.links.active_view(link_id) else {
            return;
        };
        let key = link.key;
        let mtu = link.mtu;
        let rtt_millis = link.rtt.millis();
        {
            let state = self.outgoing_resources.state_mut(index);
            if state.status == OutgoingResourceStatus::Advertised {
                state.status = OutgoingResourceStatus::Transferring;
                state.retries_left = PART_REQUEST_MAX_RETRIES;
            }
        }
        self.outgoing_resources
            .set_timeout_at(index, Some(transferring_deadline(now, rtt_millis)));

        let scope_start = self.outgoing_resources.state(index).scope_start;
        let mut originated_outbound = false;
        for part in serve_part_indices(
            self.outgoing_resources.names_flat(index),
            scope_start,
            requested,
        ) {
            let outgoing = &self.outgoing_resources;
            let sdu = outgoing.state(index).sdu;
            let sealed_len = outgoing.sealed_transfer(index).len();
            let start = part * sdu;
            let end = (start + sdu).min(sealed_len);
            let mut fill = |slot: &mut [u8]| -> Option<usize> {
                let sealed = outgoing.sealed_transfer(index);
                write_link_raw_packet(
                    link_id,
                    PacketType::Data,
                    WireContext::Resource,
                    mtu,
                    &sealed[start..end],
                    slot,
                )
                .ok()
            };
            sink(EngineReaction::Directive(Directive::EmitFrame {
                target: fire_on,
                size_hint: link_raw_frame_ceiling(end - start),
                fill: &mut fill,
            }));
            if self.outgoing_resources.mark_sent(index, part) == PartSendOutcome::FirstSend {
                originated_outbound = true;
            }
        }

        if let Some(last_known) = last_known_map_hash {
            let plan = plan_hashmap_update(
                self.outgoing_resources.names_flat(index),
                scope_start,
                &last_known,
            );
            match plan {
                Ok(plan) => {
                    self.outgoing_resources.state_mut(index).scope_start = plan.scope_start;
                    let mut iv = [0u8; 16];
                    fill_entropy(&mut iv);
                    let outgoing = &self.outgoing_resources;
                    let mut wrote = false;
                    {
                        let mut fill = |slot: &mut [u8]| -> Option<usize> {
                            let names = outgoing.names_flat(index);
                            let segment_names = &names[plan.entries_start * MAP_HASH_LEN
                                ..plan.entries_end * MAP_HASH_LEN];
                            let mut plaintext = [0u8; LINK_MDU];
                            let plaintext_len = write_hashmap_update_plaintext(
                                hash,
                                plan.segment,
                                segment_names,
                                &mut plaintext,
                            )
                            .ok()?;
                            let wire_bytes = write_link_packet(
                                link_id,
                                key,
                                mtu,
                                WireContext::ResourceHashUpdate,
                                &plaintext[..plaintext_len],
                                &iv,
                                slot,
                            )
                            .ok()?;
                            wrote = true;
                            Some(wire_bytes)
                        };
                        sink(EngineReaction::Directive(Directive::EmitFrame {
                            target: fire_on,
                            size_hint: link_data_frame_ceiling(LINK_MDU),
                            fill: &mut fill,
                        }));
                    }
                    if wrote {
                        originated_outbound = true;
                    }
                }
                Err(_) => {
                    self.cancel_outgoing_resource(
                        link_id,
                        hash,
                        SendResourceFailure::Sequencing,
                        now,
                        fill_entropy,
                        sink,
                    );
                    return;
                }
            }
        }

        if originated_outbound {
            self.links.note_outbound(link_id, now);
        }

        let state = self.outgoing_resources.state_mut(index);
        if state.sent_part_count == state.part_count {
            state.status = OutgoingResourceStatus::AwaitingProof;
            state.retries_left = AWAITING_PROOF_RETRIES;
            self.outgoing_resources
                .set_timeout_at(index, Some(awaiting_proof_deadline(now, rtt_millis)));
        }
    }

    /// The link whose staged continuation owes its deferred seal: the segment ahead of it has served every part and awaits only the proof, so the receiver is busy verifying and this is the window the seal was deferred into.
    /// The manifold drains this only after yielding because it shares its thread with the interface writers. The served parts must flush to the wire ahead of a multi-millisecond seal.
    pub fn owed_staged_seal_link(&self) -> Option<LinkId> {
        (0..self.outgoing_resources.len()).find_map(|index| {
            let state = self.outgoing_resources.state(index);
            if state.status != OutgoingResourceStatus::Staged {
                return None;
            }
            let link_id = *self.outgoing_resources.link_at(index);
            let predecessor_index = state.segment_index - 1;
            let predecessor_awaits_proof = (0..self.outgoing_resources.len()).any(|sibling| {
                let sibling_state = self.outgoing_resources.state(sibling);
                self.outgoing_resources.link_at(sibling) == &link_id
                    && sibling_state.segment_index == predecessor_index
                    && sibling_state.status == OutgoingResourceStatus::AwaitingProof
            });
            predecessor_awaits_proof.then_some(link_id)
        })
    }

    /// The deferred seal, run the moment the live segment's last part is served: the receiver spends the next stretch ingesting and verifying, so the continuation's seal rides that window instead of sitting on the advertise path.
    pub fn seal_staged_continuation<F>(
        &mut self,
        link_id: &LinkId,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let Some(index) = self.outgoing_resources.staged_index(link_id) else {
            return;
        };
        let state = self.outgoing_resources.state(index);
        if state.status != OutgoingResourceStatus::Staged {
            return;
        }
        let nonce_prefixed_bytes = state.staged_plaintext_bytes;
        let sdu = state.sdu;
        let ActiveLinkLookup::Active(link) = self.links.active_view(link_id) else {
            return;
        };
        let key = link.key;
        let mut seal_iv = [0u8; 16];
        fill_entropy(&mut seal_iv);
        let sealed = seal_staged_resource(
            key,
            &seal_iv,
            || {
                let mut salt = [0u8; RESOURCE_NONCE_LEN];
                fill_entropy(&mut salt);
                salt
            },
            sdu,
            nonce_prefixed_bytes,
            self.outgoing_resources.seal_regions_mut(index),
        );
        match sealed {
            Ok(sealed) => self.record_staged_seal(index, &sealed),
            Err(error) => self.fail_staged_seal(index, link_id, error, sink),
        }
    }

    fn record_staged_seal(&mut self, index: usize, sealed: &SealedStagedResource) {
        self.outgoing_resources.set_hash(index, sealed.hash);
        let state = self.outgoing_resources.state_mut(index);
        state.sealed_transfer_bytes = sealed.sealed_transfer_bytes;
        state.part_count = sealed.part_count;
        state.salt_nonce = sealed.salt_nonce;
        state.expected_proof = sealed.expected_proof;
        state.staged_plaintext_bytes = 0;
        state.status = OutgoingResourceStatus::StagedSealed;
    }

    fn fail_staged_seal(
        &mut self,
        index: usize,
        link_id: &LinkId,
        error: BuildOutgoingResourceError,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let state = self.outgoing_resources.state(index);
        let id = state.command_id;
        let correlation = state.correlation;
        let hash = *self.outgoing_resources.hash_at(index);
        self.outgoing_resources.remove(link_id, &hash);
        sink(EngineReaction::Journaled(Journaled::CommandSettled {
            id,
            settlement: resource_settlement(
                correlation,
                Err(SendResourceFailure::Rejected(SendResourceRejection::Build(
                    error,
                ))),
            ),
        }));
    }

    /// The owed seal's worker inputs, borrowed for the manifold to copy into a crypto-pool job; [`mark_staged_sealing`](Self::mark_staged_sealing) then parks the row until the verdict.
    pub fn staged_seal_job_view(&self, link_id: &LinkId) -> Option<StagedSealJobView<'_>> {
        let index = self.outgoing_resources.staged_index(link_id)?;
        let state = self.outgoing_resources.state(index);
        if state.status != OutgoingResourceStatus::Staged {
            return None;
        }
        let ActiveLinkLookup::Active(link) = self.links.active_view(link_id) else {
            return None;
        };
        Some(StagedSealJobView {
            key: link.key,
            sdu: state.sdu,
            nonce_prefixed_bytes: state.staged_plaintext_bytes,
            plaintext: self.outgoing_resources.staged_plaintext(index),
        })
    }

    pub fn mark_staged_sealing(&mut self, link_id: &LinkId) {
        let Some(index) = self.outgoing_resources.staged_index(link_id) else {
            return;
        };
        let state = self.outgoing_resources.state_mut(index);
        if state.status == OutgoingResourceStatus::Staged {
            state.status = OutgoingResourceStatus::StagedSealing;
        }
    }

    /// A pool worker's seal verdict lands on the row only if it still matches the job's stream nonce and length; a row that died or was replaced meanwhile drops the verdict silently.
    pub fn apply_offloaded_staged_seal(
        &mut self,
        verdict: OffloadedStagedSeal<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let OffloadedStagedSeal {
            link_id,
            stream_nonce,
            nonce_prefixed_bytes,
            sealed_bytes,
            names,
            outcome,
        } = verdict;
        let matching = (0..self.outgoing_resources.len()).find(|&index| {
            let state = self.outgoing_resources.state(index);
            self.outgoing_resources.link_at(index) == &link_id
                && state.status == OutgoingResourceStatus::StagedSealing
                && state.staged_plaintext_bytes == nonce_prefixed_bytes
                && self.outgoing_resources.staged_plaintext(index)[16..16 + RESOURCE_NONCE_LEN]
                    == stream_nonce
        });
        let Some(index) = matching else {
            return;
        };
        match outcome {
            Ok(sealed) => {
                let regions = self.outgoing_resources.seal_regions_mut(index);
                if regions.transfer.len() < sealed_bytes.len()
                    || regions.hashmap.len() < names.len()
                {
                    self.fail_staged_seal(
                        index,
                        &link_id,
                        BuildOutgoingResourceError::HashmapBufferTooShort,
                        sink,
                    );
                    return;
                }
                regions.transfer[..sealed_bytes.len()].copy_from_slice(sealed_bytes);
                regions.hashmap[..names.len()].copy_from_slice(names);
                self.record_staged_seal(index, &sealed);
            }
            Err(error) => self.fail_staged_seal(index, &link_id, error, sink),
        }
    }

    /// The staged continuation's advertisement, owed since its build and released by the live segment's proof.
    /// Runs in the same inbound pass as the proof settle, so the receiver sees the next advertisement exactly where the reference's sender would first build it.
    pub fn promote_staged_resource<F>(
        &mut self,
        link_id: &LinkId,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let link_occupied = (0..self.outgoing_resources.len()).any(|index| {
            self.outgoing_resources.link_at(index) == link_id
                && !self.outgoing_resources.state(index).status.is_staged()
        });
        if link_occupied {
            return;
        }
        let Some(index) = self.outgoing_resources.staged_index(link_id) else {
            return;
        };
        if self.outgoing_resources.state(index).status == OutgoingResourceStatus::Staged {
            self.seal_staged_continuation(link_id, fill_entropy, sink);
        }
        let Some(index) = self.outgoing_resources.staged_index(link_id) else {
            return;
        };
        if self.outgoing_resources.state(index).status != OutgoingResourceStatus::StagedSealed {
            return;
        }
        let hash = *self.outgoing_resources.hash_at(index);
        let ActiveLinkLookup::Active(link) = self.links.active_view(link_id) else {
            self.fail_staged_continuation(link_id, sink);
            return;
        };
        let key = link.key;
        let mtu = link.mtu;
        let fire_on = link.attached_interface;
        let rtt_millis = link.rtt.millis();
        self.outgoing_resources.state_mut(index).status = OutgoingResourceStatus::Advertised;
        let mut adv_iv = [0u8; 16];
        fill_entropy(&mut adv_iv);
        match emit_resource_advertisement(
            &self.outgoing_resources,
            link_id,
            &hash,
            &AdvertisementLane { key, mtu, fire_on },
            &adv_iv,
            sink,
        ) {
            AdvertisementWriteOutcome::Wrote => {
                self.links.note_outbound(link_id, now);
                self.outgoing_resources.state_mut(index).retries_left = MAX_ADVERTISEMENT_RETRIES;
                self.outgoing_resources
                    .set_timeout_at(index, Some(advertised_deadline(now, rtt_millis)));
            }
            AdvertisementWriteOutcome::DidNotWrite => {
                let state = self.outgoing_resources.state(index);
                let id = state.command_id;
                let correlation = state.correlation;
                self.outgoing_resources.remove(link_id, &hash);
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: resource_settlement(
                        correlation,
                        Err(SendResourceFailure::WriteFailed),
                    ),
                }));
            }
        }
    }

    /// A staged continuation dies with whatever killed the segment ahead of it; nothing rides the wire because nothing was ever advertised.
    /// Drains every staged row because a follower can wait behind a still-sealing row, and both fall together.
    pub(crate) fn fail_staged_continuation(
        &mut self,
        link_id: &LinkId,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        while let Some(index) = self.outgoing_resources.staged_index(link_id) {
            let state = self.outgoing_resources.state(index);
            let id = state.command_id;
            let correlation = state.correlation;
            let hash = *self.outgoing_resources.hash_at(index);
            self.outgoing_resources.remove(link_id, &hash);
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id,
                settlement: resource_settlement(
                    correlation,
                    Err(SendResourceFailure::PredecessorFailed),
                ),
            }));
        }
    }

    /// RNS 1.4.2 `Resource.cancel`
    pub(crate) fn cancel_outgoing_resource<F>(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        failure: SendResourceFailure,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let Some(index) = self.outgoing_resources.lookup(link_id, hash) else {
            return;
        };
        let state = self.outgoing_resources.state(index);
        let id = state.command_id;
        let correlation = state.correlation;
        self.outgoing_resources.remove(link_id, hash);
        self.outgoing_assemblies.clear(link_id);
        if let ActiveLinkLookup::Active(link) = self.links.active_view(link_id) {
            let key = link.key;
            let mtu = link.mtu;
            let fire_on = link.attached_interface;
            let mut cancel_iv = [0u8; 16];
            fill_entropy(&mut cancel_iv);
            let mut cancel_plaintext = [0u8; RESOURCE_HASH_LEN];
            if write_cancel_plaintext(hash, &mut cancel_plaintext).is_ok() {
                let mut wrote = false;
                {
                    let mut fill = |slot: &mut [u8]| -> Option<usize> {
                        let wire_bytes = write_link_packet(
                            link_id,
                            key,
                            mtu,
                            WireContext::ResourceInitiatorCancel,
                            &cancel_plaintext,
                            &cancel_iv,
                            slot,
                        )
                        .ok()?;
                        wrote = true;
                        Some(wire_bytes)
                    };
                    sink(EngineReaction::Directive(Directive::EmitFrame {
                        target: fire_on,
                        size_hint: link_data_frame_ceiling(RESOURCE_HASH_LEN),
                        fill: &mut fill,
                    }));
                }
                if wrote {
                    self.links.note_outbound(link_id, now);
                }
            }
        }
        sink(EngineReaction::Journaled(Journaled::CommandSettled {
            id,
            settlement: resource_settlement(correlation, Err(failure)),
        }));
        self.fail_staged_continuation(link_id, sink);
    }

    /// RNS 1.4.2's watchdog states, held as deadlines on the register.
    ///
    /// The reference also fires `Transport.cache_request` for a missing proof, but packet caching is disabled there: (`TODO: Enable when caching has been redesigned`), so that request recovers nothing: our retry-then-cancel is equivalent, minus the dead packet.
    pub(crate) fn fire_due_outgoing_resources<F>(
        &mut self,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        while let Some(index) = self.outgoing_resources.due_index(now) {
            self.retry_or_cancel_outgoing_resource(index, now, fill_entropy, sink);
        }
    }

    fn retry_or_cancel_outgoing_resource<F>(
        &mut self,
        index: usize,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let link_id = *self.outgoing_resources.link_at(index);
        let hash = *self.outgoing_resources.hash_at(index);
        let state = *self.outgoing_resources.state(index);
        let ActiveLinkLookup::Active(link) = self.links.active_view(&link_id) else {
            self.cancel_outgoing_resource(
                &link_id,
                &hash,
                SendResourceFailure::Timeout,
                now,
                fill_entropy,
                sink,
            );
            return;
        };
        let key = link.key;
        let mtu = link.mtu;
        let fire_on = link.attached_interface;
        let rtt_millis = link.rtt.millis();
        match state.status {
            OutgoingResourceStatus::Staged
            | OutgoingResourceStatus::StagedSealing
            | OutgoingResourceStatus::StagedSealed => {
                self.fail_staged_continuation(&link_id, sink);
            }
            OutgoingResourceStatus::Advertised => {
                if state.retries_left == 0 {
                    self.cancel_outgoing_resource(
                        &link_id,
                        &hash,
                        SendResourceFailure::Timeout,
                        now,
                        fill_entropy,
                        sink,
                    );
                    return;
                }
                let mut adv_iv = [0u8; 16];
                fill_entropy(&mut adv_iv);
                if matches!(
                    emit_resource_advertisement(
                        &self.outgoing_resources,
                        &link_id,
                        &hash,
                        &AdvertisementLane { key, mtu, fire_on },
                        &adv_iv,
                        sink,
                    ),
                    AdvertisementWriteOutcome::Wrote
                ) {
                    self.links.note_outbound(&link_id, now);
                }
                let state = self.outgoing_resources.state_mut(index);
                state.retries_left -= 1;
                self.outgoing_resources
                    .set_timeout_at(index, Some(advertised_deadline(now, rtt_millis)));
            }
            OutgoingResourceStatus::Transferring => {
                self.cancel_outgoing_resource(
                    &link_id,
                    &hash,
                    SendResourceFailure::Timeout,
                    now,
                    fill_entropy,
                    sink,
                );
            }
            OutgoingResourceStatus::AwaitingProof => {
                if state.retries_left == 0 {
                    self.cancel_outgoing_resource(
                        &link_id,
                        &hash,
                        SendResourceFailure::Timeout,
                        now,
                        fill_entropy,
                        sink,
                    );
                    return;
                }
                let state = self.outgoing_resources.state_mut(index);
                state.retries_left -= 1;
                self.outgoing_resources
                    .set_timeout_at(index, Some(awaiting_proof_deadline(now, rtt_millis)));
            }
        }
    }
}

fn advertised_deadline(now: InstantMillis, rtt_millis: u64) -> InstantMillis {
    InstantMillis(
        now.0
            .saturating_add(rtt_millis.saturating_mul(LINK_TRAFFIC_TIMEOUT_FACTOR))
            .saturating_add(PROCESSING_GRACE_MS),
    )
}

/// RNS 1.4.2's sender-side transferring wait: one fat deadline re-armed on each request, after which the receiver is gone.
fn transferring_deadline(now: InstantMillis, rtt_millis: u64) -> InstantMillis {
    let retry_rtts = rtt_millis
        .saturating_mul(LINK_TRAFFIC_TIMEOUT_FACTOR)
        .saturating_mul(PART_REQUEST_MAX_RETRIES as u64);
    let max_extra_wait = PER_RETRY_DELAY_MS
        * ((PART_REQUEST_MAX_RETRIES as u64) * (PART_REQUEST_MAX_RETRIES as u64 + 1) / 2);
    InstantMillis(
        now.0
            .saturating_add(retry_rtts)
            .saturating_add(SENDER_GRACE_MS)
            .saturating_add(max_extra_wait),
    )
}

fn awaiting_proof_deadline(now: InstantMillis, rtt_millis: u64) -> InstantMillis {
    InstantMillis(
        now.0
            .saturating_add(rtt_millis.saturating_mul(PROOF_TIMEOUT_FACTOR))
            .saturating_add(SENDER_GRACE_MS),
    )
}

struct AdvertisementLane<'a> {
    key: &'a crate::routing::links::LinkKey,
    mtu: usize,
    fire_on: InterfaceId,
}

enum AdvertisementWriteOutcome {
    Wrote,
    DidNotWrite,
}

fn emit_resource_advertisement<C>(
    outgoing: &crate::routing::links::resources::table::OutgoingResources<C>,
    link_id: &LinkId,
    hash: &ResourceHash,
    lane: &AdvertisementLane<'_>,
    adv_iv: &[u8; 16],
    sink: &mut impl FnMut(EngineReaction<'_>),
) -> AdvertisementWriteOutcome
where
    C: crate::routing::links::resources::table::ResourceTable<
        crate::routing::links::resources::table::OutgoingResourceState,
    >,
{
    let mut outcome = AdvertisementWriteOutcome::DidNotWrite;
    let mut fill = |slot: &mut [u8]| -> Option<usize> {
        let index = outgoing.lookup(link_id, hash)?;
        let state = outgoing.state(index);
        let names = outgoing.names_flat(index);
        let first_segment = &names[..names.len().min(HASHMAP_MAX_LEN * MAP_HASH_LEN)];
        let advertisement = ResourceAdvertisement {
            transfer_bytes: state.sealed_transfer_bytes as u64,
            data_bytes: state.uncompressed_data_bytes,
            part_count: state.part_count as u64,
            hash: *hash,
            salt_nonce: state.salt_nonce,
            original_hash: state.original_hash,
            segment_index: state.segment_index,
            total_segments: state.total_segments,
            request_id: state.correlation.request_id(),
            flags: ResourceFlags {
                encrypted: true,
                compressed: state.compression.wire_flag(),
                split: state.total_segments > 1,
                is_request: state.correlation.is_request(),
                is_response: state.correlation.is_response(),
                has_metadata: state.has_metadata,
            },
            hashmap: first_segment,
        };
        let mut plaintext = [0u8; LINK_MDU];
        let plaintext_len = advertisement.write(&mut plaintext).ok()?;
        let wire_bytes = write_link_packet(
            link_id,
            lane.key,
            lane.mtu,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            adv_iv,
            slot,
        )
        .ok()?;
        outcome = AdvertisementWriteOutcome::Wrote;
        Some(wire_bytes)
    };
    sink(EngineReaction::Directive(Directive::EmitFrame {
        target: lane.fire_on,
        size_hint: link_data_frame_ceiling(LINK_MDU),
        fill: &mut fill,
    }));
    outcome
}

/// RNS 1.4.2 `Resource.request`: `retries_left = 3` once every part has been sent and only the proof is owed.
const AWAITING_PROOF_RETRIES: u8 = 3;

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use super::*;
    use crate::crypto::{x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};
    use crate::crypto::{BufferTooShort, Ed25519PublicKey, Ed25519SecretKey};
    use crate::engine::test_support::{filled_frame, TestStorageLayout};
    use crate::engine::CommandId;
    use crate::engine::IngestIo;
    use crate::engine::InstantMillis;
    use crate::interfaces::AttachedInterfaces;
    use crate::interfaces::InterfaceId;
    use crate::routing::links::resources::build_outgoing::BuildOutgoingResourceError;
    use crate::routing::links::resources::table::OutgoingResourceStatus;
    use crate::routing::links::resources::{ResourceBody, ResourceCorrelation, ResourceMetadata};
    use crate::routing::links::table::InitiatedLink;
    use crate::routing::links::table::LinkActivation;
    use crate::routing::links::{LinkKey, LinkMode};
    use crate::wire::{PacketType, WirePacketHeader, BROADCAST_MTU};

    fn bytes_from_hex(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    const INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    const RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";

    pub(crate) fn link_id() -> LinkId {
        LinkId::new(bytes_from_hex(LINK_ID).try_into().unwrap())
    }

    pub(crate) fn link_key() -> LinkKey {
        let scalar: [u8; 32] = bytes_from_hex(INITIATOR_SCALAR).try_into().unwrap();
        let public: [u8; 32] = bytes_from_hex(RESPONDER_PUBLIC).try_into().unwrap();
        let shared = x25519_diffie_hellman(&X25519SecretKey::new(scalar), &X25519PublicKey(public));
        LinkKey::derive(&link_id(), &shared)
    }

    fn lane() -> InterfaceId {
        InterfaceId::new([0xEE; 8])
    }

    fn install_active_link<S: StorageLayout>(engine: &mut EngineState<S>) {
        engine
            .links
            .track_initiated(InitiatedLink {
                link_id: link_id(),
                destination: DestinationHash::new([0x77; 16]),
                route_evidence: crate::routing::routes::RouteEvidenceHandle::new(
                    crate::routing::routes::RouteEvidenceId::FIRST,
                    0,
                ),
                expected_hops: 1,
                mode: LinkMode::Aes256Cbc,
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
                    received_hops: 1,
                    rtt: crate::units::RttMillis::new(250),
                    mtu: BROADCAST_MTU,
                    attached_interface: lane(),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(1_000),
            )
            .unwrap();
    }

    pub(crate) fn sender_with_active_link() -> EngineState<TestStorageLayout> {
        let mut engine = EngineState::<TestStorageLayout>::default();
        install_active_link(&mut engine);
        engine
    }

    /// Staging needs a second outgoing row, which the deliberately tight fixed test layout does not carry; the heap layout is the shape every staging host actually runs.
    pub(crate) fn heap_sender_with_active_link() -> EngineState<crate::storage::GrowableHeap> {
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        install_active_link(&mut engine);
        engine
    }

    pub(crate) struct SendCapture {
        pub(crate) frames: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
        pub(crate) settlements: std::vec::Vec<(CommandId, Settlement)>,
    }

    pub(crate) fn watch_capture<S: StorageLayout>(
        engine: &mut EngineState<S>,
        at: u64,
    ) -> SendCapture {
        let mut capture = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        engine.fire_due_resource_deadlines(
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xF1),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        capture
    }

    pub(crate) fn send<S: StorageLayout>(
        engine: &mut EngineState<S>,
        id: u64,
        data: &[u8],
        candidate: Option<&[u8]>,
    ) -> SendCapture {
        let mut capture = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        engine.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(id),
                link_id: link_id(),
                body: ResourceBody {
                    data,
                    compressed_candidate: candidate,
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        capture
    }

    pub(crate) fn send_segment<S: StorageLayout>(
        engine: &mut EngineState<S>,
        id: u64,
        data: &[u8],
        segment: ResourceSegment,
    ) -> SendCapture {
        send_segment_with_metadata(engine, id, data, segment, ResourceMetadata::None)
    }

    fn send_segment_with_metadata<S: StorageLayout>(
        engine: &mut EngineState<S>,
        id: u64,
        data: &[u8],
        segment: ResourceSegment,
        metadata: ResourceMetadata<'_>,
    ) -> SendCapture {
        let mut capture = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        engine.ingest_send_resource_segment_into(
            &ResourceSend {
                id: CommandId(id),
                link_id: link_id(),
                body: ResourceBody {
                    data,
                    compressed_candidate: None,
                    metadata,
                },
                correlation: ResourceCorrelation::Unsolicited,
            },
            segment,
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        capture
    }

    static CASE1_PLAINTEXT: LazyLock<std::vec::Vec<u8>> =
        LazyLock::new(|| b"reticulum resources ride the link ".repeat(40));
    static MULTI_SEGMENT_STATIC_RESPONSE: [u8; STATIC_RESPONSE_SEGMENT_BYTES * 2 + 31_337] =
        [0x42; STATIC_RESPONSE_SEGMENT_BYTES * 2 + 31_337];
    static REJECTED_STATIC_RESPONSE: [u8; STATIC_RESPONSE_SEGMENT_BYTES * 2 + 1] =
        [0x42; STATIC_RESPONSE_SEGMENT_BYTES * 2 + 1];
    static OVERSIZED_FIXED_LAYOUT_STATIC_RESPONSE: [u8; 12_000] = [0x42; 12_000];

    fn case1_plaintext() -> std::vec::Vec<u8> {
        CASE1_PLAINTEXT.clone()
    }

    const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";

    #[test]
    fn a_send_resource_seals_registers_and_advertises() {
        let mut engine = sender_with_active_link();
        let plaintext = case1_plaintext();
        let candidate = bytes_from_hex(CASE1_BZ2);
        let capture = send(&mut engine, 7, &plaintext, Some(&candidate));

        assert!(
            capture.settlements.is_empty(),
            "success settles at the proof"
        );
        assert_eq!(capture.frames.len(), 1);
        let (target, frame) = &capture.frames[0];
        assert_eq!(*target, lane());

        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(header.context, WireContext::ResourceAdvertisement);
        assert_eq!(header.address, link_id().to_address());

        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();

        let index = engine
            .outgoing_resources
            .lookup(&link_id(), &advertisement.hash)
            .expect("the advertised transfer is registered");
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.status, OutgoingResourceStatus::Advertised);
        assert_eq!(
            advertisement.transfer_bytes,
            state.sealed_transfer_bytes as u64
        );
        assert_eq!(advertisement.data_bytes, 1_360);
        assert_eq!(advertisement.part_count, 1);
        assert_eq!(advertisement.salt_nonce, state.salt_nonce);
        assert_eq!(advertisement.original_hash, advertisement.hash);
        assert_eq!(advertisement.segment_index, 1);
        assert_eq!(advertisement.total_segments, 1);
        assert_eq!(advertisement.request_id, None);
        assert!(advertisement.flags.encrypted);
        assert!(advertisement.flags.compressed);
        assert!(!advertisement.flags.is_response);
        assert_eq!(
            advertisement.hashmap,
            engine.outgoing_resources.names_flat(index),
        );
    }

    #[test]
    fn malformed_segment_coordinates_settle_without_tracking_state() {
        let mut engine = sender_with_active_link();
        let cases = [
            ResourceSegment {
                index: 0,
                total_segments: 1,
                total_data_bytes: 4,
            },
            ResourceSegment {
                index: 1,
                total_segments: 0,
                total_data_bytes: 4,
            },
            ResourceSegment {
                index: 2,
                total_segments: 1,
                total_data_bytes: 4,
            },
        ];
        for (offset, segment) in cases.into_iter().enumerate() {
            let id = 20 + u64::try_from(offset).unwrap();
            let capture = send_segment(&mut engine, id, b"data", segment);
            assert!(capture.frames.is_empty());
            assert!(matches!(
                capture.settlements.as_slice(),
                [(settled_id, Settlement::SendResource(Err(SendResourceFailure::Sequencing)))]
                    if *settled_id == CommandId(id)
            ));
            assert!(engine.outgoing_resources.is_empty());
        }
    }

    #[test]
    fn an_unrepresentable_advertised_data_length_is_rejected_before_tracking() {
        let mut engine = sender_with_active_link();
        let metadata = [0x81];
        let capture = send_segment_with_metadata(
            &mut engine,
            30,
            b"data",
            ResourceSegment {
                index: 1,
                total_segments: 1,
                total_data_bytes: u64::MAX,
            },
            ResourceMetadata::Packed(&metadata),
        );
        assert!(capture.frames.is_empty());
        assert!(matches!(
            capture.settlements.as_slice(),
            [(
                CommandId(30),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceRejection::Build(BuildOutgoingResourceError::DataTooLarge),
                ))),
            )]
        ));
        assert!(engine.outgoing_resources.is_empty());
    }

    #[test]
    fn one_resource_per_link_rejects_the_second_send() {
        let mut engine = sender_with_active_link();
        let plaintext = case1_plaintext();
        send(&mut engine, 7, &plaintext, None);
        let second = send(&mut engine, 8, &plaintext, None);

        assert!(second.frames.is_empty());
        assert_eq!(second.settlements.len(), 1);
        assert!(matches!(
            second.settlements[0],
            (
                CommandId(8),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceRejection::LinkBusy,
                ))),
            ),
        ));
        assert_eq!(engine.outgoing_resources.len(), 1);
    }

    #[test]
    fn a_missing_or_inactive_link_rejects_by_name() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let capture = send(&mut engine, 7, b"data", None);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceRejection::NoSuchLink,
                ))),
            ),
        ));

        engine
            .links
            .track_initiated(InitiatedLink {
                link_id: link_id(),
                destination: DestinationHash::new([0x77; 16]),
                route_evidence: crate::routing::routes::RouteEvidenceHandle::new(
                    crate::routing::routes::RouteEvidenceId::FIRST,
                    0,
                ),
                expected_hops: 1,
                mode: LinkMode::Aes256Cbc,
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                link_signing: Ed25519SecretKey::new([0x33; 32]),
                requested_at: InstantMillis(500),
                timeout_at: InstantMillis(5_000),
                command_id: CommandId(1),
            })
            .unwrap();
        let capture = send(&mut engine, 8, b"data", None);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(8),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceRejection::LinkNotActive,
                ))),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }

    #[test]
    fn a_static_bytes_response_builds_byte_identical_to_a_prepacked_response() {
        use crate::engine::{Respond, RespondPayload};
        use crate::routing::links::request::{
            packed_binary_len, write_packed_binary_header, write_response_plaintext, RequestId,
            MAX_PACKED_BINARY_HEADER_LEN, RESPONSE_WIRE_OVERHEAD,
        };

        let page: &'static [u8] = CASE1_PLAINTEXT.as_slice();
        let request_id = RequestId([0x5A; 16]);

        let mut packed_page = std::vec![0u8; packed_binary_len(page.len()).unwrap()];
        let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
        let header_len = write_packed_binary_header(page.len(), &mut header).unwrap();
        packed_page[..header_len].copy_from_slice(&header[..header_len]);
        packed_page[header_len..].copy_from_slice(page);
        let mut prepacked = std::vec![0u8; RESPONSE_WIRE_OVERHEAD + packed_page.len()];
        let packed_len =
            write_response_plaintext(&request_id, &packed_page, &mut prepacked).unwrap();
        prepacked.truncate(packed_len);

        let mut host_lane = sender_with_active_link();
        let mut host_capture = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        host_lane.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &prepacked,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Response(request_id),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        host_capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    host_capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );

        let mut static_lane = sender_with_active_link();
        let mut static_capture = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        static_lane.ingest_send_static_response_into(
            CommandId(7),
            &Respond {
                link_id: link_id(),
                request_id,
                payload: RespondPayload::StaticBytes(page),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        static_capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    static_capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );

        assert!(host_capture.settlements.is_empty());
        assert!(static_capture.settlements.is_empty());
        assert_eq!(host_capture.frames.len(), 1);
        assert_eq!(host_capture.frames, static_capture.frames);

        let (_, frame) = &static_capture.frames[0];
        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.context, WireContext::ResourceAdvertisement);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();
        assert!(advertisement.flags.is_response);
        assert!(!advertisement.flags.compressed);
        assert_eq!(advertisement.request_id, Some(request_id));
        assert_eq!(
            advertisement.data_bytes,
            (RESPONSE_WIRE_OVERHEAD + packed_page.len()) as u64
        );
    }

    #[test]
    fn static_bytes_respond_picks_the_packet_rung_or_the_resource_rung() {
        use crate::engine::{CommandOutcome, Respond, RespondPayload};
        use crate::routing::links::request::{write_packed_binary_header, RequestId};

        let request_id = RequestId([0x5A; 16]);
        let engine = sender_with_active_link();

        let small: &'static [u8] = &[0xC4, 0x02, b'o', b'k'];
        match engine.ingest_respond(
            CommandId(1),
            Respond {
                link_id: link_id(),
                request_id,
                payload: RespondPayload::StaticBytes(small),
            },
        ) {
            CommandOutcome::OwesRespond { id, respond } => {
                assert_eq!(id, CommandId(1));
                assert_eq!(respond.link_id, link_id());
                assert_eq!(respond.request_id, request_id);
                let RespondPayload::Packed(data) = respond.payload else {
                    panic!("packet response should be packed");
                };
                let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
                let header_len = write_packed_binary_header(small.len(), &mut header).unwrap();
                assert_eq!(&data[..header_len], &header[..header_len]);
                assert_eq!(&data[header_len..], small);
            }
            other => panic!("expected the packet rung, got {other:?}"),
        }

        let big: &'static [u8] = CASE1_PLAINTEXT.as_slice();
        assert!(matches!(
            engine.ingest_respond(
                CommandId(2),
                Respond {
                    link_id: link_id(),
                    request_id,
                    payload: RespondPayload::StaticBytes(big),
                },
            ),
            CommandOutcome::OwesResourceResponse {
                id: CommandId(2),
                respond: Respond {
                    payload: RespondPayload::StaticBytes(data),
                    ..
                },
            } if core::ptr::eq(data, big)
        ));

        let linkless = EngineState::<TestStorageLayout>::default();
        assert!(matches!(
            linkless.ingest_respond(
                CommandId(3),
                Respond {
                    link_id: link_id(),
                    request_id,
                    payload: RespondPayload::StaticBytes(small),
                },
            ),
            CommandOutcome::RespondRejected {
                id: CommandId(3),
                ..
            }
        ));
    }

    #[test]
    fn a_static_file_advances_one_bounded_segment_per_proof() {
        use crate::engine::{Respond, RespondPayload};
        use crate::routing::links::request::RequestId;

        let bytes: &'static [u8] = &MULTI_SEGMENT_STATIC_RESPONSE;
        let request_id = RequestId([0xA7; 16]);
        let mut engine = heap_sender_with_active_link();
        let mut first = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        engine.ingest_send_static_response_into(
            CommandId(41),
            &Respond {
                link_id: link_id(),
                request_id,
                payload: RespondPayload::StaticFile {
                    name: "source.zip",
                    bytes,
                },
            },
            InstantMillis(1_500),
            &mut |entropy: &mut [u8]| entropy.fill(0xA5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        first.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    first.settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        assert!(first.settlements.is_empty());
        assert_eq!(first.frames.len(), 1);

        let mut current_hash = advertised_hash(&first.frames[0].1);
        let first_index = engine
            .outgoing_resources
            .lookup(&link_id(), &current_hash)
            .unwrap();
        let first_state = engine.outgoing_resources.state(first_index);
        assert!(first_state.has_metadata, "the filename travels as metadata");
        let mut transfer = engine
            .outgoing_resources
            .sealed_transfer(first_index)
            .to_vec();
        let opened = crate::routing::links::resources::assemble_incoming::open_transfer(
            &link_key(),
            &mut transfer,
        )
        .unwrap();
        let metadata_len = u32::from_be_bytes([0, opened[0], opened[1], opened[2]]) as usize;
        assert_eq!(
            crate::rncp::parse_file_metadata(&opened[3..3 + metadata_len]).unwrap(),
            b"source.zip"
        );
        assert!(
            opened[3 + metadata_len..].iter().all(|byte| *byte == 0x42),
            "a file response carries the bare file bytes with no envelope or binary header"
        );
        assert!(
            first_state.sealed_transfer_bytes <= engine.outgoing_resources.transfer_capacity(),
            "one segment fits the configured outgoing window"
        );
        assert!(
            engine
                .outgoing_assemblies
                .static_continuation(&link_id())
                .is_some(),
            "only offsets and the static source survive between segments"
        );

        let mut advertised_segments = 1u64;
        loop {
            let index = engine
                .outgoing_resources
                .lookup(&link_id(), &current_hash)
                .unwrap();
            let state = *engine.outgoing_resources.state(index);
            let capture = feed(
                &mut engine,
                &proof_frame(&current_hash, &state.expected_proof),
                2_000 + advertised_segments,
            );
            if state.segment_index == state.total_segments {
                assert!(matches!(
                    capture.settlements.as_slice(),
                    [(CommandId(41), Settlement::Respond(Ok(())))]
                ));
                assert!(capture.frames.is_empty());
                assert!(engine.outgoing_resources.is_empty());
                assert!(engine
                    .outgoing_assemblies
                    .static_continuation(&link_id())
                    .is_none());
                break;
            }

            assert!(
                capture.settlements.is_empty(),
                "intermediate proofs do not settle the response"
            );
            assert_eq!(
                capture.frames.len(),
                1,
                "exactly one next segment is advertised"
            );
            let (_, payload) = WirePacketHeader::parse(&capture.frames[0].1).unwrap();
            let mut sealed = payload.to_vec();
            let opened = link_key().open_in_place(&mut sealed).unwrap();
            let advertisement = ResourceAdvertisement::parse(opened).unwrap();
            advertised_segments += 1;
            assert_eq!(advertisement.segment_index, advertised_segments);
            assert_eq!(
                advertisement.original_hash,
                advertised_hash(&first.frames[0].1)
            );
            assert!(advertisement.flags.is_response);
            current_hash = advertisement.hash;
        }
        assert!(advertised_segments > 1);
    }

    #[test]
    fn a_rejected_static_segment_clears_the_remaining_file() {
        use crate::engine::{Respond, RespondPayload};
        use crate::routing::links::request::RequestId;

        let bytes: &'static [u8] = &REJECTED_STATIC_RESPONSE;
        let mut engine = heap_sender_with_active_link();
        let mut first = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        engine.ingest_send_static_response_into(
            CommandId(43),
            &Respond {
                link_id: link_id(),
                request_id: RequestId([0xA9; 16]),
                payload: RespondPayload::StaticFile {
                    name: "source.zip",
                    bytes,
                },
            },
            InstantMillis(1_500),
            &mut |entropy: &mut [u8]| entropy.fill(0xA5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        first.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    first.settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        let live = advertised_hash(&first.frames[0].1);
        let rejected = feed(&mut engine, &receiver_cancel_frame(&live), 2_000);

        assert!(matches!(
            rejected.settlements.as_slice(),
            [(
                CommandId(43),
                Settlement::Respond(Err(RespondFailure::Resource(
                    SendResourceFailure::RejectedByPeer
                )))
            )]
        ));
        assert!(engine.outgoing_resources.is_empty());
        assert!(engine
            .outgoing_assemblies
            .static_continuation(&link_id())
            .is_none());
    }

    #[test]
    fn fixed_layouts_without_continuation_storage_reject_split_static_files() {
        use crate::engine::{Respond, RespondPayload};
        use crate::routing::links::request::RequestId;

        let bytes: &'static [u8] = &OVERSIZED_FIXED_LAYOUT_STATIC_RESPONSE;
        let mut engine = sender_with_active_link();
        let mut capture = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        engine.ingest_send_static_response_into(
            CommandId(42),
            &Respond {
                link_id: link_id(),
                request_id: RequestId([0xA8; 16]),
                payload: RespondPayload::StaticFile {
                    name: "source.zip",
                    bytes,
                },
            },
            InstantMillis(1_500),
            &mut |entropy: &mut [u8]| entropy.fill(0xA5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );

        assert!(capture.frames.is_empty());
        assert!(matches!(
            capture.settlements.as_slice(),
            [(
                CommandId(42),
                Settlement::Respond(Err(RespondFailure::Resource(
                    SendResourceFailure::Rejected(SendResourceRejection::Build(
                        BuildOutgoingResourceError::DataTooLarge
                    ))
                )))
            )]
        ));
        assert!(engine.outgoing_resources.is_empty());
        assert!(engine
            .outgoing_assemblies
            .original_hash(&link_id())
            .is_none());
    }

    pub(crate) struct InboundCapture {
        pub(crate) frames: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
        pub(crate) settlements: std::vec::Vec<(CommandId, Settlement)>,
    }

    pub(crate) fn feed<S: StorageLayout>(
        engine: &mut EngineState<S>,
        frame: &[u8],
        at: u64,
    ) -> InboundCapture {
        use crate::engine::test_support::routable_descriptor;
        use crate::interfaces::InboundPacket;
        let mut capture = InboundCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        let mut raw = frame.to_vec();
        engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(at),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[routable_descriptor(lane())]),
                now: InstantMillis(at),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                        if let Some(frame) = filled_frame(fill) {
                            capture.frames.push((target, frame));
                        }
                    }
                    EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                        capture.settlements.push((id, settlement));
                    }
                    _ => {}
                },
            },
        );
        capture
    }

    pub(crate) fn request_frame(
        hash: &ResourceHash,
        last_known: Option<&[u8; MAP_HASH_LEN]>,
        requested: &[u8],
    ) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::control::{
            write_part_request_plaintext, PART_REQUEST_PLAINTEXT_CAP,
        };
        let mut plaintext = [0u8; PART_REQUEST_PLAINTEXT_CAP];
        let plaintext_len =
            write_part_request_plaintext(hash, last_known, requested, &mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceRequest,
            &plaintext[..plaintext_len],
            &[0xC3; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_bytes].to_vec()
    }

    fn advertised_resource<S: StorageLayout>(
        engine: &mut EngineState<S>,
        data: &[u8],
    ) -> (ResourceHash, std::vec::Vec<u8>) {
        let capture = send(engine, 7, data, None);
        let (_, frame) = &capture.frames[0];
        let (_, payload) = WirePacketHeader::parse(frame).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();
        (advertisement.hash, advertisement.hashmap.to_vec())
    }

    fn four_part_payload() -> std::vec::Vec<u8> {
        b"resource parts ride raw on the wire! ".repeat(41)
    }

    #[test]
    fn requested_parts_stream_back_raw_from_the_register() {
        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let (hash, names) = advertised_resource(&mut engine, &data);

        let mut requested = std::vec::Vec::new();
        requested.extend_from_slice(&names[4..8]);
        requested.extend_from_slice(&names[12..16]);
        let capture = feed(&mut engine, &request_frame(&hash, None, &requested), 2_000);

        assert_eq!(capture.frames.len(), 2);
        let index = engine.outgoing_resources.lookup(&link_id(), &hash).unwrap();
        let sealed = engine.outgoing_resources.sealed_transfer(index);
        for ((target, frame), expected) in capture
            .frames
            .iter()
            .zip([&sealed[464..928], &sealed[1_392..]])
        {
            assert_eq!(*target, lane());
            let (header, payload) = WirePacketHeader::parse(frame).unwrap();
            assert_eq!(header.packet_type, PacketType::Data);
            assert_eq!(header.context, WireContext::Resource);
            assert_eq!(payload, expected, "the part is a raw sealed-stream slice");
        }
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.status, OutgoingResourceStatus::Transferring);
        assert_eq!(state.sent_part_count, 2);
        let mut observed = None;
        engine
            .links
            .reconcile_pending_route_evidence(|_, at| observed = Some(at));
        assert_eq!(
            observed,
            Some(InstantMillis(2_000)),
            "the valid inbound resource request is route evidence",
        );
    }

    #[test]
    fn serving_every_part_awaits_the_proof_and_resends_are_not_recounted() {
        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let (hash, names) = advertised_resource(&mut engine, &data);

        let first = feed(&mut engine, &request_frame(&hash, None, &names), 2_000);
        assert_eq!(first.frames.len(), 4);
        let index = engine.outgoing_resources.lookup(&link_id(), &hash).unwrap();
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.status, OutgoingResourceStatus::AwaitingProof);
        assert_eq!(state.sent_part_count, 4);
        assert_eq!(state.retries_left, 3);
        let Some(LinkPhase::Active { last_outbound, .. }) = engine.links.phase_for(&link_id())
        else {
            panic!("the resource sender's link must remain active");
        };
        assert_eq!(*last_outbound, InstantMillis(2_000));

        let again = feed(&mut engine, &request_frame(&hash, None, &names), 2_500);
        assert_eq!(
            again.frames.len(),
            4,
            "an identical retry passes the duplicate filter, like the reference exempts RESOURCE_REQ",
        );
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.sent_part_count, 4, "a resend is never recounted");
        let Some(LinkPhase::Active { last_outbound, .. }) = engine.links.phase_for(&link_id())
        else {
            panic!("the resource sender's link must remain active");
        };
        assert_eq!(
            *last_outbound,
            InstantMillis(2_000),
            "byte-identical resource part retransmissions do not refresh outbound liveness",
        );
    }

    #[test]
    fn a_request_for_an_unknown_transfer_is_ignored() {
        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let (_, names) = advertised_resource(&mut engine, &data);

        let unknown = ResourceHash::new([0x5A; 32]);
        let capture = feed(
            &mut engine,
            &request_frame(&unknown, None, &names[..4]),
            2_000,
        );
        assert!(capture.frames.is_empty());
        assert!(capture.settlements.is_empty());
    }

    #[test]
    fn an_exhausted_request_earns_the_next_hashmap_segment() {
        use crate::routing::links::resources::advertisement::parse_hashmap_update_plaintext;
        use crate::storage::GrowableHeap;

        let mut engine = EngineState::<GrowableHeap>::default();
        install_active_link(&mut engine);
        let data = std::vec![0x42u8; 100 * 464 - 100];
        let (hash, names) = advertised_resource(&mut engine, &data);
        assert_eq!(
            names.len(),
            74 * MAP_HASH_LEN,
            "the advertisement carries one segment"
        );

        let last_known: [u8; 4] = names[73 * 4..74 * 4].try_into().unwrap();
        let capture = feed(
            &mut engine,
            &request_frame(&hash, Some(&last_known), &names[72 * 4..74 * 4]),
            2_000,
        );

        assert_eq!(capture.frames.len(), 3, "two parts and the hashmap update");
        let (_, hmu_frame) = &capture.frames[2];
        let (header, payload) = WirePacketHeader::parse(hmu_frame).unwrap();
        assert_eq!(header.context, WireContext::ResourceHashUpdate);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let update = parse_hashmap_update_plaintext(opened).unwrap();
        assert_eq!(update.hash, hash);
        assert_eq!(update.segment, 1);

        let index = engine.outgoing_resources.lookup(&link_id(), &hash).unwrap();
        assert_eq!(
            update.hashmap,
            &engine.outgoing_resources.names_flat(index)[74 * MAP_HASH_LEN..],
            "the update carries every name past the first segment",
        );
        assert_eq!(engine.outgoing_resources.state(index).scope_start, 0);
    }

    #[test]
    fn a_sequencing_break_cancels_the_transfer_by_name() {
        use crate::storage::GrowableHeap;

        let mut engine = EngineState::<GrowableHeap>::default();
        install_active_link(&mut engine);
        let data = std::vec![0x42u8; 100 * 464 - 100];
        let (hash, names) = advertised_resource(&mut engine, &data);

        let off_boundary: [u8; 4] = names[10 * 4..11 * 4].try_into().unwrap();
        let capture = feed(
            &mut engine,
            &request_frame(&hash, Some(&off_boundary), &[]),
            2_000,
        );

        assert_eq!(capture.frames.len(), 1, "the cancel rides to the receiver");
        let (_, cancel) = &capture.frames[0];
        let (header, payload) = WirePacketHeader::parse(cancel).unwrap();
        assert_eq!(header.context, WireContext::ResourceInitiatorCancel);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        assert_eq!(
            crate::routing::links::resources::control::parse_cancel_plaintext(opened).unwrap(),
            hash,
        );
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Sequencing)),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }

    fn proof_frame(
        hash: &ResourceHash,
        proof: &crate::routing::links::resources::ResourceProof,
    ) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::control::write_proof_plaintext;
        let mut plaintext = [0u8; 64];
        write_proof_plaintext(hash, proof, &mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_raw_packet(
            &link_id(),
            PacketType::Proof,
            WireContext::ResourceProof,
            BROADCAST_MTU,
            &plaintext,
            &mut frame,
        )
        .unwrap();
        frame[..wire_bytes].to_vec()
    }

    #[test]
    fn the_receivers_proof_settles_the_send_and_retires_the_transfer() {
        use crate::routing::links::resources::assemble_incoming::{
            open_transfer, verify_and_prove,
        };

        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let capture = send(&mut engine, 7, &data, None);
        let (_, adv_frame) = &capture.frames[0];
        let (_, adv_payload) = WirePacketHeader::parse(adv_frame).unwrap();
        let mut sealed_adv = adv_payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed_adv).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();

        let serve = feed(
            &mut engine,
            &request_frame(&advertisement.hash, None, advertisement.hashmap),
            2_000,
        );
        let mut reassembled = std::vec::Vec::new();
        for (_, frame) in &serve.frames {
            let (_, part) = WirePacketHeader::parse(frame).unwrap();
            reassembled.extend_from_slice(part);
        }
        let plaintext = open_transfer(&link_key(), &mut reassembled).unwrap();
        assert_eq!(
            plaintext,
            &data[..],
            "the receiver assembles the original data"
        );
        let proof =
            verify_and_prove(plaintext, &advertisement.salt_nonce, &advertisement.hash).unwrap();

        let settled = feed(
            &mut engine,
            &proof_frame(&advertisement.hash, &proof),
            3_000,
        );
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
        assert!(
            engine.outgoing_resources.is_empty(),
            "a proven transfer retires its register row",
        );
    }

    #[test]
    fn a_wrong_or_misaddressed_proof_settles_nothing() {
        use crate::routing::links::resources::ResourceProof;

        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let (hash, names) = advertised_resource(&mut engine, &data);
        feed(&mut engine, &request_frame(&hash, None, &names), 2_000);

        let forged = feed(
            &mut engine,
            &proof_frame(&hash, &ResourceProof::new([0x5A; 32])),
            3_000,
        );
        assert!(forged.settlements.is_empty());

        let unknown = feed(
            &mut engine,
            &proof_frame(
                &ResourceHash::new([0x66; 32]),
                &ResourceProof::new([0x5A; 32]),
            ),
            3_100,
        );
        assert!(unknown.settlements.is_empty());

        let index = engine.outgoing_resources.lookup(&link_id(), &hash).unwrap();
        assert_eq!(
            engine.outgoing_resources.state(index).status,
            OutgoingResourceStatus::AwaitingProof,
            "the transfer keeps waiting for the genuine proof",
        );
    }

    fn advertised_hash(frame: &[u8]) -> ResourceHash {
        let (_, payload) = WirePacketHeader::parse(frame).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        ResourceAdvertisement::parse(opened).unwrap().hash
    }

    /// Segment 1 of 2 advertised live, segment 2 staged raw behind it; returns segment 1's hash.
    /// The payloads differ because the test entropy is a constant fill: identical data would seal to identical hashes here, where live entropy never repeats a nonce.
    pub(crate) fn staged_pair<S: StorageLayout>(engine: &mut EngineState<S>) -> ResourceHash {
        let first_data = four_part_payload();
        let second_data = b"the follower rides sealed and silent! ".repeat(40);
        let segment = |index| ResourceSegment {
            index,
            total_segments: 2,
            total_data_bytes: 3_000,
        };
        let first = send_segment(engine, 7, &first_data, segment(1));
        assert_eq!(first.frames.len(), 1);
        let live = advertised_hash(&first.frames[0].1);

        let second = send_segment(engine, 8, &second_data, segment(2));
        assert!(second.frames.is_empty(), "a staged segment sends nothing");
        assert!(
            second.settlements.is_empty(),
            "it settles at its own proof, like any segment"
        );
        assert!(
            engine.outgoing_resources.staged_index(&link_id()).is_some(),
            "the continuation is staged",
        );
        live
    }

    /// Request every live part, then drain the owed seal the way the manifold's yield arm would.
    fn serve_live_parts<S: StorageLayout>(
        engine: &mut EngineState<S>,
        live: &ResourceHash,
    ) -> InboundCapture {
        let index = engine.outgoing_resources.lookup(&link_id(), live).unwrap();
        let names = engine.outgoing_resources.names_flat(index).to_vec();
        let capture = feed(engine, &request_frame(live, None, &names), 2_000);
        while let Some(owed) = engine.owed_staged_seal_link() {
            engine.seal_staged_continuation(
                &owed,
                &mut |bytes: &mut [u8]| bytes.fill(0xB2),
                &mut |_| {},
            );
        }
        capture
    }

    #[test]
    fn a_continuation_stages_raw_and_silent_behind_the_live_segment() {
        let mut engine = heap_sender_with_active_link();
        staged_pair(&mut engine);

        assert_eq!(engine.outgoing_resources.len(), 2);
        let staged = engine.outgoing_resources.staged_index(&link_id()).unwrap();
        let state = engine.outgoing_resources.state(staged);
        assert_eq!(state.status, OutgoingResourceStatus::Staged);
        assert_eq!(
            state.staged_plaintext_bytes,
            RESOURCE_NONCE_LEN + 38 * 40,
            "the raw stream waits nonce-prefixed at its sealed offset",
        );
        assert_eq!(state.part_count, 0, "nothing is named before the seal");
        assert_eq!(
            engine.outgoing_resources.hash_at(staged),
            &ResourceHash::new([0; 32]),
            "the hash cannot exist before the seal",
        );
    }

    #[test]
    fn the_seal_is_owed_only_once_every_live_part_has_served() {
        let mut engine = heap_sender_with_active_link();
        let live = staged_pair(&mut engine);
        assert_eq!(
            engine.owed_staged_seal_link(),
            None,
            "no seal is owed while the live segment still serves — sealing early would sit between the request and the parts",
        );

        let index = engine.outgoing_resources.lookup(&link_id(), &live).unwrap();
        let names = engine.outgoing_resources.names_flat(index).to_vec();
        feed(&mut engine, &request_frame(&live, None, &names[..8]), 1_900);
        assert_eq!(
            engine.owed_staged_seal_link(),
            None,
            "a half-served window still owes parts first",
        );

        feed(&mut engine, &request_frame(&live, None, &names), 2_000);
        assert_eq!(engine.owed_staged_seal_link(), Some(link_id()));
        let staged = engine.outgoing_resources.staged_index(&link_id()).unwrap();
        assert_eq!(
            engine.outgoing_resources.state(staged).status,
            OutgoingResourceStatus::Staged,
            "the seal itself waits for the manifold's yielded turn",
        );
    }

    #[test]
    fn serving_the_last_live_part_seals_the_staged_continuation() {
        let mut engine = heap_sender_with_active_link();
        let live = staged_pair(&mut engine);

        let served = serve_live_parts(&mut engine, &live);
        assert_eq!(served.frames.len(), 4, "every live part rides out");
        assert_eq!(
            engine.owed_staged_seal_link(),
            None,
            "a sealed continuation owes nothing more",
        );

        let staged = engine.outgoing_resources.staged_index(&link_id()).unwrap();
        let state = *engine.outgoing_resources.state(staged);
        assert_eq!(state.status, OutgoingResourceStatus::StagedSealed);
        assert_eq!(state.staged_plaintext_bytes, 0);
        assert_eq!(
            state.sealed_transfer_bytes,
            crate::routing::links::resources::sealed_transfer_bytes(38 * 40),
        );
        assert_eq!(state.part_count, 4);
        assert_ne!(
            engine.outgoing_resources.hash_at(staged),
            &ResourceHash::new([0; 32]),
            "the seal names the transfer",
        );
        assert_eq!(
            engine.outgoing_resources.names_flat(staged).len(),
            4 * MAP_HASH_LEN,
        );
    }

    #[test]
    fn the_live_proof_promotes_the_sealed_continuation_in_the_same_pass() {
        let mut engine = heap_sender_with_active_link();
        let live = staged_pair(&mut engine);
        serve_live_parts(&mut engine, &live);
        let staged = engine.outgoing_resources.staged_index(&link_id()).unwrap();
        let staged_hash = *engine.outgoing_resources.hash_at(staged);
        let index = engine.outgoing_resources.lookup(&link_id(), &live).unwrap();
        let proof = engine.outgoing_resources.state(index).expected_proof;

        let capture = feed(&mut engine, &proof_frame(&live, &proof), 3_000);

        assert!(matches!(
            capture.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
        assert_eq!(
            capture.frames.len(),
            1,
            "the staged advertisement rides the proof's own pass",
        );
        let (_, frame) = &capture.frames[0];
        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.context, WireContext::ResourceAdvertisement);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();
        assert_eq!(advertisement.hash, staged_hash);
        assert_eq!(advertisement.segment_index, 2);
        assert_eq!(advertisement.original_hash, live, "the chain carries on");
        assert!(!advertisement.flags.compressed);

        let index = engine
            .outgoing_resources
            .lookup(&link_id(), &staged_hash)
            .unwrap();
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.status, OutgoingResourceStatus::Advertised);
        assert_eq!(state.retries_left, MAX_ADVERTISEMENT_RETRIES);
        assert!(
            engine.outgoing_resources.earliest_timeout_at().is_some(),
            "the promoted advertisement arms its own watchdog",
        );
    }

    #[test]
    fn a_request_or_proof_for_a_staged_hash_is_ignored() {
        let mut engine = heap_sender_with_active_link();
        let live = staged_pair(&mut engine);
        serve_live_parts(&mut engine, &live);
        let staged = engine.outgoing_resources.staged_index(&link_id()).unwrap();
        let staged_hash = *engine.outgoing_resources.hash_at(staged);
        let names = engine.outgoing_resources.names_flat(staged).to_vec();
        let proof = engine.outgoing_resources.state(staged).expected_proof;

        let requested = feed(
            &mut engine,
            &request_frame(&staged_hash, None, &names[..4]),
            2_500,
        );
        assert!(
            requested.frames.iter().all(|(_, frame)| {
                let (header, _) = WirePacketHeader::parse(frame).unwrap();
                header.context != WireContext::Resource
            }),
            "nothing staged is served",
        );

        let proven = feed(&mut engine, &proof_frame(&staged_hash, &proof), 2_600);
        assert!(proven.settlements.is_empty(), "nothing staged can prove");
        assert_eq!(engine.outgoing_resources.len(), 2);
    }

    #[test]
    fn a_pool_verdict_lands_and_a_follower_stages_behind_the_sealing_row() {
        let mut engine = heap_sender_with_active_link();
        let data = four_part_payload();
        let segment = |index| ResourceSegment {
            index,
            total_segments: 3,
            total_data_bytes: 5_000,
        };
        let first = send_segment(&mut engine, 7, &data, segment(1));
        let live = advertised_hash(&first.frames[0].1);
        let second_data = b"the follower rides sealed and silent! ".repeat(40);
        send_segment(&mut engine, 8, &second_data, segment(2));

        let index = engine.outgoing_resources.lookup(&link_id(), &live).unwrap();
        let names = engine.outgoing_resources.names_flat(index).to_vec();
        feed(&mut engine, &request_frame(&live, None, &names), 2_000);
        assert_eq!(engine.owed_staged_seal_link(), Some(link_id()));

        let (job_sdu, job_len, job_plaintext) = {
            let view = engine.staged_seal_job_view(&link_id()).unwrap();
            (view.sdu, view.nonce_prefixed_bytes, view.plaintext.to_vec())
        };
        engine.mark_staged_sealing(&link_id());
        assert_eq!(
            engine.owed_staged_seal_link(),
            None,
            "a row parked on the pool is not owed twice",
        );

        let proof = engine.outgoing_resources.state(index).expected_proof;
        let proven = feed(&mut engine, &proof_frame(&live, &proof), 2_500);
        assert!(matches!(
            proven.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
        assert!(
            proven.frames.is_empty(),
            "nothing can promote while the seal is still on the pool",
        );

        let third = send_segment(&mut engine, 9, &data, segment(3));
        assert!(
            third.settlements.is_empty(),
            "the follower stages behind the sealing row"
        );
        assert_eq!(engine.outgoing_resources.len(), 2);

        let stream_nonce: [u8; RESOURCE_NONCE_LEN] = job_plaintext[16..16 + RESOURCE_NONCE_LEN]
            .try_into()
            .unwrap();
        let stream_len = job_len - RESOURCE_NONCE_LEN;
        let mut transfer = job_plaintext;
        transfer.resize(
            crate::routing::links::resources::sealed_transfer_bytes(stream_len),
            0,
        );
        let mut worker_names = std::vec![0u8; transfer.len().div_ceil(job_sdu) * MAP_HASH_LEN];
        let outcome = seal_staged_resource(
            &link_key(),
            &[0xD1; 16],
            || [0xD2; RESOURCE_NONCE_LEN],
            job_sdu,
            job_len,
            crate::routing::links::resources::build_outgoing::BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut worker_names,
            },
        );
        let sealed_meta = outcome.unwrap();

        let stale_nonce = [0xEE; RESOURCE_NONCE_LEN];
        engine.apply_offloaded_staged_seal(
            OffloadedStagedSeal {
                link_id: link_id(),
                stream_nonce: stale_nonce,
                nonce_prefixed_bytes: job_len,
                sealed_bytes: &transfer[..sealed_meta.sealed_transfer_bytes],
                names: &worker_names[..sealed_meta.part_count * MAP_HASH_LEN],
                outcome: Ok(sealed_meta),
            },
            &mut |_| {},
        );
        assert!(
            engine
                .outgoing_resources
                .lookup(&link_id(), &sealed_meta.hash)
                .is_none(),
            "a verdict whose stream nonce matches no row lands nowhere",
        );

        engine.apply_offloaded_staged_seal(
            OffloadedStagedSeal {
                link_id: link_id(),
                stream_nonce,
                nonce_prefixed_bytes: job_len,
                sealed_bytes: &transfer[..sealed_meta.sealed_transfer_bytes],
                names: &worker_names[..sealed_meta.part_count * MAP_HASH_LEN],
                outcome: Ok(sealed_meta),
            },
            &mut |_| {},
        );
        let sealed_index = engine
            .outgoing_resources
            .lookup(&link_id(), &sealed_meta.hash)
            .expect("the verdict lands on the sealing row");
        assert_eq!(
            engine.outgoing_resources.state(sealed_index).status,
            OutgoingResourceStatus::StagedSealed,
        );

        let mut promoted_frames = std::vec::Vec::new();
        engine.promote_staged_resource(
            &link_id(),
            InstantMillis(3_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    if let Some(frame) = filled_frame(fill) {
                        promoted_frames.push(frame);
                    }
                }
            },
        );
        assert_eq!(
            promoted_frames.len(),
            1,
            "the applied seal promotes now that the proof already came"
        );
        let (header, payload) = WirePacketHeader::parse(&promoted_frames[0]).unwrap();
        assert_eq!(header.context, WireContext::ResourceAdvertisement);
        let mut sealed_adv = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed_adv).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();
        assert_eq!(advertisement.hash, sealed_meta.hash);
        assert_eq!(advertisement.segment_index, 2);
        assert_eq!(advertisement.original_hash, live);

        let follower = engine.outgoing_resources.staged_index(&link_id()).unwrap();
        assert_eq!(
            engine.outgoing_resources.state(follower).status,
            OutgoingResourceStatus::Staged,
            "the follower waits its own turn",
        );
        assert_eq!(engine.outgoing_resources.state(follower).segment_index, 3);
        assert_eq!(
            engine.outgoing_resources.state(follower).original_hash,
            live,
            "the landing patch must address the follower itself — the lowest staged row is the sealing one",
        );
    }

    fn receiver_cancel_frame(hash: &ResourceHash) -> std::vec::Vec<u8> {
        let mut plaintext = [0u8; RESOURCE_HASH_LEN];
        write_cancel_plaintext(hash, &mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceReceiverCancel,
            &plaintext,
            &[0xC4; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_bytes].to_vec()
    }

    #[test]
    fn a_continuation_after_its_chain_died_settles_predecessor_failed() {
        let mut engine = heap_sender_with_active_link();
        let data = four_part_payload();
        let first = send_segment(
            &mut engine,
            7,
            &data,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: 3_000,
            },
        );
        let live = advertised_hash(&first.frames[0].1);
        feed(&mut engine, &receiver_cancel_frame(&live), 2_000);
        assert!(engine.outgoing_resources.is_empty());

        let late = send_segment(
            &mut engine,
            8,
            &data,
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: 3_000,
            },
        );
        assert!(
            late.frames.is_empty(),
            "a dead chain's tail never advertises"
        );
        assert!(matches!(
            late.settlements[0],
            (
                CommandId(8),
                Settlement::SendResource(Err(SendResourceFailure::PredecessorFailed)),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }

    #[test]
    fn a_peer_rejection_fails_the_staged_continuation_by_name() {
        let mut engine = heap_sender_with_active_link();
        let live = staged_pair(&mut engine);

        let capture = feed(&mut engine, &receiver_cancel_frame(&live), 2_500);

        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::RejectedByPeer)),
            ),
        ));
        assert!(matches!(
            capture.settlements[1],
            (
                CommandId(8),
                Settlement::SendResource(Err(SendResourceFailure::PredecessorFailed)),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }

    #[test]
    fn a_transfer_the_store_cannot_hold_rejects_and_releases_the_slot() {
        let mut engine = sender_with_active_link();
        let oversized = std::vec![0x42u8; 5_000];
        let capture = send(&mut engine, 7, &oversized, None);

        assert!(capture.frames.is_empty());
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceRejection::Build(BuildOutgoingResourceError::Seal(BufferTooShort,)),
                ))),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::tests::{link_id, sender_with_active_link, watch_capture, SendCapture};
    use super::*;
    use crate::engine::{CommandId, Settlement};
    use crate::engine::{InstantMillis, WakeSchedule};
    use crate::wire::WirePacketHeader;

    fn advertised_sender() -> (
        crate::engine::EngineState<crate::engine::test_support::TestStorageLayout>,
        SendCapture,
    ) {
        let mut engine = sender_with_active_link();
        let data = b"watchdogs keep the resource honest! ".repeat(40);
        let capture = super::tests::send(&mut engine, 7, &data, None);
        (engine, capture)
    }

    #[test]
    fn an_unanswered_advertisement_retries_then_cancels_with_its_name() {
        let (mut engine, _) = advertised_sender();
        assert_eq!(
            engine.resource_deadlines_wake(),
            WakeSchedule::At(InstantMillis(1_500 + 250 * 6 + 1_000)),
            "the advertisement arms rtt x traffic factor plus the processing grace",
        );

        let mut now = 4_000u64;
        for retry in 0..4u64 {
            let capture = watch_capture(&mut engine, now);
            assert_eq!(capture.frames.len(), 1, "retry {retry} re-advertises");
            let (header, _) = WirePacketHeader::parse(&capture.frames[0].1).unwrap();
            assert_eq!(header.context, WireContext::ResourceAdvertisement);
            assert!(capture.settlements.is_empty());
            now += 3_000;
        }

        let capture = watch_capture(&mut engine, now);
        assert_eq!(capture.frames.len(), 1, "the cancel rides out");
        let (header, _) = WirePacketHeader::parse(&capture.frames[0].1).unwrap();
        assert_eq!(header.context, WireContext::ResourceInitiatorCancel);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Timeout)),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
        assert_eq!(engine.resource_deadlines_wake(), WakeSchedule::Idle);
    }

    #[test]
    fn a_dead_live_segment_takes_its_staged_continuation_with_it() {
        let mut engine = super::tests::heap_sender_with_active_link();
        super::tests::staged_pair(&mut engine);

        let mut now = 4_000u64;
        for _ in 0..4 {
            watch_capture(&mut engine, now);
            now += 3_000;
        }
        let capture = watch_capture(&mut engine, now);

        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Timeout)),
            ),
        ));
        assert!(matches!(
            capture.settlements[1],
            (
                CommandId(8),
                Settlement::SendResource(Err(SendResourceFailure::PredecessorFailed)),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
        assert_eq!(engine.resource_deadlines_wake(), WakeSchedule::Idle);
    }

    #[test]
    fn a_missing_proof_rearms_its_retries_then_cancels() {
        let (mut engine, capture) = advertised_sender();
        let (_, adv_frame) = &capture.frames[0];
        let (_, payload) = WirePacketHeader::parse(adv_frame).unwrap();
        let mut sealed = payload.to_vec();
        let opened = super::tests::link_key().open_in_place(&mut sealed).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();
        super::tests::feed(
            &mut engine,
            &super::tests::request_frame(&advertisement.hash, None, advertisement.hashmap),
            2_000,
        );
        let index = engine
            .outgoing_resources
            .lookup(&link_id(), &advertisement.hash)
            .unwrap();
        assert_eq!(
            engine.outgoing_resources.state(index).status,
            OutgoingResourceStatus::AwaitingProof,
        );
        assert_eq!(
            engine.resource_deadlines_wake(),
            WakeSchedule::At(InstantMillis(2_000 + 250 * 3 + 10_000)),
        );

        let mut now = 13_000u64;
        for _ in 0..3 {
            let capture = watch_capture(&mut engine, now);
            assert!(capture.frames.is_empty(), "a proof retry sends nothing yet");
            assert!(capture.settlements.is_empty());
            now += 11_000;
        }
        let capture = watch_capture(&mut engine, now);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Timeout)),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }
}
