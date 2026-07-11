use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::routing::links::resources::build_outgoing::{
    BuildOutgoingResourceError, BuildRegions, BuiltResource,
};
use crate::routing::links::resources::{
    ResourceCompression, ResourceCorrelation, ResourceHash, ResourceProof, SaltNonce,
    HASHMAP_MAX_LEN, MAP_HASH_LEN, PART_TIMEOUT_FACTOR, WINDOW_MAX_SLOW, WINDOW_MIN, WINDOW_START,
};
use crate::routing::links::LinkId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingResourceStatus {
    Advertised,
    Transferring,
    AwaitingProof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncomingResourceStatus {
    Transferring,
    AwaitingDecompression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutgoingResourceState {
    pub salt_nonce: SaltNonce,
    pub expected_proof: ResourceProof,
    pub sealed_transfer_len: usize,
    pub uncompressed_data_len: u64,
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
            sealed_transfer_len: 0,
            uncompressed_data_len: 0,
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
    pub uncompressed_data_len: u64,
    pub segment_index: u64,
    pub total_segments: u64,
    pub sealed_transfer_len: usize,
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
    pub request_sent_byte_len: u64,
    pub awaiting_round_first_response: bool,
    pub received_byte_count: u64,
    pub received_byte_count_at_request: u64,
    pub request_response_byte_rate: u64,
    pub data_byte_rate: u64,
    pub inherited_eifr: Option<u64>,
    pub fast_rate_rounds: u8,
    pub very_slow_rate_rounds: u8,
}

/// The vacant-slot value for fixed-capacity tables to initialize with, never a live transfer's state; [accept](IncomingResources::accept) writes every field.
impl Default for IncomingResourceState {
    fn default() -> Self {
        Self {
            salt_nonce: SaltNonce::new([0; 4]),
            compression: ResourceCompression::Uncompressed,
            has_metadata: false,
            uncompressed_data_len: 0,
            segment_index: 1,
            total_segments: 1,
            sealed_transfer_len: 0,
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
            request_sent_byte_len: 0,
            awaiting_round_first_response: false,
            received_byte_count: 0,
            received_byte_count_at_request: 0,
            request_response_byte_rate: 0,
            data_byte_rate: 0,
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
}

pub trait ResourceTable<State> {
    fn capacity(&self) -> usize;
    fn transfer_capacity(&self) -> usize;
    fn part_capacity(&self) -> usize;
    fn len(&self) -> usize;

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
    fn buffers_mut(&mut self, index: usize) -> ResourceBuffers<'_>;

    fn push(
        &mut self,
        link_id: LinkId,
        hash: ResourceHash,
        state: State,
    ) -> Result<usize, ResourceTablePushError>;
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackOutgoingResourceError {
    TableFull,
    LinkBusy,
    Build(BuildOutgoingResourceError),
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
    /// A failed build releases the slot untouched. One resource per link at a time.
    /// RNS 1.3.5 `Link.ready_for_new_resource`.
    pub fn track(
        &mut self,
        link_id: LinkId,
        sdu: usize,
        command_id: CommandId,
        correlation: ResourceCorrelation,
        build: impl FnOnce(BuildRegions<'_>) -> Result<BuiltResource, BuildOutgoingResourceError>,
    ) -> Result<ResourceHash, TrackOutgoingResourceError> {
        if self.table.link_ids().contains(&link_id) {
            return Err(TrackOutgoingResourceError::LinkBusy);
        }

        let index = self
            .table
            .push(
                link_id,
                ResourceHash::new([0; 32]),
                OutgoingResourceState::default(),
            )
            .map_err(|ResourceTablePushError::TableFull| TrackOutgoingResourceError::TableFull)?;

        let buffers = self.table.buffers_mut(index);

        match build(BuildRegions {
            transfer: buffers.transfer,
            hashmap: buffers.part_names.as_flattened_mut(),
        }) {
            Ok(built) => {
                self.table.set_hash(index, built.hash);
                *self.table.state_mut(index) = OutgoingResourceState {
                    salt_nonce: built.salt_nonce,
                    expected_proof: built.expected_proof,
                    sealed_transfer_len: built.sealed_transfer_len,
                    uncompressed_data_len: built.uncompressed_data_len,
                    segment_index: 1,
                    total_segments: 1,
                    original_hash: built.hash,
                    compression: built.compression,
                    has_metadata: built.has_metadata,
                    part_count: built.part_count,
                    sdu,
                    scope_start: 0,
                    sent_part_count: 0,
                    status: OutgoingResourceStatus::Advertised,
                    retries_left: 0,
                    command_id,
                    correlation,
                };
                self.refresh_earliest_timeout();
                Ok(built.hash)
            }
            Err(error) => {
                self.table.swap_remove(index);
                self.refresh_earliest_timeout();
                Err(TrackOutgoingResourceError::Build(error))
            }
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

    pub fn sealed_transfer(&self, index: usize) -> &[u8] {
        let len = self.table.states()[index].sealed_transfer_len;
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

    /// The distinction RNS 1.3.5 draws between `part.send()` (counted toward `sent_parts`) and `part.resend()` (not counted).
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
    pub uncompressed_data_len: u64,
    pub segment_index: u64,
    pub sealed_transfer_len: usize,
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
    TransferTooLarge,
    TooManyParts,
    HashmapTooLong,
    /// The name bytes are not a whole number of 4-byte map hashes: a torn name at the tail.
    HashmapRagged,
    HashmapBeyondPartCount,
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

impl<C: ResourceTable<IncomingResourceState>> IncomingResources<C> {
    /// The capacity and shape gate the engine asks at accept; policy gating happens before the offer ever reaches the table.
    /// The duplicate refusal is RNS 1.3.5 `Resource.accept`'s `has_incoming_resource` registration gate.
    pub fn accept(
        &mut self,
        link_id: LinkId,
        offer: AcceptedResource<'_>,
    ) -> Result<usize, AcceptIncomingResourceError> {
        if offer.sealed_transfer_len > self.table.transfer_capacity() {
            return Err(AcceptIncomingResourceError::TransferTooLarge);
        }
        if offer.part_count > self.table.part_capacity() {
            return Err(AcceptIncomingResourceError::TooManyParts);
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
                    uncompressed_data_len: offer.uncompressed_data_len,
                    segment_index: offer.segment_index,
                    total_segments: offer.total_segment_count,
                    sealed_transfer_len: offer.sealed_transfer_len,
                    part_count: offer.part_count,
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
                    request_sent_byte_len: 0,
                    awaiting_round_first_response: false,
                    received_byte_count: 0,
                    received_byte_count_at_request: 0,
                    request_response_byte_rate: 0,
                    data_byte_rate: 0,
                    inherited_eifr: None,
                    fast_rate_rounds: 0,
                    very_slow_rate_rounds: 0,
                },
            )
            .map_err(|ResourceTablePushError::TableFull| AcceptIncomingResourceError::TableFull)?;

        self.write_names(index, 0, offer.initial_names);
        self.refresh_earliest_timeout();
        Ok(index)
    }

    /// RNS 1.3.5 `Resource.hashmap_update`. We refuse two shapes the reference mishandles:
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
        if offset + entries > state.part_count {
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

    /// RNS 1.3.5 `Resource.receive_part`'s bookkeeping half.
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
        if offset + bytes.len() > state.sealed_transfer_len {
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
        let len = self.table.states()[index].sealed_transfer_len;
        &self.table.transfer(index)[..len]
    }

    /// Never payload bytes: once complete, the transfer opens in place and the plaintext emerges as a sub-slice.
    pub fn sealed_transfer_mut(&mut self, index: usize) -> &mut [u8] {
        let len = self.table.states()[index].sealed_transfer_len;
        &mut self.table.buffers_mut(index).transfer[..len]
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

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
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

    fn fabricated(hash_byte: u8, sealed_transfer_len: usize, part_count: usize) -> BuiltResource {
        BuiltResource {
            sealed_transfer_len,
            part_count,
            hash: hash(hash_byte),
            salt_nonce: SaltNonce::new([hash_byte; 4]),
            expected_proof: ResourceProof::new([hash_byte; 32]),
            compression: ResourceCompression::Uncompressed,
            has_metadata: false,
            uncompressed_data_len: sealed_transfer_len as u64,
        }
    }

    fn track(
        outgoing: &mut TestOutgoing,
        link: u8,
        hash_byte: u8,
    ) -> Result<ResourceHash, TrackOutgoingResourceError> {
        outgoing.track(
            link_id(link),
            464,
            CommandId(7),
            ResourceCorrelation::Unsolicited,
            |regions| {
                regions.transfer[..3].copy_from_slice(&[hash_byte; 3]);
                regions.hashmap[..8].copy_from_slice(&[hash_byte; 8]);
                Ok(fabricated(hash_byte, 930, 2))
            },
        )
    }

    fn offer<'a>(hash_byte: u8, initial_names: &'a [u8]) -> AcceptedResource<'a> {
        AcceptedResource {
            hash: hash(hash_byte),
            salt_nonce: SaltNonce::new([hash_byte; 4]),
            compression: ResourceCompression::Uncompressed,
            has_metadata: false,
            uncompressed_data_len: 900,
            segment_index: 1,
            total_segment_count: 1,
            sealed_transfer_len: 980,
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

        assert_eq!(tracked, hash(0xAB));
        let index = outgoing.lookup(&link_id(1), &hash(0xAB)).unwrap();
        assert_eq!(outgoing.sealed_transfer(index).len(), 930);
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
    fn a_failed_build_releases_its_slot() {
        let mut outgoing = TestOutgoing::default();
        let refused = outgoing.track(
            link_id(1),
            464,
            CommandId(7),
            ResourceCorrelation::Unsolicited,
            |_| Err(BuildOutgoingResourceError::SduTooSmall),
        );
        assert_eq!(
            refused.unwrap_err(),
            TrackOutgoingResourceError::Build(BuildOutgoingResourceError::SduTooSmall),
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
        too_large.sealed_transfer_len = 1025;
        assert_eq!(
            incoming.accept(link_id(1), too_large).unwrap_err(),
            AcceptIncomingResourceError::TransferTooLarge,
        );

        let mut too_many = offer(0xCD, &[]);
        too_many.part_count = 4;
        assert_eq!(
            incoming.accept(link_id(1), too_many).unwrap_err(),
            AcceptIncomingResourceError::TooManyParts,
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
        big.sealed_transfer_len = 100 * 464;
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
