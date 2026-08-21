use crate::crypto::BufferTooShort;
use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::routing::links::resources::build_outgoing::{
    BuildOutgoingResourceError, BuildRegions, BuiltResource,
};
use crate::routing::links::resources::streamed_open::OpenProgress;
use crate::routing::links::resources::{
    checked_resource_buffer_bytes, sealed_transfer_bytes, ResourceBufferShape,
    ResourceBufferShapeError, ResourceCompression, ResourceCorrelation, ResourceHash,
    ResourceProof, ResourceSegment, SaltNonce, HASHMAP_MAX_LEN, MAP_HASH_LEN, MAX_EFFICIENT_SIZE,
    PART_TIMEOUT_FACTOR, RESOURCE_NONCE_LEN, WINDOW_MAX_SLOW, WINDOW_MIN, WINDOW_START,
};
use crate::routing::links::LinkId;

/// Splits a row's state by storage class: the `Copy` bookkeeping struct implements this, naming the non-`Copy` working state its table stores in a parallel column beside it. The incoming side tracks its streamed open's [`OpenProgress`] there; the outgoing side parks nothing.
pub trait ResourceRowState {
    type StreamedOpenSlot: Default + core::fmt::Debug;
}

impl ResourceRowState for OutgoingResourceState {
    type StreamedOpenSlot = ();
}

impl ResourceRowState for IncomingResourceState {
    type StreamedOpenSlot = OpenProgress;
}

#[cfg(test)]
impl ResourceRowState for u8 {
    type StreamedOpenSlot = ();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingResourceStatus {
    /// A raw continuation stream parked at its sealed offset, deferring the seal until the segment ahead finishes serving during the receiver-busy window.
    Staged,
    /// The deferred seal is running on a crypto-pool worker; the verdict lands as [`StagedSealed`](Self::StagedSealed).
    StagedSealing,
    /// Sealed and named, waiting only for the proof ahead of it to release its advertisement.
    StagedSealed,
    Advertised,
    Transferring,
    AwaitingProof,
}

impl OutgoingResourceStatus {
    /// Off the wire in every staged form: nothing staged serves parts, accepts proofs, or hears cancels.
    pub fn is_staged(self) -> bool {
        matches!(
            self,
            Self::Staged | Self::StagedSealing | Self::StagedSealed
        )
    }
}

/// Which lane [`lane_for`](OutgoingResources::lane_for) assigns an arriving segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackLane {
    Live,
    Staged,
}

