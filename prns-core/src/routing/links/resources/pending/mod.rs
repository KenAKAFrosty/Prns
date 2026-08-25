//! Authenticated Resource advertisements waiting for an incoming transfer row.
//!
//! This store owns only the advertisement metadata needed to admit the transfer
//! later. Advertised payload bytes remain exclusively at the sender until a row
//! is admitted and Prns emits the first part request.

mod fixed;
#[cfg(feature = "external-alloc")]
mod fixed_heap;
#[cfg(feature = "alloc")]
mod heap;

pub use fixed::{FixedPendingResourceOfferTable, NoPendingResourceOfferTable};
#[cfg(feature = "external-alloc")]
pub use fixed_heap::FixedHeapPendingResourceOfferTable;
#[cfg(feature = "alloc")]
pub use heap::{HeapPendingResourceOfferTable, HOST_PENDING_RESOURCE_OFFER_CAPACITY};

use crate::engine::InstantMillis;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::table::AcceptedResource;
use crate::routing::links::resources::{
    ResourceBufferShape, ResourceBufferShapeError, ResourceCorrelation, ResourceHash,
    HASHMAP_MAX_LEN, MAP_HASH_LEN,
};
use crate::routing::links::LinkId;
use crate::units::RttMillis;

const INITIAL_NAMES_BYTES: usize = HASHMAP_MAX_LEN * MAP_HASH_LEN;
const PENDING_RESOURCE_WAIT_CEILING_MS: u64 = 10_000;