/// The command-side envelope every landed row records, whichever track form lands it.
#[derive(Debug, Clone, Copy)]
pub struct TrackedCommand {
    pub link_id: LinkId,
    pub sdu: usize,
    pub command_id: CommandId,
    pub correlation: ResourceCorrelation,
    pub segment: ResourceSegment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncomingResourceStatus {
    Transferring,
    /// Every part is placed but a pool worker still holds the streamed open; the span verdict concludes the transfer.
    AwaitingOpen,
    AwaitingDecompression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutgoingResourceState {
    pub salt_nonce: SaltNonce,
    pub expected_proof: ResourceProof,
    pub sealed_transfer_bytes: usize,
    /// The nonce-prefixed plaintext length a raw [`Staged`](OutgoingResourceStatus::Staged) row holds until its deferred seal; zero once sealed.
    pub staged_plaintext_bytes: usize,
    pub uncompressed_data_bytes: u64,
    pub segment_index: u64,
    pub total_segments: u64,
    pub original_hash: ResourceHash,
    pub compression: ResourceCompression,
    pub has_metadata: bool,
    pub part_count: usize,
    pub sdu: usize,
    pub scope_start: usize,
    pub sent_part_count: usize,
    pub status: OutgoingResourceStatus,
    pub retries_left: u8,
    pub command_id: CommandId,
    pub correlation: ResourceCorrelation,
}

// The vacant-slot value for fixed-capacity tables to initialize with, never a live resource's state; a successful [track](OutgoingResources::track) writes every field.
impl Default for OutgoingResourceState {
    fn default() -> Self {
        Self {
            salt_nonce: SaltNonce::new([0; 4]),
            expected_proof: ResourceProof::new([0; 32]),
            sealed_transfer_bytes: 0,
            staged_plaintext_bytes: 0,
            uncompressed_data_bytes: 0,
            segment_index: 1,
            total_segments: 1,
            original_hash: ResourceHash::new([0; 32]),
            compression: ResourceCompression::Uncompressed,
            has_metadata: false,
            part_count: 0,
            sdu: 0,
            scope_start: 0,
            sent_part_count: 0,
            status: OutgoingResourceStatus::Advertised,
            retries_left: 0,
            command_id: CommandId(0),
            correlation: ResourceCorrelation::Unsolicited,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncomingResourceState {
    pub salt_nonce: SaltNonce,
    pub compression: ResourceCompression,
    pub has_metadata: bool,
    pub uncompressed_data_bytes: u64,
    pub segment_index: u64,
    pub total_segments: u64,
    pub sealed_transfer_bytes: usize,
    pub part_count: usize,
    pub sdu: usize,
    pub received_part_count: usize,
    pub outstanding_part_count: usize,
    pub consecutive_completed: Option<usize>,
    pub hashmap_height: usize,
    pub waiting_for_hmu: bool,
    pub window: usize,
    pub window_min: usize,
    pub window_max: usize,
    pub status: IncomingResourceStatus,
    pub retries_left: u8,
    pub correlation: ResourceCorrelation,
    pub measured_rtt_ms: Option<u64>,
    pub part_timeout_factor: u64,
    pub request_sent_at: Option<InstantMillis>,
    pub request_sent_bytes: u64,
    pub awaiting_round_first_response: bool,
    pub received_byte_count: u64,
    pub received_byte_count_at_request: u64,
    pub request_response_bytes_per_second: u64,
    pub data_bytes_per_second: u64,
    pub inherited_eifr: Option<u64>,
    pub fast_rate_rounds: u8,
    pub very_slow_rate_rounds: u8,
}

impl IncomingResourceState {
    /// The consecutive frontier in bytes: everything placed before the first gap.
    pub fn contiguous_byte_len(&self) -> usize {
        match self.consecutive_completed {
            None => 0,
            Some(height) => ((height + 1) * self.sdu).min(self.sealed_transfer_bytes),
        }
    }
}

/// The vacant-slot value for fixed-capacity tables to initialize with, never a live transfer's state; [accept](IncomingResources::accept) writes every field.
impl Default for IncomingResourceState {
    fn default() -> Self {
        Self {
            salt_nonce: SaltNonce::new([0; 4]),
            compression: ResourceCompression::Uncompressed,
            has_metadata: false,
            uncompressed_data_bytes: 0,
            segment_index: 1,
            total_segments: 1,
            sealed_transfer_bytes: 0,
            part_count: 0,
            sdu: 0,
            received_part_count: 0,
            outstanding_part_count: 0,
            consecutive_completed: None,
            hashmap_height: 0,
            waiting_for_hmu: false,
            window: WINDOW_START,
            window_min: WINDOW_MIN,
            window_max: WINDOW_MAX_SLOW,
            status: IncomingResourceStatus::Transferring,
            retries_left: 0,
            correlation: ResourceCorrelation::Unsolicited,
            measured_rtt_ms: None,
            part_timeout_factor: PART_TIMEOUT_FACTOR,
            request_sent_at: None,
            request_sent_bytes: 0,
            awaiting_round_first_response: false,
            received_byte_count: 0,
            received_byte_count_at_request: 0,
            request_response_bytes_per_second: 0,
            data_bytes_per_second: 0,
            inherited_eifr: None,
            fast_rate_rounds: 0,
            very_slow_rate_rounds: 0,
        }
    }
}

/// One slot's mutable regions, borrowed together so a build can seal into the transfer while naming parts into the same slot.
pub struct ResourceBuffers<'a> {
    pub transfer: &'a mut [u8],
    pub part_names: &'a mut [[u8; MAP_HASH_LEN]],
    pub part_flags: &'a mut [bool],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceTablePushError {
    TableFull,
    MemoryLimit,
    TransferTooLarge,
    TooManyParts,
}

pub trait ResourceTable<State: ResourceRowState> {
    fn capacity(&self) -> usize;
    fn transfer_capacity(&self) -> usize;
    fn part_capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn active_buffer_bytes(&self) -> usize {
        (0..self.len()).fold(0usize, |total, index| {
            let row = self
                .transfer(index)
                .len()
                .saturating_add(self.part_names(index).len().saturating_mul(MAP_HASH_LEN))
                .saturating_add(
                    self.part_flags(index)
                        .len()
                        .saturating_mul(core::mem::size_of::<bool>()),
                );
            total.saturating_add(row)
        })
    }

    fn buffer_memory_limit(&self) -> usize {
        checked_resource_buffer_bytes(self.transfer_capacity(), self.part_capacity())
            .and_then(|row| row.checked_mul(self.capacity()))
            .unwrap_or(usize::MAX)
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn link_ids(&self) -> &[LinkId];
    fn hashes(&self) -> &[ResourceHash];
    fn timeout_ats(&self) -> &[Option<InstantMillis>];
    fn states(&self) -> &[State];

    fn set_hash(&mut self, index: usize, hash: ResourceHash);
    fn set_timeout_at(&mut self, index: usize, timeout_at: Option<InstantMillis>);
    fn state_mut(&mut self, index: usize) -> &mut State;

    fn transfer(&self, index: usize) -> &[u8];
    fn part_names(&self, index: usize) -> &[[u8; MAP_HASH_LEN]];
    fn part_flags(&self, index: usize) -> &[bool];
    fn streamed_open(&self, index: usize) -> &State::StreamedOpenSlot;
    fn buffers_mut(&mut self, index: usize) -> ResourceBuffers<'_>;
    fn transfer_and_streamed_open_mut(
        &mut self,
        index: usize,
    ) -> (&mut [u8], &mut State::StreamedOpenSlot);

    /// Classify a validated row shape without reserving it. The pending-offer
    /// scheduler uses this to distinguish burst pressure from an offer that
    /// can never fit this storage recipe.
    fn admission_for_shape(&self, shape: ResourceBufferShape) -> ResourceTableAdmission;

    fn push(
        &mut self,
        link_id: LinkId,
        hash: ResourceHash,
        state: State,
        shape: ResourceBufferShape,
    ) -> Result<usize, ResourceTablePushError>;
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceTableAdmission {
    Available,
    TemporarilyFull,
    Impossible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackOutgoingResourceError {
    TableFull,
    LinkBusy,
    Build(BuildOutgoingResourceError),
}

fn track_push_error(error: ResourceTablePushError) -> TrackOutgoingResourceError {
    match error {
        ResourceTablePushError::TableFull | ResourceTablePushError::MemoryLimit => {
            TrackOutgoingResourceError::TableFull
        }
        ResourceTablePushError::TransferTooLarge => {
            TrackOutgoingResourceError::Build(BuildOutgoingResourceError::Seal(BufferTooShort))
        }
        ResourceTablePushError::TooManyParts => {
            TrackOutgoingResourceError::Build(BuildOutgoingResourceError::HashmapBufferTooShort)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartSendOutcome {
    FirstSend,
    Resend,
    NoSuchPart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    NotTracked,
}

#[derive(Debug, Default)]
pub struct OutgoingResources<C: ResourceTable<OutgoingResourceState>> {
    table: C,
    earliest_timeout: Option<InstantMillis>,
}

impl<C: ResourceTable<OutgoingResourceState>> OutgoingResources<C> {
    /// One resource per link at a time, matching RNS 1.4.2 `Link.ready_for_new_resource`.
    ///
    /// Intentional deviation from reference: the next continuation segment may land beside the link's one occupied row and remain off the wire until the segment ahead proves. Its preparation therefore overlaps the wire instead of following it.
    /// The occupied row may itself still be staged: a proof settles its segment before the follower finishes sealing, and the host's next dispatch lands in that window.
    /// The wire still carries one resource at a time; a second row already waiting, or anything that is not the exact continuation, stays `LinkBusy`.
    pub fn lane_for(
        &self,
        link_id: &LinkId,
        segment: &ResourceSegment,
    ) -> Result<TrackLane, TrackOutgoingResourceError> {
        let mut newest = None;
        for (candidate, state) in self.table.link_ids().iter().zip(self.table.states()) {
            if candidate != link_id {
                continue;
            }
            if newest.is_some() {
                return Err(TrackOutgoingResourceError::LinkBusy);
            }
            newest = Some((state.segment_index, state.total_segments));
        }
        match newest {
            None => Ok(TrackLane::Live),
            Some((newest_index, newest_total)) => {
                let continues = newest_total > 1
                    && newest_index.checked_add(1) == Some(segment.index)
                    && segment.total_segments == newest_total;
                if continues {
                    Ok(TrackLane::Staged)
                } else {
                    Err(TrackOutgoingResourceError::LinkBusy)
                }
            }
        }
    }

    /// A failed build releases the slot untouched.
    /// A `Staged` lane lands the finished build as [`StagedSealed`](OutgoingResourceStatus::StagedSealed): the compressed-continuation path, whose stream cannot re-derive its digests later and so seals here.
    pub fn track_built(
        &mut self,
        command: TrackedCommand,
        lane: TrackLane,
        shape: ResourceBufferShape,
        build: impl FnOnce(BuildRegions<'_>) -> Result<BuiltResource, BuildOutgoingResourceError>,
    ) -> Result<ResourceHash, TrackOutgoingResourceError> {
        let TrackedCommand {
            link_id,
            sdu,
            command_id,
            correlation,
            segment,
        } = command;
        let expected_transfer_bytes = shape.transfer_bytes();
        let expected_part_count = shape.part_count();
        let index = self
            .table
            .push(
                link_id,
                ResourceHash::new([0; 32]),
                OutgoingResourceState::default(),
                shape,
            )
            .map_err(track_push_error)?;

        let buffers = self.table.buffers_mut(index);

        match build(BuildRegions {
            transfer: buffers.transfer,
            hashmap: buffers.part_names.as_flattened_mut(),
        }) {
            Ok(built)
                if built.sealed_transfer_bytes == expected_transfer_bytes
                    && built.part_count == expected_part_count =>
            {
                self.table.set_hash(index, built.hash);
                *self.table.state_mut(index) = OutgoingResourceState {
                    salt_nonce: built.salt_nonce,
                    expected_proof: built.expected_proof,
                    sealed_transfer_bytes: built.sealed_transfer_bytes,
                    staged_plaintext_bytes: 0,
                    uncompressed_data_bytes: built.uncompressed_data_bytes,
                    segment_index: segment.index,
                    total_segments: segment.total_segments,
                    original_hash: built.hash,
                    compression: built.compression,
                    has_metadata: built.has_metadata,
                    part_count: built.part_count,
                    sdu,
                    scope_start: 0,
                    sent_part_count: 0,
                    status: match lane {
                        TrackLane::Live => OutgoingResourceStatus::Advertised,
                        TrackLane::Staged => OutgoingResourceStatus::StagedSealed,
                    },
                    retries_left: 0,
                    command_id,
                    correlation,
                };
                self.refresh_earliest_timeout();
                Ok(built.hash)
            }
            Ok(_) => {
                self.table.swap_remove(index);
                self.refresh_earliest_timeout();
                Err(TrackOutgoingResourceError::Build(
                    BuildOutgoingResourceError::BufferShapeMismatch,
                ))
            }
            Err(error) => {
                self.table.swap_remove(index);
                self.refresh_earliest_timeout();
                Err(TrackOutgoingResourceError::Build(error))
            }
        }
    }

    /// The uncompressed-continuation path parks the raw stream at its sealed offset and defers the whole seal to [`seal_regions_mut`](Self::seal_regions_mut) time. It returns the landed row's index because the placeholder hash can never name it.
    pub fn stage_raw(
        &mut self,
        command: TrackedCommand,
        has_metadata: bool,
        stream_len: usize,
        prefill: impl FnOnce(&mut [u8]),
    ) -> Result<usize, TrackOutgoingResourceError> {
        let TrackedCommand {
            link_id,
            sdu,
            command_id,
            correlation,
            segment,
        } = command;
        if stream_len > MAX_EFFICIENT_SIZE {
            return Err(TrackOutgoingResourceError::Build(
                BuildOutgoingResourceError::DataTooLarge,
            ));
        }
        if sealed_transfer_bytes(stream_len) > self.table.transfer_capacity() {
            return Err(TrackOutgoingResourceError::Build(
                BuildOutgoingResourceError::Seal(BufferTooShort),
            ));
        }
        let transfer_bytes = sealed_transfer_bytes(stream_len);
        let shape =
            ResourceBufferShape::try_for_transfer(transfer_bytes, sdu).map_err(|error| {
                TrackOutgoingResourceError::Build(match error {
                    ResourceBufferShapeError::EmptyTransfer => {
                        BuildOutgoingResourceError::Seal(BufferTooShort)
                    }
                    ResourceBufferShapeError::SduTooSmall => {
                        BuildOutgoingResourceError::SduTooSmall
                    }
                    ResourceBufferShapeError::SizeOverflow => {
                        BuildOutgoingResourceError::DataTooLarge
                    }
                })
            })?;
        let index = self
            .table
            .push(
                link_id,
                ResourceHash::new([0; 32]),
                OutgoingResourceState::default(),
                shape,
            )
            .map_err(track_push_error)?;

        prefill(self.table.buffers_mut(index).transfer);
        *self.table.state_mut(index) = OutgoingResourceState {
            staged_plaintext_bytes: RESOURCE_NONCE_LEN + stream_len,
            segment_index: segment.index,
            total_segments: segment.total_segments,
            compression: ResourceCompression::Uncompressed,
            has_metadata,
            sdu,
            status: OutgoingResourceStatus::Staged,
            command_id,
            correlation,
            ..OutgoingResourceState::default()
        };
        self.refresh_earliest_timeout();
        Ok(index)
    }

    /// The link's next staged row in segment order. With a follower parked behind a still-sealing row, the lower segment index promotes first.
    pub fn staged_index(&self, link_id: &LinkId) -> Option<usize> {
        let mut lowest: Option<(usize, u64)> = None;
        for (index, (candidate, state)) in self
            .table
            .link_ids()
            .iter()
            .zip(self.table.states())
            .enumerate()
        {
            if candidate != link_id || !state.status.is_staged() {
                continue;
            }
            if lowest.is_none_or(|(_, segment)| state.segment_index < segment) {
                lowest = Some((index, state.segment_index));
            }
        }
        lowest.map(|(index, _)| index)
    }

    /// The mutable transfer + name regions a deferred seal writes, borrowed together like a build's.
    pub fn seal_regions_mut(&mut self, index: usize) -> BuildRegions<'_> {
        let buffers = self.table.buffers_mut(index);
        BuildRegions {
            transfer: buffers.transfer,
            hashmap: buffers.part_names.as_flattened_mut(),
        }
    }

    pub fn lookup(&self, link_id: &LinkId, hash: &ResourceHash) -> Option<usize> {
        self.table
            .link_ids()
            .iter()
            .zip(self.table.hashes())
            .position(|(candidate_link, candidate_hash)| {
                candidate_link == link_id && candidate_hash == hash
            })
    }

    pub fn state(&self, index: usize) -> &OutgoingResourceState {
        &self.table.states()[index]
    }

    pub fn state_mut(&mut self, index: usize) -> &mut OutgoingResourceState {
        self.table.state_mut(index)
    }

    pub fn set_hash(&mut self, index: usize, hash: ResourceHash) {
        self.table.set_hash(index, hash);
    }

    /// A raw staged row's whole worker input: the reserved IV span, the stream nonce, and the parked stream.
    pub fn staged_plaintext(&self, index: usize) -> &[u8] {
        let len = 16 + self.table.states()[index].staged_plaintext_bytes;
        &self.table.transfer(index)[..len]
    }

    pub fn sealed_transfer(&self, index: usize) -> &[u8] {
        let len = self.table.states()[index].sealed_transfer_bytes;
        &self.table.transfer(index)[..len]
    }

    pub fn names_flat(&self, index: usize) -> &[u8] {
        let count = self.table.states()[index].part_count;
        self.table.part_names(index)[..count].as_flattened()
    }

    pub fn link_at(&self, index: usize) -> &LinkId {
        &self.table.link_ids()[index]
    }

    pub fn hash_at(&self, index: usize) -> &ResourceHash {
        &self.table.hashes()[index]
    }

    /// The distinction RNS 1.4.2 draws between `part.send()` (counted toward `sent_parts`) and `part.resend()` (not counted).
    pub fn mark_sent(&mut self, index: usize, part_index: usize) -> PartSendOutcome {
        if part_index >= self.table.states()[index].part_count {
            return PartSendOutcome::NoSuchPart;
        }
        let buffers = self.table.buffers_mut(index);
        if buffers.part_flags[part_index] {
            return PartSendOutcome::Resend;
        }
        buffers.part_flags[part_index] = true;
        self.table.state_mut(index).sent_part_count += 1;
        PartSendOutcome::FirstSend
    }

    pub fn remove(&mut self, link_id: &LinkId, hash: &ResourceHash) -> RemoveOutcome {
        match self.lookup(link_id, hash) {
            Some(index) => {
                self.table.swap_remove(index);
                self.refresh_earliest_timeout();
                RemoveOutcome::Removed
            }
            None => RemoveOutcome::NotTracked,
        }
    }

    pub fn set_timeout_at(&mut self, index: usize, timeout_at: Option<InstantMillis>) {
        self.table.set_timeout_at(index, timeout_at);
        self.refresh_earliest_timeout();
    }

    fn refresh_earliest_timeout(&mut self) {
        self.earliest_timeout = self.table.timeout_ats().iter().flatten().min().copied();
    }

    pub fn earliest_timeout_at(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_timeout,
            self.table.timeout_ats().iter().flatten().min().copied(),
            "earliest_timeout cache desynced from the timeout_ats column"
        );
        self.earliest_timeout
    }

    pub fn due_index(&self, now: InstantMillis) -> Option<usize> {
        self.table
            .timeout_ats()
            .iter()
            .position(|deadline| deadline.is_some_and(|at| at <= now))
    }

    pub fn transfer_capacity(&self) -> usize {
        self.table.transfer_capacity()
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn active_buffer_bytes(&self) -> usize {
        self.table.active_buffer_bytes()
    }

    pub fn buffer_memory_limit(&self) -> usize {
        self.table.buffer_memory_limit()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedResource<'a> {
    pub hash: ResourceHash,
    pub salt_nonce: SaltNonce,
    pub compression: ResourceCompression,
    /// The advertisement's metadata flag: the verified stream opens with a length-prefixed packed block (in this segment if it is the first).
    pub has_metadata: bool,
    pub uncompressed_data_bytes: u64,
    pub segment_index: u64,
    pub sealed_transfer_bytes: usize,
    pub sdu: usize,
    pub correlation: ResourceCorrelation,

    /// How many wire packets this one segment's sealed stream splits into. Each is `sdu` bytes long, except for the last one which is typically shorter.
    pub part_count: usize,
    /// How many sibling resources the whole transfer was split into; this offer carries one of them.
    pub total_segment_count: u64,

    /// The advertisement's embedded first hashmap page: the flat salted 4-byte names of the leading parts.
    pub initial_names: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptIncomingResourceError {
    TableFull,
    AlreadyReceiving,
    EmptyTransfer,
    SduTooSmall,
    PartCountMismatch,
    TransferTooLarge,
    TooManyParts,
    HashmapTooLong,
    /// The name bytes are not a whole number of 4-byte map hashes: a torn name at the tail.
    HashmapRagged,
    HashmapBeyondPartCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncomingResourceAdmission {
    Available,
    AlreadyReceiving,
    TemporarilyFull,
    Impossible,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncomingResourceStorageAdmission {
    Available,
    TemporarilyFull,
    Impossible,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyHashmapUpdateError {
    BeyondPartCount,
    SkipsAhead,
    HashmapTooLong,
    /// The name bytes are not a whole number of 4-byte map hashes: a torn name at the tail.
    HashmapRagged,
}

/// Every non-placed outcome matches the reference's, we just name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacePartOutcome {
    Placed,
    NoSuchPart,
    WrongLength,
    BeyondTransferEnd,
    Duplicate,
}

#[derive(Debug, Default)]
pub struct IncomingResources<C: ResourceTable<IncomingResourceState>> {
    table: C,
    earliest_timeout: Option<InstantMillis>,
}

fn accepted_resource_shape(
    offer: AcceptedResource<'_>,
) -> Result<ResourceBufferShape, AcceptIncomingResourceError> {
    let shape = ResourceBufferShape::try_for_transfer(offer.sealed_transfer_bytes, offer.sdu)
        .map_err(|error| match error {
            ResourceBufferShapeError::EmptyTransfer => AcceptIncomingResourceError::EmptyTransfer,
            ResourceBufferShapeError::SduTooSmall => AcceptIncomingResourceError::SduTooSmall,
            ResourceBufferShapeError::SizeOverflow => AcceptIncomingResourceError::TransferTooLarge,
        })?;
    if offer.part_count != shape.part_count() {
        return Err(AcceptIncomingResourceError::PartCountMismatch);
    }
    if offer.initial_names.len() > HASHMAP_MAX_LEN * MAP_HASH_LEN {
        return Err(AcceptIncomingResourceError::HashmapTooLong);
    }
    if !offer.initial_names.len().is_multiple_of(MAP_HASH_LEN) {
        return Err(AcceptIncomingResourceError::HashmapRagged);
    }
    if offer.initial_names.len() / MAP_HASH_LEN > offer.part_count {
        return Err(AcceptIncomingResourceError::HashmapBeyondPartCount);
    }
    Ok(shape)
}

impl<C: ResourceTable<IncomingResourceState>> IncomingResources<C> {
    pub(crate) fn storage_admission_for(
        &self,
        offer: AcceptedResource<'_>,
    ) -> IncomingResourceStorageAdmission {
        let shape = match accepted_resource_shape(offer) {
            Ok(shape) => shape,
            Err(AcceptIncomingResourceError::TransferTooLarge) => {
                return IncomingResourceStorageAdmission::Impossible
            }
            Err(
                AcceptIncomingResourceError::TableFull
                | AcceptIncomingResourceError::AlreadyReceiving
                | AcceptIncomingResourceError::EmptyTransfer
                | AcceptIncomingResourceError::SduTooSmall
                | AcceptIncomingResourceError::PartCountMismatch
                | AcceptIncomingResourceError::TooManyParts
                | AcceptIncomingResourceError::HashmapTooLong
                | AcceptIncomingResourceError::HashmapRagged
                | AcceptIncomingResourceError::HashmapBeyondPartCount,
            ) => return IncomingResourceStorageAdmission::Malformed,
        };
        match self.table.admission_for_shape(shape) {
            ResourceTableAdmission::Available => IncomingResourceStorageAdmission::Available,
            ResourceTableAdmission::TemporarilyFull => {
                IncomingResourceStorageAdmission::TemporarilyFull
            }
            ResourceTableAdmission::Impossible => IncomingResourceStorageAdmission::Impossible,
        }
    }

    pub(crate) fn admission_for(
        &self,
        link_id: &LinkId,
        offer: AcceptedResource<'_>,
    ) -> IncomingResourceAdmission {
        let storage = self.storage_admission_for(offer);
        let storage = match storage {
            IncomingResourceStorageAdmission::Available => IncomingResourceAdmission::Available,
            IncomingResourceStorageAdmission::TemporarilyFull => {
                IncomingResourceAdmission::TemporarilyFull
            }
            IncomingResourceStorageAdmission::Impossible => {
                return IncomingResourceAdmission::Impossible
            }
            IncomingResourceStorageAdmission::Malformed => {
                return IncomingResourceAdmission::Malformed
            }
        };
        if self.lookup(link_id, &offer.hash).is_some() {
            IncomingResourceAdmission::AlreadyReceiving
        } else {
            storage
        }
    }

    /// The capacity and shape gate the engine asks at accept; policy gating happens before the offer ever reaches the table.
    /// The duplicate refusal is RNS 1.4.2 `Resource.accept`'s `has_incoming_resource` registration gate.
    pub fn accept(
        &mut self,
        link_id: LinkId,
        offer: AcceptedResource<'_>,
    ) -> Result<usize, AcceptIncomingResourceError> {
        let shape = accepted_resource_shape(offer)?;
        if shape.transfer_bytes() > self.table.transfer_capacity() {
            return Err(AcceptIncomingResourceError::TransferTooLarge);
        }
        if shape.part_count() > self.table.part_capacity() {
            return Err(AcceptIncomingResourceError::TooManyParts);
        }

        if self.lookup(&link_id, &offer.hash).is_some() {
            return Err(AcceptIncomingResourceError::AlreadyReceiving);
        }

        let index = self
            .table
            .push(
                link_id,
                offer.hash,
                IncomingResourceState {
                    salt_nonce: offer.salt_nonce,
                    compression: offer.compression,
                    has_metadata: offer.has_metadata,
                    uncompressed_data_bytes: offer.uncompressed_data_bytes,
                    segment_index: offer.segment_index,
                    total_segments: offer.total_segment_count,
                    sealed_transfer_bytes: offer.sealed_transfer_bytes,
                    part_count: shape.part_count(),
                    sdu: offer.sdu,
                    received_part_count: 0,
                    outstanding_part_count: 0,
                    consecutive_completed: None,
                    hashmap_height: 0,
                    waiting_for_hmu: false,
                    window: WINDOW_START,
                    window_min: WINDOW_MIN,
                    window_max: WINDOW_MAX_SLOW,
                    status: IncomingResourceStatus::Transferring,
                    retries_left: 0,
                    correlation: offer.correlation,
                    measured_rtt_ms: None,
                    part_timeout_factor: PART_TIMEOUT_FACTOR,
                    request_sent_at: None,
                    request_sent_bytes: 0,
                    awaiting_round_first_response: false,
                    received_byte_count: 0,
                    received_byte_count_at_request: 0,
                    request_response_bytes_per_second: 0,
                    data_bytes_per_second: 0,
                    inherited_eifr: None,
                    fast_rate_rounds: 0,
                    very_slow_rate_rounds: 0,
                },
                shape,
            )
            .map_err(|error| match error {
                ResourceTablePushError::TableFull | ResourceTablePushError::MemoryLimit => {
                    AcceptIncomingResourceError::TableFull
                }
                ResourceTablePushError::TransferTooLarge => {
                    AcceptIncomingResourceError::TransferTooLarge
                }
                ResourceTablePushError::TooManyParts => AcceptIncomingResourceError::TooManyParts,
            })?;

        self.write_names(index, 0, offer.initial_names);
        self.refresh_earliest_timeout();
        Ok(index)
    }

    /// RNS 1.4.2 `Resource.hashmap_update`. We refuse two shapes the reference mishandles:
    /// - Names past the part count: an `IndexError` off its fixed-length `[None] * total_parts` list, uncaught until the delivering interface's read loop, which tears the whole interface down.
    /// - A segment that skips ahead of the height: lands silently while `hashmap_height` (a fill count, not a prefix height) inflates, so `request_next` reads `None` holes.
    ///
    /// As the receiver we drive the requests, so a hole can only come from a sender we should not trust.
    pub fn apply_hashmap_update(
        &mut self,
        index: usize,
        segment: u64,
        names: &[u8],
    ) -> Result<usize, ApplyHashmapUpdateError> {
        let offset = usize::try_from(segment)
            .ok()
            .and_then(|segment| segment.checked_mul(HASHMAP_MAX_LEN))
            .ok_or(ApplyHashmapUpdateError::BeyondPartCount)?;

        if names.len() > HASHMAP_MAX_LEN * MAP_HASH_LEN {
            return Err(ApplyHashmapUpdateError::HashmapTooLong);
        }
        if !names.len().is_multiple_of(MAP_HASH_LEN) {
            return Err(ApplyHashmapUpdateError::HashmapRagged);
        }
        let entries = names.len() / MAP_HASH_LEN;
        let state = &self.table.states()[index];
        let end = offset
            .checked_add(entries)
            .ok_or(ApplyHashmapUpdateError::BeyondPartCount)?;
        if end > state.part_count {
            return Err(ApplyHashmapUpdateError::BeyondPartCount);
        }
        if offset > state.hashmap_height {
            return Err(ApplyHashmapUpdateError::SkipsAhead);
        }
        self.write_names(index, offset, names);
        let state = self.table.state_mut(index);
        state.waiting_for_hmu = false;
        Ok(state.hashmap_height)
    }

    fn write_names(&mut self, index: usize, offset: usize, names: &[u8]) {
        let entries = names.len() / MAP_HASH_LEN;
        let byte_len = entries * MAP_HASH_LEN;
        let byte_start = offset * MAP_HASH_LEN;
        let byte_end = byte_start + byte_len;
        let buffers = self.table.buffers_mut(index);
        buffers.part_names.as_flattened_mut()[byte_start..byte_end]
            .copy_from_slice(&names[..byte_len]);
        let height = offset + entries;
        let state = self.table.state_mut(index);
        state.hashmap_height = state.hashmap_height.max(height);
    }

    /// RNS 1.4.2 `Resource.receive_part`'s bookkeeping half.
    /// A part before the last must fill the sdu exactly: parts land at `index × sdu`, so a short middle part could only corrupt.
    pub fn place_part(
        &mut self,
        index: usize,
        at_part_index: usize,
        bytes: &[u8],
    ) -> PlacePartOutcome {
        let state = self.table.states()[index];
        if at_part_index >= state.part_count {
            return PlacePartOutcome::NoSuchPart;
        }
        let is_last = at_part_index + 1 == state.part_count;
        let fills_its_slot = bytes.len() == state.sdu || (is_last && bytes.len() < state.sdu);
        if !fills_its_slot {
            return PlacePartOutcome::WrongLength;
        }
        let offset = at_part_index * state.sdu;
        if offset + bytes.len() > state.sealed_transfer_bytes {
            return PlacePartOutcome::BeyondTransferEnd;
        }
        let buffers = self.table.buffers_mut(index);
        if buffers.part_flags[at_part_index] {
            return PlacePartOutcome::Duplicate;
        }
        buffers.transfer[offset..offset + bytes.len()].copy_from_slice(bytes);
        buffers.part_flags[at_part_index] = true;

        let flags = self.table.part_flags(index);
        let mut consecutive = state.consecutive_completed;
        let mut next = consecutive.map_or(0, |height| height + 1);
        while next < state.part_count && flags[next] {
            consecutive = Some(next);
            next += 1;
        }
        let state = self.table.state_mut(index);
        state.received_part_count += 1;
        state.outstanding_part_count = state.outstanding_part_count.saturating_sub(1);
        state.consecutive_completed = consecutive;
        PlacePartOutcome::Placed
    }

    pub fn lookup(&self, link_id: &LinkId, hash: &ResourceHash) -> Option<usize> {
        self.table
            .link_ids()
            .iter()
            .zip(self.table.hashes())
            .position(|(candidate_link, candidate_hash)| {
                candidate_link == link_id && candidate_hash == hash
            })
    }

    pub fn state(&self, index: usize) -> &IncomingResourceState {
        &self.table.states()[index]
    }

    pub fn state_mut(&mut self, index: usize) -> &mut IncomingResourceState {
        self.table.state_mut(index)
    }

    /// Never payload bytes: once complete, the transfer opens in place and the plaintext emerges as a sub-slice.
    pub fn sealed_transfer(&self, index: usize) -> &[u8] {
        let len = self.table.states()[index].sealed_transfer_bytes;
        &self.table.transfer(index)[..len]
    }

    /// The sealed transfer and its streamed-open slot, borrowed together: the frontier advance and the conclusion walk them in lockstep. Never payload bytes: the transfer opens in place and the plaintext emerges as a sub-slice.
    pub fn transfer_and_streamed_open_mut(
        &mut self,
        index: usize,
    ) -> (&mut [u8], &mut OpenProgress) {
        let len = self.table.states()[index].sealed_transfer_bytes;
        let (transfer, streamed_open) = self.table.transfer_and_streamed_open_mut(index);
        (&mut transfer[..len], streamed_open)
    }

    /// The read-only pair for the offload's owed-span scan and job view.
    pub fn transfer_and_streamed_open(&self, index: usize) -> (&[u8], &OpenProgress) {
        let len = self.table.states()[index].sealed_transfer_bytes;
        (
            &self.table.transfer(index)[..len],
            self.table.streamed_open(index),
        )
    }

    pub fn link_at(&self, index: usize) -> &LinkId {
        &self.table.link_ids()[index]
    }

    pub fn hash_at(&self, index: usize) -> &ResourceHash {
        &self.table.hashes()[index]
    }

    pub fn received_flags(&self, index: usize) -> &[bool] {
        let count = self.table.states()[index].part_count;
        &self.table.part_flags(index)[..count]
    }

    pub fn names_flat(&self, index: usize) -> &[u8] {
        let height = self.table.states()[index].hashmap_height;
        self.table.part_names(index)[..height].as_flattened()
    }

    pub fn remove(&mut self, link_id: &LinkId, hash: &ResourceHash) -> RemoveOutcome {
        match self.lookup(link_id, hash) {
            Some(index) => {
                self.table.swap_remove(index);
                self.refresh_earliest_timeout();
                RemoveOutcome::Removed
            }
            None => RemoveOutcome::NotTracked,
        }
    }

    pub fn set_timeout_at(&mut self, index: usize, timeout_at: Option<InstantMillis>) {
        self.table.set_timeout_at(index, timeout_at);
        self.refresh_earliest_timeout();
    }

    fn refresh_earliest_timeout(&mut self) {
        self.earliest_timeout = self.table.timeout_ats().iter().flatten().min().copied();
    }

    pub fn earliest_timeout_at(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_timeout,
            self.table.timeout_ats().iter().flatten().min().copied(),
            "earliest_timeout cache desynced from the timeout_ats column"
        );
        self.earliest_timeout
    }

    pub fn due_index(&self, now: InstantMillis) -> Option<usize> {
        self.table
            .timeout_ats()
            .iter()
            .position(|deadline| deadline.is_some_and(|at| at <= now))
    }

    pub fn transfer_capacity(&self) -> usize {
        self.table.transfer_capacity()
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn active_buffer_bytes(&self) -> usize {
        self.table.active_buffer_bytes()
    }

    pub fn buffer_memory_limit(&self) -> usize {
        self.table.buffer_memory_limit()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[cfg(feature = "alloc")]
impl OutgoingResources<super::HeapResourceTable<OutgoingResourceState>> {
    pub(crate) fn set_memory_limit(&mut self, bytes: usize) {
        self.table.set_memory_limit(bytes);
    }

    pub(crate) fn memory_limit(&self) -> usize {
        self.table.memory_limit()
    }
}

#[cfg(feature = "alloc")]
impl IncomingResources<super::HeapResourceTable<IncomingResourceState>> {
    pub(crate) fn set_memory_limit(&mut self, bytes: usize) {
        self.table.set_memory_limit(bytes);
    }

    pub(crate) fn memory_limit(&self) -> usize {
        self.table.memory_limit()
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use crate::routing::links::resources::max_part_count;

    type TestOutgoing = OutgoingResources<FixedResourceTable<OutgoingResourceState, 2, 1024, 3>>;
    type TestIncoming = IncomingResources<FixedResourceTable<IncomingResourceState, 2, 1024, 3>>;

    fn link_id(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    fn hash(byte: u8) -> ResourceHash {
        ResourceHash::new([byte; 32])
    }

    fn shape(transfer_bytes: usize, sdu: usize) -> ResourceBufferShape {
        ResourceBufferShape::try_for_transfer(transfer_bytes, sdu).unwrap()
    }

    fn fabricated(hash_byte: u8, sealed_transfer_bytes: usize, part_count: usize) -> BuiltResource {
        BuiltResource {
            sealed_transfer_bytes,
            part_count,
            hash: hash(hash_byte),
            salt_nonce: SaltNonce::new([hash_byte; 4]),
            expected_proof: ResourceProof::new([hash_byte; 32]),
            compression: ResourceCompression::Uncompressed,
            has_metadata: false,
            uncompressed_data_bytes: sealed_transfer_bytes as u64,
        }
    }

    fn track_segment(
        outgoing: &mut TestOutgoing,
        link: u8,
        hash_byte: u8,
        segment: ResourceSegment,
    ) -> Result<TrackLane, TrackOutgoingResourceError> {
        let lane = outgoing.lane_for(&link_id(link), &segment)?;
        outgoing
            .track_built(
                TrackedCommand {
                    link_id: link_id(link),
                    sdu: 464,
                    command_id: CommandId(7),
                    correlation: ResourceCorrelation::Unsolicited,
                    segment,
                },
                lane,
                shape(928, 464),
                |regions| {
                    regions.transfer[..3].copy_from_slice(&[hash_byte; 3]);
                    regions.hashmap[..8].copy_from_slice(&[hash_byte; 8]);
                    Ok(fabricated(hash_byte, 928, 2))
                },
            )
            .map(|_| lane)
    }

    fn track(
        outgoing: &mut TestOutgoing,
        link: u8,
        hash_byte: u8,
    ) -> Result<TrackLane, TrackOutgoingResourceError> {
        track_segment(outgoing, link, hash_byte, ResourceSegment::whole(930))
    }

    fn offer<'a>(hash_byte: u8, initial_names: &'a [u8]) -> AcceptedResource<'a> {
        AcceptedResource {
            hash: hash(hash_byte),
            salt_nonce: SaltNonce::new([hash_byte; 4]),
            compression: ResourceCompression::Uncompressed,
            has_metadata: false,
            uncompressed_data_bytes: 900,
            segment_index: 1,
            total_segment_count: 1,
            sealed_transfer_bytes: 980,
            part_count: 3,
            sdu: 464,
            correlation: ResourceCorrelation::Unsolicited,
            initial_names,
        }
    }

    #[test]
    fn a_tracked_build_lands_its_bytes_names_and_state_in_the_slot() {
        let mut outgoing = TestOutgoing::default();
        let tracked = track(&mut outgoing, 1, 0xAB).unwrap();

        assert_eq!(tracked, TrackLane::Live);
        let index = outgoing.lookup(&link_id(1), &hash(0xAB)).unwrap();
        assert_eq!(outgoing.sealed_transfer(index).len(), 928);
        assert_eq!(&outgoing.sealed_transfer(index)[..3], &[0xAB; 3]);
        assert_eq!(outgoing.names_flat(index), &[0xAB; 8]);
        let state = outgoing.state(index);
        assert_eq!(state.part_count, 2);
        assert_eq!(state.sdu, 464);
        assert_eq!(state.status, OutgoingResourceStatus::Advertised);
        assert_eq!(state.command_id, CommandId(7));
    }

    #[test]
    fn one_outgoing_resource_per_link_like_the_reference() {
        let mut outgoing = TestOutgoing::default();
        track(&mut outgoing, 1, 0xAB).unwrap();
        assert_eq!(
            track(&mut outgoing, 1, 0xCD).unwrap_err(),
            TrackOutgoingResourceError::LinkBusy,
        );
        track(&mut outgoing, 2, 0xCD).unwrap();
        assert_eq!(
            track(&mut outgoing, 3, 0xEE).unwrap_err(),
            TrackOutgoingResourceError::TableFull,
        );
    }

    #[test]
    fn the_exact_next_continuation_stages_and_anything_else_stays_busy() {
        let mut outgoing = TestOutgoing::default();
        let segment = |index, total| ResourceSegment {
            index,
            total_segments: total,
            total_data_bytes: 2_000,
        };
        track_segment(&mut outgoing, 1, 0xAB, segment(1, 3)).unwrap();

        assert_eq!(
            track_segment(&mut outgoing, 1, 0xCD, segment(3, 3)).unwrap_err(),
            TrackOutgoingResourceError::LinkBusy,
            "a continuation must be the very next index",
        );
        assert_eq!(
            track_segment(&mut outgoing, 1, 0xCD, segment(2, 4)).unwrap_err(),
            TrackOutgoingResourceError::LinkBusy,
            "a different segment count is a different transfer",
        );
        assert_eq!(
            track(&mut outgoing, 1, 0xCD).unwrap_err(),
            TrackOutgoingResourceError::LinkBusy,
            "an unrelated whole send never stages",
        );

        assert_eq!(
            track_segment(&mut outgoing, 1, 0xCD, segment(2, 3)).unwrap(),
            TrackLane::Staged,
        );
        let staged = outgoing.staged_index(&link_id(1)).unwrap();
        assert_eq!(
            outgoing.state(staged).status,
            OutgoingResourceStatus::StagedSealed,
            "a built staged row waits fully sealed",
        );
        assert_eq!(outgoing.state(staged).segment_index, 2);

        assert_eq!(
            track_segment(&mut outgoing, 2, 0xEE, segment(3, 3)).unwrap_err(),
            TrackOutgoingResourceError::TableFull,
            "the staged row occupies a real slot",
        );
    }

    #[test]
    fn a_failed_build_releases_its_slot() {
        let mut outgoing = TestOutgoing::default();
        let refused = outgoing.track_built(
            TrackedCommand {
                link_id: link_id(1),
                sdu: 464,
                command_id: CommandId(7),
                correlation: ResourceCorrelation::Unsolicited,
                segment: ResourceSegment::whole(930),
            },
            TrackLane::Live,
            shape(928, 464),
            |_| Err(BuildOutgoingResourceError::SduTooSmall),
        );
        assert_eq!(
            refused.unwrap_err(),
            TrackOutgoingResourceError::Build(BuildOutgoingResourceError::SduTooSmall),
        );
        assert!(outgoing.is_empty());
        track(&mut outgoing, 1, 0xAB).unwrap();
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn heap_budget_pressure_uses_the_existing_table_full_outcomes() {
        let mut outgoing = OutgoingResources::<HeapResourceTable<OutgoingResourceState>>::default();
        outgoing.set_memory_limit(0);
        let refused = outgoing.track_built(
            TrackedCommand {
                link_id: link_id(1),
                sdu: 464,
                command_id: CommandId(7),
                correlation: ResourceCorrelation::Unsolicited,
                segment: ResourceSegment::whole(930),
            },
            TrackLane::Live,
            shape(928, 464),
            |_| unreachable!("budget pressure must refuse before building"),
        );
        assert_eq!(refused, Err(TrackOutgoingResourceError::TableFull));

        let mut incoming = IncomingResources::<HeapResourceTable<IncomingResourceState>>::default();
        incoming.set_memory_limit(0);
        assert_eq!(
            incoming.accept(link_id(1), offer(0xAB, &[])),
            Err(AcceptIncomingResourceError::TableFull),
        );
    }

    #[test]
    fn a_build_cannot_commit_dimensions_other_than_its_reserved_shape() {
        let mut outgoing = TestOutgoing::default();
        let refused = outgoing.track_built(
            TrackedCommand {
                link_id: link_id(1),
                sdu: 464,
                command_id: CommandId(7),
                correlation: ResourceCorrelation::Unsolicited,
                segment: ResourceSegment::whole(930),
            },
            TrackLane::Live,
            shape(928, 464),
            |_| Ok(fabricated(0xAB, 927, 2)),
        );
        assert_eq!(
            refused.unwrap_err(),
            TrackOutgoingResourceError::Build(BuildOutgoingResourceError::BufferShapeMismatch,),
        );
        assert!(outgoing.is_empty());
        track(&mut outgoing, 1, 0xAB).unwrap();
    }

    #[test]
    fn marking_sent_counts_each_part_once() {
        let mut outgoing = TestOutgoing::default();
        track(&mut outgoing, 1, 0xAB).unwrap();
        let index = outgoing.lookup(&link_id(1), &hash(0xAB)).unwrap();

        assert_eq!(outgoing.mark_sent(index, 0), PartSendOutcome::FirstSend);
        assert_eq!(outgoing.mark_sent(index, 0), PartSendOutcome::Resend);
        assert_eq!(outgoing.mark_sent(index, 1), PartSendOutcome::FirstSend);
        assert_eq!(outgoing.mark_sent(index, 2), PartSendOutcome::NoSuchPart);
        assert_eq!(outgoing.state(index).sent_part_count, 2);
    }

    #[test]
    fn a_removed_resource_frees_its_link_and_slot_flags() {
        let mut outgoing = TestOutgoing::default();
        track(&mut outgoing, 1, 0xAB).unwrap();
        let index = outgoing.lookup(&link_id(1), &hash(0xAB)).unwrap();
        outgoing.mark_sent(index, 0);

        assert_eq!(
            outgoing.remove(&link_id(1), &hash(0xAB)),
            RemoveOutcome::Removed
        );
        assert_eq!(
            outgoing.remove(&link_id(1), &hash(0xAB)),
            RemoveOutcome::NotTracked
        );
        assert!(outgoing.is_empty());

        track(&mut outgoing, 1, 0xCD).unwrap();
        let index = outgoing.lookup(&link_id(1), &hash(0xCD)).unwrap();
        assert_eq!(
            outgoing.mark_sent(index, 0),
            PartSendOutcome::FirstSend,
            "a reused slot must arrive with cleared flags",
        );
    }

    #[test]
    fn an_accepted_offer_lands_with_its_initial_names() {
        let mut incoming = TestIncoming::default();
        let names = [[0x11u8; 4], [0x22; 4]].as_flattened().to_vec();
        let index = incoming.accept(link_id(1), offer(0xAB, &names)).unwrap();

        let state = incoming.state(index);
        assert_eq!(state.part_count, 3);
        assert_eq!(state.hashmap_height, 2);
        assert_eq!(state.window, WINDOW_START);
        assert_eq!(state.window_max, WINDOW_MAX_SLOW);
        assert_eq!(state.consecutive_completed, None);
        assert_eq!(incoming.names_flat(index), &names[..]);
    }

    #[test]
    fn the_accept_gate_refuses_what_the_store_cannot_hold() {
        let mut incoming = TestIncoming::default();
        incoming.accept(link_id(1), offer(0xAB, &[])).unwrap();
        assert_eq!(
            incoming.accept(link_id(1), offer(0xAB, &[])).unwrap_err(),
            AcceptIncomingResourceError::AlreadyReceiving,
        );

        let mut too_large = offer(0xCD, &[]);
        too_large.sealed_transfer_bytes = 1025;
        assert_eq!(
            incoming.accept(link_id(1), too_large).unwrap_err(),
            AcceptIncomingResourceError::TransferTooLarge,
        );

        let mut too_many = offer(0xCD, &[]);
        too_many.sdu = 245;
        too_many.part_count = 4;
        assert_eq!(
            incoming.accept(link_id(1), too_many).unwrap_err(),
            AcceptIncomingResourceError::TooManyParts,
        );

        let mut mismatched_parts = offer(0xCD, &[]);
        mismatched_parts.part_count = 2;
        assert_eq!(
            incoming.accept(link_id(1), mismatched_parts).unwrap_err(),
            AcceptIncomingResourceError::PartCountMismatch,
        );

        let mut empty = offer(0xCD, &[]);
        empty.sealed_transfer_bytes = 0;
        empty.part_count = 0;
        assert_eq!(
            incoming.accept(link_id(1), empty).unwrap_err(),
            AcceptIncomingResourceError::EmptyTransfer,
        );

        let mut zero_sdu = offer(0xCD, &[]);
        zero_sdu.sdu = 0;
        assert_eq!(
            incoming.accept(link_id(1), zero_sdu).unwrap_err(),
            AcceptIncomingResourceError::SduTooSmall,
        );

        let too_long_names = [0u8; (HASHMAP_MAX_LEN + 1) * MAP_HASH_LEN];
        assert_eq!(
            incoming
                .accept(link_id(1), offer(0xCD, &too_long_names))
                .unwrap_err(),
            AcceptIncomingResourceError::HashmapTooLong,
        );

        assert_eq!(
            incoming
                .accept(link_id(1), offer(0xCD, &[0u8; MAP_HASH_LEN + 1]))
                .unwrap_err(),
            AcceptIncomingResourceError::HashmapRagged,
        );

        assert_eq!(
            incoming
                .accept(link_id(1), offer(0xCD, &[0u8; 4 * MAP_HASH_LEN]))
                .unwrap_err(),
            AcceptIncomingResourceError::HashmapBeyondPartCount,
        );

        incoming.accept(link_id(2), offer(0xCD, &[])).unwrap();
        assert_eq!(
            incoming.accept(link_id(3), offer(0xEE, &[])).unwrap_err(),
            AcceptIncomingResourceError::TableFull,
        );
    }

    #[test]
    fn hashmap_updates_extend_the_height_and_refuse_misfits() {
        let mut incoming = IncomingResources::<HeapResourceTable<IncomingResourceState>>::default();
        let mut big = offer(0xAB, &[]);
        big.part_count = 100;
        big.sealed_transfer_bytes = 100 * 464;
        let index = incoming.accept(link_id(1), big).unwrap();

        assert_eq!(
            incoming
                .apply_hashmap_update(index, 1, &[0u8; 8])
                .unwrap_err(),
            ApplyHashmapUpdateError::SkipsAhead,
        );

        let segment_zero = std::vec![0x55u8; 74 * MAP_HASH_LEN];
        assert_eq!(
            incoming
                .apply_hashmap_update(index, 0, &segment_zero)
                .unwrap(),
            74,
        );
        assert!(!incoming.state(index).waiting_for_hmu);

        let tail = std::vec![0x66u8; 26 * MAP_HASH_LEN];
        assert_eq!(incoming.apply_hashmap_update(index, 1, &tail).unwrap(), 100);

        assert_eq!(
            incoming
                .apply_hashmap_update(index, 1, &std::vec![0u8; 27 * MAP_HASH_LEN])
                .unwrap_err(),
            ApplyHashmapUpdateError::BeyondPartCount,
        );
        assert_eq!(
            incoming
                .apply_hashmap_update(index, 1, &[0u8; (HASHMAP_MAX_LEN + 1) * MAP_HASH_LEN])
                .unwrap_err(),
            ApplyHashmapUpdateError::HashmapTooLong,
        );
        assert_eq!(
            incoming
                .apply_hashmap_update(index, 1, &[0u8; MAP_HASH_LEN + 1])
                .unwrap_err(),
            ApplyHashmapUpdateError::HashmapRagged,
        );
        assert_eq!(
            incoming
                .apply_hashmap_update(index, u64::MAX, &[])
                .unwrap_err(),
            ApplyHashmapUpdateError::BeyondPartCount,
        );
        let overflowing_segment = u64::try_from(usize::MAX / HASHMAP_MAX_LEN).unwrap();
        assert_eq!(
            incoming
                .apply_hashmap_update(
                    index,
                    overflowing_segment,
                    &[0u8; HASHMAP_MAX_LEN * MAP_HASH_LEN],
                )
                .unwrap_err(),
            ApplyHashmapUpdateError::BeyondPartCount,
        );
    }

    #[test]
    fn placed_parts_advance_the_consecutive_height_across_gaps() {
        let mut incoming = TestIncoming::default();
        let index = incoming.accept(link_id(1), offer(0xAB, &[])).unwrap();
        incoming.state_mut(index).outstanding_part_count = 3;

        assert_eq!(
            incoming.place_part(index, 2, &[0x33; 52]),
            PlacePartOutcome::Placed
        );
        assert_eq!(incoming.state(index).consecutive_completed, None);

        assert_eq!(
            incoming.place_part(index, 0, &[0x11; 464]),
            PlacePartOutcome::Placed
        );
        assert_eq!(incoming.state(index).consecutive_completed, Some(0));

        assert_eq!(
            incoming.place_part(index, 1, &[0x22; 464]),
            PlacePartOutcome::Placed
        );
        let state = incoming.state(index);
        assert_eq!(state.consecutive_completed, Some(2));
        assert_eq!(state.received_part_count, 3);
        assert_eq!(state.outstanding_part_count, 0);

        assert_eq!(&incoming.sealed_transfer(index)[..464], &[0x11; 464][..]);
        assert_eq!(&incoming.sealed_transfer(index)[464..928], &[0x22; 464][..]);
        assert_eq!(&incoming.sealed_transfer(index)[928..], &[0x33; 52][..]);
    }

    #[test]
    fn misfit_parts_are_dropped_silently_like_the_reference() {
        let mut incoming = TestIncoming::default();
        let index = incoming.accept(link_id(1), offer(0xAB, &[])).unwrap();

        assert_eq!(
            incoming.place_part(index, 0, &[0x11; 464]),
            PlacePartOutcome::Placed
        );
        assert_eq!(
            incoming.place_part(index, 0, &[0x11; 464]),
            PlacePartOutcome::Duplicate
        );
        assert_eq!(
            incoming.place_part(index, 3, &[0x11; 464]),
            PlacePartOutcome::NoSuchPart
        );
        assert_eq!(
            incoming.place_part(index, 1, &[0x22; 100]),
            PlacePartOutcome::WrongLength,
            "a short middle part would misalign the stream",
        );
        assert_eq!(
            incoming.place_part(index, 2, &[0x33; 60]),
            PlacePartOutcome::BeyondTransferEnd,
            "the last part may be short but never past the transfer size",
        );
        assert_eq!(incoming.state(index).received_part_count, 1);
    }

    #[test]
    fn the_fixed_part_capacity_covers_its_transfer_bytes() {
        assert_eq!(max_part_count(1024), 3);
        let table = FixedResourceTable::<OutgoingResourceState, 2, 1024, 3>::default();
        assert_eq!(table.part_capacity(), 3);
        assert_eq!(table.transfer_capacity(), 1024);
    }
}