/// A pending advertisement gets two complete Resource advertisement-response
/// windows, capped so admission pressure cannot pin metadata indefinitely.
pub const fn pending_resource_wait_ms(rtt: RttMillis) -> u64 {
    let calculated = rtt
        .millis()
        .saturating_mul(6)
        .saturating_add(1_000)
        .saturating_mul(2);
    if calculated < PENDING_RESOURCE_WAIT_CEILING_MS {
        calculated
    } else {
        PENDING_RESOURCE_WAIT_CEILING_MS
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PendingResourcePriority {
    Response = 0,
    PeerRequest = 1,
    Unsolicited = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingResourceOfferError {
    BufferShape(ResourceBufferShapeError),
    PartCountMismatch,
    RequestSettingsNotApplicable,
    HashmapTooLong,
    HashmapRagged,
    HashmapBeyondPartCount,
}

/// One owned Resource advertisement whose transfer buffers have not yet been
/// allocated. The receive path owns authentication, correlation, and policy
/// approval before constructing this row.
#[derive(Debug)]
pub struct PendingResourceOffer {
    link_id: LinkId,
    original_hash: ResourceHash,
    accepted: PendingAcceptedResource,
    first_arrived_at: InstantMillis,
    frozen_link_rtt: RttMillis,
    initial_names_len: u16,
    initial_names: [u8; INITIAL_NAMES_BYTES],
}

#[derive(Debug)]
struct PendingAcceptedResource {
    hash: ResourceHash,
    salt_nonce: crate::routing::links::resources::SaltNonce,
    compression: crate::routing::links::resources::ResourceCompression,
    has_metadata: bool,
    uncompressed_data_bytes: u64,
    segment_index: u64,
    total_segment_count: u64,
    sealed_transfer_bytes: usize,
    sdu: usize,
    correlation: PendingResourceCorrelation,
}

/// Inbound offers retain only their wire correlation. The response timeout and
/// byte limit in [`ResourceCorrelation::Request`] configure locally sent
/// requests and have no meaning for a peer's request advertisement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingResourceCorrelation {
    Unsolicited,
    Request(RequestId),
    Response(RequestId),
}

impl PendingResourceCorrelation {
    const fn try_from_resource_correlation(
        correlation: ResourceCorrelation,
    ) -> Result<Self, PendingResourceOfferError> {
        match correlation {
            ResourceCorrelation::Unsolicited => Ok(Self::Unsolicited),
            ResourceCorrelation::Request {
                id,
                response_timeout: crate::engine::RequestResponseTimeout::LinkDefault,
                maximum_response_bytes: crate::units::ByteLimit::Unlimited,
            } => Ok(Self::Request(id)),
            ResourceCorrelation::Request { .. } => {
                Err(PendingResourceOfferError::RequestSettingsNotApplicable)
            }
            ResourceCorrelation::Response(id) => Ok(Self::Response(id)),
        }
    }

    const fn as_resource_correlation(self) -> ResourceCorrelation {
        match self {
            Self::Unsolicited => ResourceCorrelation::Unsolicited,
            Self::Request(id) => ResourceCorrelation::Request {
                id,
                response_timeout: crate::engine::RequestResponseTimeout::LinkDefault,
                maximum_response_bytes: crate::units::ByteLimit::Unlimited,
            },
            Self::Response(id) => ResourceCorrelation::Response(id),
        }
    }

    const fn priority(self) -> PendingResourcePriority {
        match self {
            Self::Response(_) => PendingResourcePriority::Response,
            Self::Request(_) => PendingResourcePriority::PeerRequest,
            Self::Unsolicited => PendingResourcePriority::Unsolicited,
        }
    }
}

impl PendingResourceOffer {
    pub fn try_from_accepted(
        link_id: LinkId,
        original_hash: ResourceHash,
        accepted: AcceptedResource<'_>,
        first_arrived_at: InstantMillis,
        frozen_link_rtt: RttMillis,
    ) -> Result<Self, PendingResourceOfferError> {
        let shape =
            ResourceBufferShape::try_for_transfer(accepted.sealed_transfer_bytes, accepted.sdu)
                .map_err(PendingResourceOfferError::BufferShape)?;
        if accepted.part_count != shape.part_count() {
            return Err(PendingResourceOfferError::PartCountMismatch);
        }
        if accepted.initial_names.len() > INITIAL_NAMES_BYTES {
            return Err(PendingResourceOfferError::HashmapTooLong);
        }
        if !accepted.initial_names.len().is_multiple_of(MAP_HASH_LEN) {
            return Err(PendingResourceOfferError::HashmapRagged);
        }
        if accepted.initial_names.len() / MAP_HASH_LEN > shape.part_count() {
            return Err(PendingResourceOfferError::HashmapBeyondPartCount);
        }
        let initial_names_len = u16::try_from(accepted.initial_names.len())
            .map_err(|_error| PendingResourceOfferError::HashmapTooLong)?;
        let correlation =
            PendingResourceCorrelation::try_from_resource_correlation(accepted.correlation)?;
        let mut initial_names = [0u8; INITIAL_NAMES_BYTES];
        initial_names[..accepted.initial_names.len()].copy_from_slice(accepted.initial_names);
        Ok(Self {
            link_id,
            original_hash,
            accepted: PendingAcceptedResource {
                hash: accepted.hash,
                salt_nonce: accepted.salt_nonce,
                compression: accepted.compression,
                has_metadata: accepted.has_metadata,
                uncompressed_data_bytes: accepted.uncompressed_data_bytes,
                segment_index: accepted.segment_index,
                total_segment_count: accepted.total_segment_count,
                sealed_transfer_bytes: accepted.sealed_transfer_bytes,
                sdu: accepted.sdu,
                correlation,
            },
            first_arrived_at,
            frozen_link_rtt,
            initial_names_len,
            initial_names,
        })
    }

    pub const fn link_id(&self) -> LinkId {
        self.link_id
    }

    pub const fn hash(&self) -> ResourceHash {
        self.accepted.hash
    }

    pub const fn original_hash(&self) -> ResourceHash {
        self.original_hash
    }

    pub const fn first_arrived_at(&self) -> InstantMillis {
        self.first_arrived_at
    }

    pub const fn frozen_link_rtt(&self) -> RttMillis {
        self.frozen_link_rtt
    }

    pub const fn wait_deadline(&self) -> InstantMillis {
        InstantMillis(
            self.first_arrived_at
                .0
                .saturating_add(pending_resource_wait_ms(self.frozen_link_rtt)),
        )
    }

    pub const fn priority(&self) -> PendingResourcePriority {
        self.accepted.correlation.priority()
    }

    pub const fn correlation(&self) -> ResourceCorrelation {
        self.accepted.correlation.as_resource_correlation()
    }

    pub fn accepted(&self) -> AcceptedResource<'_> {
        AcceptedResource {
            hash: self.accepted.hash,
            salt_nonce: self.accepted.salt_nonce,
            compression: self.accepted.compression,
            has_metadata: self.accepted.has_metadata,
            uncompressed_data_bytes: self.accepted.uncompressed_data_bytes,
            segment_index: self.accepted.segment_index,
            total_segment_count: self.accepted.total_segment_count,
            sealed_transfer_bytes: self.accepted.sealed_transfer_bytes,
            part_count: self
                .accepted
                .sealed_transfer_bytes
                .div_ceil(self.accepted.sdu),
            sdu: self.accepted.sdu,
            correlation: self.accepted.correlation.as_resource_correlation(),
            initial_names: &self.initial_names[..self.initial_names_len as usize],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingResourceOfferTableFull;

pub trait PendingResourceOfferTable {
    fn capacity(&self) -> usize;
    fn offers(&self) -> &[PendingResourceOffer];
    fn push(&mut self, offer: PendingResourceOffer) -> Result<(), PendingResourceOfferTableFull>;
    fn swap_remove(&mut self, index: usize) -> PendingResourceOffer;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum QueuePendingResourceOfferOutcome {
    Queued,
    RetryCoalesced,
    TableFull,
}

#[derive(Debug, Default)]
pub struct PendingResourceOffers<C: PendingResourceOfferTable> {
    table: C,
}

impl<C: PendingResourceOfferTable> PendingResourceOffers<C> {
    pub fn capacity(&self) -> usize {
        self.table.capacity()
    }

    pub fn len(&self) -> usize {
        self.table.offers().len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.offers().is_empty()
    }

    pub fn offers(&self) -> &[PendingResourceOffer] {
        self.table.offers()
    }

    pub fn contains(&self, link_id: &LinkId, hash: &ResourceHash) -> bool {
        self.table
            .offers()
            .iter()
            .any(|offer| &offer.link_id() == link_id && &offer.hash() == hash)
    }

    pub fn queue(&mut self, offer: PendingResourceOffer) -> QueuePendingResourceOfferOutcome {
        if self
            .table
            .offers()
            .iter()
            .any(|pending| pending.link_id() == offer.link_id() && pending.hash() == offer.hash())
        {
            return QueuePendingResourceOfferOutcome::RetryCoalesced;
        }
        match self.table.push(offer) {
            Ok(()) => QueuePendingResourceOfferOutcome::Queued,
            Err(PendingResourceOfferTableFull) => QueuePendingResourceOfferOutcome::TableFull,
        }
    }

    pub fn pop_oldest_fitting(
        &mut self,
        mut fits: impl FnMut(&PendingResourceOffer) -> bool,
    ) -> Option<PendingResourceOffer> {
        let index = self
            .table
            .offers()
            .iter()
            .enumerate()
            .filter(|(_, offer)| fits(offer))
            .min_by_key(|(_, offer)| (offer.priority(), offer.first_arrived_at()))
            .map(|(index, _)| index)?;
        Some(self.table.swap_remove(index))
    }

    pub(crate) fn remove_at(&mut self, index: usize) -> PendingResourceOffer {
        self.table.swap_remove(index)
    }

    pub fn remove_link(&mut self, link_id: &LinkId) -> usize {
        self.remove_matching(|offer| &offer.link_id() == link_id)
    }

    pub fn remove_response_for_request(&mut self, request_id: &RequestId) -> usize {
        self.remove_matching(|offer| {
            matches!(offer.correlation(), ResourceCorrelation::Response(id) if &id == request_id)
        })
    }

    fn remove_matching(&mut self, mut matches: impl FnMut(&PendingResourceOffer) -> bool) -> usize {
        let mut index = 0;
        let mut removed = 0;
        while index < self.table.offers().len() {
            if matches(&self.table.offers()[index]) {
                self.table.swap_remove(index);
                removed += 1;
            } else {
                index += 1;
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::links::resources::table::AcceptedResource;
    use crate::routing::links::resources::{ResourceCompression, ResourceCorrelation, SaltNonce};

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    fn hash(byte: u8) -> ResourceHash {
        ResourceHash::new([byte; 32])
    }

    fn offer(
        link_byte: u8,
        hash_byte: u8,
        arrived_at: u64,
        correlation: ResourceCorrelation,
        sealed_transfer_bytes: usize,
    ) -> PendingResourceOffer {
        let name_rows = [[hash_byte; MAP_HASH_LEN]; 2];
        let names = name_rows.as_flattened();
        PendingResourceOffer::try_from_accepted(
            link(link_byte),
            hash(0xF0),
            AcceptedResource {
                hash: hash(hash_byte),
                salt_nonce: SaltNonce::new([0x44; 4]),
                compression: ResourceCompression::Uncompressed,
                has_metadata: false,
                uncompressed_data_bytes: sealed_transfer_bytes as u64,
                segment_index: 1,
                total_segment_count: 1,
                sealed_transfer_bytes,
                part_count: 2,
                sdu: sealed_transfer_bytes.div_ceil(2),
                correlation,
                initial_names: names,
            },
            InstantMillis(arrived_at),
            RttMillis::new(250),
        )
        .unwrap()
    }

    #[test]
    fn owned_offer_copies_only_the_advertisement_metadata() {
        let names = [0xA5; 8];
        let pending = PendingResourceOffer::try_from_accepted(
            link(1),
            hash(2),
            AcceptedResource {
                hash: hash(3),
                salt_nonce: SaltNonce::new([4; 4]),
                compression: ResourceCompression::Bz2,
                has_metadata: true,
                uncompressed_data_bytes: 900,
                segment_index: 2,
                total_segment_count: 3,
                sealed_transfer_bytes: 464,
                part_count: 2,
                sdu: 232,
                correlation: ResourceCorrelation::Unsolicited,
                initial_names: &names,
            },
            InstantMillis(5_000),
            RttMillis::new(125),
        )
        .unwrap();

        assert_eq!(pending.link_id(), link(1));
        assert_eq!(pending.original_hash(), hash(2));
        assert_eq!(pending.first_arrived_at(), InstantMillis(5_000));
        assert_eq!(pending.frozen_link_rtt(), RttMillis::new(125));
        assert_eq!(pending.accepted().initial_names, names);
        assert_eq!(pending.accepted().compression, ResourceCompression::Bz2);
    }

    #[test]
    fn owned_offer_rejects_impossible_hashmap_shapes() {
        let accepted = |names| AcceptedResource {
            hash: hash(3),
            salt_nonce: SaltNonce::new([4; 4]),
            compression: ResourceCompression::Uncompressed,
            has_metadata: false,
            uncompressed_data_bytes: 8,
            segment_index: 1,
            total_segment_count: 1,
            sealed_transfer_bytes: 8,
            part_count: 1,
            sdu: 8,
            correlation: ResourceCorrelation::Unsolicited,
            initial_names: names,
        };
        assert_eq!(
            PendingResourceOffer::try_from_accepted(
                link(1),
                hash(2),
                accepted(&[0; INITIAL_NAMES_BYTES + 1]),
                InstantMillis(0),
                RttMillis::new(0),
            )
            .unwrap_err(),
            PendingResourceOfferError::HashmapTooLong,
        );
        assert_eq!(
            PendingResourceOffer::try_from_accepted(
                link(1),
                hash(2),
                accepted(&[0; MAP_HASH_LEN - 1]),
                InstantMillis(0),
                RttMillis::new(0),
            )
            .unwrap_err(),
            PendingResourceOfferError::HashmapRagged,
        );
        assert_eq!(
            PendingResourceOffer::try_from_accepted(
                link(1),
                hash(2),
                accepted(&[0; MAP_HASH_LEN * 2]),
                InstantMillis(0),
                RttMillis::new(0),
            )
            .unwrap_err(),
            PendingResourceOfferError::HashmapBeyondPartCount,
        );
    }

    #[test]
    fn owned_offer_rejects_impossible_transfer_shapes() {
        let accepted = |sealed_transfer_bytes, part_count| AcceptedResource {
            hash: hash(3),
            salt_nonce: SaltNonce::new([4; 4]),
            compression: ResourceCompression::Uncompressed,
            has_metadata: false,
            uncompressed_data_bytes: sealed_transfer_bytes as u64,
            segment_index: 1,
            total_segment_count: 1,
            sealed_transfer_bytes,
            part_count,
            sdu: 8,
            correlation: ResourceCorrelation::Unsolicited,
            initial_names: &[0; MAP_HASH_LEN],
        };
        assert_eq!(
            PendingResourceOffer::try_from_accepted(
                link(1),
                hash(2),
                accepted(0, 0),
                InstantMillis(0),
                RttMillis::new(0),
            )
            .unwrap_err(),
            PendingResourceOfferError::BufferShape(ResourceBufferShapeError::EmptyTransfer),
        );
        assert_eq!(
            PendingResourceOffer::try_from_accepted(
                link(1),
                hash(2),
                accepted(8, 2),
                InstantMillis(0),
                RttMillis::new(0),
            )
            .unwrap_err(),
            PendingResourceOfferError::PartCountMismatch,
        );
    }

    #[test]
    fn owned_offer_rejects_request_settings_that_do_not_apply_to_inbound_offers() {
        let request_id = RequestId([0x55; 16]);
        for correlation in [
            ResourceCorrelation::Request {
                id: request_id,
                response_timeout: crate::engine::RequestResponseTimeout::Exact(
                    crate::units::DurationMillis(1_000),
                ),
                maximum_response_bytes: crate::units::ByteLimit::Unlimited,
            },
            ResourceCorrelation::Request {
                id: request_id,
                response_timeout: crate::engine::RequestResponseTimeout::LinkDefault,
                maximum_response_bytes: crate::units::ByteLimit::Maximum(1_024),
            },
        ] {
            assert_eq!(
                PendingResourceOffer::try_from_accepted(
                    link(1),
                    hash(2),
                    AcceptedResource {
                        hash: hash(3),
                        salt_nonce: SaltNonce::new([4; 4]),
                        compression: ResourceCompression::Uncompressed,
                        has_metadata: false,
                        uncompressed_data_bytes: 8,
                        segment_index: 1,
                        total_segment_count: 1,
                        sealed_transfer_bytes: 8,
                        part_count: 1,
                        sdu: 8,
                        correlation,
                        initial_names: &[0; MAP_HASH_LEN],
                    },
                    InstantMillis(0),
                    RttMillis::new(0),
                )
                .unwrap_err(),
                PendingResourceOfferError::RequestSettingsNotApplicable,
            );
        }
    }

    #[test]
    fn retries_keep_the_first_offer_clock_and_rtt() {
        let mut pending = PendingResourceOffers::<FixedPendingResourceOfferTable<4>>::default();
        let first = offer(1, 2, 1_000, ResourceCorrelation::Unsolicited, 100);
        let retry = PendingResourceOffer::try_from_accepted(
            link(1),
            hash(0xEE),
            AcceptedResource {
                hash: hash(2),
                salt_nonce: SaltNonce::new([0x99; 4]),
                compression: ResourceCompression::Uncompressed,
                has_metadata: false,
                uncompressed_data_bytes: 200,
                segment_index: 1,
                total_segment_count: 1,
                sealed_transfer_bytes: 200,
                part_count: 1,
                sdu: 200,
                correlation: ResourceCorrelation::Unsolicited,
                initial_names: &[0; MAP_HASH_LEN],
            },
            InstantMillis(9_000),
            RttMillis::new(900),
        )
        .unwrap();

        assert_eq!(
            pending.queue(first),
            QueuePendingResourceOfferOutcome::Queued
        );
        assert_eq!(
            pending.queue(retry),
            QueuePendingResourceOfferOutcome::RetryCoalesced
        );
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.offers()[0].first_arrived_at(), InstantMillis(1_000));
        assert_eq!(pending.offers()[0].frozen_link_rtt(), RttMillis::new(250));
        assert_eq!(pending.offers()[0].wait_deadline(), InstantMillis(6_000));
        assert_eq!(pending.offers()[0].accepted().sealed_transfer_bytes, 100);
    }

    #[test]
    fn wait_budget_is_two_resource_windows_capped_at_ten_seconds() {
        assert_eq!(pending_resource_wait_ms(RttMillis::new(0)), 2_000);
        assert_eq!(pending_resource_wait_ms(RttMillis::new(250)), 5_000);
        assert_eq!(pending_resource_wait_ms(RttMillis::new(666)), 9_992);
        assert_eq!(pending_resource_wait_ms(RttMillis::new(667)), 10_000);
        assert_eq!(pending_resource_wait_ms(RttMillis::new(u64::MAX)), 10_000);
    }

    #[test]
    fn responses_then_requests_then_unsolicited_use_oldest_fitting_order() {
        let mut pending = PendingResourceOffers::<FixedPendingResourceOfferTable<8>>::default();
        let request_id = RequestId([0x55; 16]);
        for queued in [
            offer(1, 1, 100, ResourceCorrelation::Unsolicited, 100),
            offer(2, 2, 400, ResourceCorrelation::Response(request_id), 400),
            offer(
                3,
                3,
                200,
                ResourceCorrelation::Request {
                    id: request_id,
                    response_timeout: Default::default(),
                    maximum_response_bytes: Default::default(),
                },
                200,
            ),
            offer(4, 4, 300, ResourceCorrelation::Response(request_id), 300),
            offer(5, 5, 250, ResourceCorrelation::Response(request_id), 300),
        ] {
            assert_eq!(
                pending.queue(queued),
                QueuePendingResourceOfferOutcome::Queued
            );
        }

        let first = pending
            .pop_oldest_fitting(|offer| offer.accepted().sealed_transfer_bytes <= 350)
            .unwrap();
        assert_eq!(first.hash(), hash(5), "the oldest fitting response wins");
        let second = pending
            .pop_oldest_fitting(|offer| offer.accepted().sealed_transfer_bytes <= 350)
            .unwrap();
        assert_eq!(second.hash(), hash(4), "the next fitting response follows");
        let third = pending
            .pop_oldest_fitting(|offer| offer.accepted().sealed_transfer_bytes <= 350)
            .unwrap();
        assert_eq!(third.hash(), hash(3), "the peer request is next");
        let fourth = pending.pop_oldest_fitting(|_| true).unwrap();
        assert_eq!(fourth.hash(), hash(2), "a response outranks greater age");
        let fifth = pending.pop_oldest_fitting(|_| true).unwrap();
        assert_eq!(fifth.hash(), hash(1));
    }

    #[test]
    fn removals_follow_link_and_request_ownership() {
        let mut pending = PendingResourceOffers::<FixedPendingResourceOfferTable<8>>::default();
        let request_id = RequestId([0x55; 16]);
        for queued in [
            offer(1, 1, 100, ResourceCorrelation::Response(request_id), 100),
            offer(1, 2, 200, ResourceCorrelation::Unsolicited, 100),
            offer(2, 3, 300, ResourceCorrelation::Response(request_id), 100),
        ] {
            assert_eq!(
                pending.queue(queued),
                QueuePendingResourceOfferOutcome::Queued,
            );
        }

        assert_eq!(pending.remove_link(&link(1)), 2);
        assert_eq!(pending.remove_response_for_request(&request_id), 1);
        assert!(pending.is_empty());
    }

    #[test]
    fn fixed_queue_enforces_its_exact_row_limit() {
        let mut pending = PendingResourceOffers::<FixedPendingResourceOfferTable<4>>::default();
        for hash_byte in 0..4 {
            assert_eq!(
                pending.queue(offer(
                    0,
                    hash_byte,
                    hash_byte as u64,
                    ResourceCorrelation::Unsolicited,
                    100,
                )),
                QueuePendingResourceOfferOutcome::Queued,
            );
        }
        assert_eq!(pending.capacity(), 4);
        assert_eq!(pending.len(), 4);
        assert_eq!(
            pending.queue(offer(0, 0, 99, ResourceCorrelation::Unsolicited, 100,)),
            QueuePendingResourceOfferOutcome::RetryCoalesced,
        );
        assert_eq!(
            pending.queue(offer(1, 0, 5, ResourceCorrelation::Unsolicited, 100,)),
            QueuePendingResourceOfferOutcome::TableFull,
        );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn host_queue_enforces_its_256_row_limit_without_eager_allocation() {
        let table = HeapPendingResourceOfferTable::default();
        assert_eq!(table.offers.capacity(), 0);
        let mut pending = PendingResourceOffers { table };
        for hash_byte in u8::MIN..=u8::MAX {
            assert_eq!(
                pending.queue(offer(
                    0,
                    hash_byte,
                    hash_byte as u64,
                    ResourceCorrelation::Unsolicited,
                    100,
                )),
                QueuePendingResourceOfferOutcome::Queued,
            );
        }
        assert_eq!(pending.capacity(), HOST_PENDING_RESOURCE_OFFER_CAPACITY);
        assert_eq!(pending.len(), HOST_PENDING_RESOURCE_OFFER_CAPACITY);
        assert_eq!(
            pending.table.offers.capacity(),
            HOST_PENDING_RESOURCE_OFFER_CAPACITY,
        );
        assert_eq!(
            pending.queue(offer(1, 0, 257, ResourceCorrelation::Unsolicited, 100,)),
            QueuePendingResourceOfferOutcome::TableFull,
        );
    }

    #[test]
    fn pending_rows_are_bounded_advertisement_metadata() {
        assert_eq!(INITIAL_NAMES_BYTES, 296);
        assert!(core::mem::size_of::<PendingResourceOffer>() <= 480);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(core::mem::size_of::<PendingResourceOffer>(), 464);
        #[cfg(all(feature = "alloc", target_pointer_width = "64"))]
        assert_eq!(
            HOST_PENDING_RESOURCE_OFFER_CAPACITY * core::mem::size_of::<PendingResourceOffer>(),
            118_784,
        );
        println!(
            "PendingResourceOffer={} B; PendingAcceptedResource={} B; ResourceCorrelation={} B; FixedPendingResourceOfferTable<4>={} B",
            core::mem::size_of::<PendingResourceOffer>(),
            core::mem::size_of::<PendingAcceptedResource>(),
            core::mem::size_of::<ResourceCorrelation>(),
            core::mem::size_of::<FixedPendingResourceOfferTable<4>>(),
        );
    }
}
