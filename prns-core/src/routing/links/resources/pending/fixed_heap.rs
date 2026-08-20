use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::vec::Vec;

use super::{PendingResourceOffer, PendingResourceOfferTable, PendingResourceOfferTableFull};

/// A fixed number of pending offer rows allocated in the caller-selected heap
/// region. ESP32-S3 storage places this reservation in PSRAM.
pub struct FixedHeapPendingResourceOfferTable<const CAP: usize, A: Allocator = Global> {
    offers: Vec<PendingResourceOffer, A>,
}

impl<const CAP: usize, A: Allocator> FixedHeapPendingResourceOfferTable<CAP, A> {
    pub const RESERVED_ROW_BYTES: usize = CAP * core::mem::size_of::<PendingResourceOffer>();
}

impl<const CAP: usize, A: Allocator + Default> Default
    for FixedHeapPendingResourceOfferTable<CAP, A>
{
    fn default() -> Self {
        Self {
            offers: Vec::with_capacity_in(CAP, A::default()),
        }
    }
}

impl<const CAP: usize, A: Allocator> PendingResourceOfferTable
    for FixedHeapPendingResourceOfferTable<CAP, A>
{
    fn capacity(&self) -> usize {
        CAP
    }

    fn offers(&self) -> &[PendingResourceOffer] {
        &self.offers
    }

    fn push(&mut self, offer: PendingResourceOffer) -> Result<(), PendingResourceOfferTableFull> {
        if self.offers.len() >= CAP {
            return Err(PendingResourceOfferTableFull);
        }
        self.offers.push(offer);
        Ok(())
    }

    fn swap_remove(&mut self, index: usize) -> PendingResourceOffer {
        self.offers.swap_remove(index)
    }
}

impl<const CAP: usize, A: Allocator> core::fmt::Debug
    for FixedHeapPendingResourceOfferTable<CAP, A>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FixedHeapPendingResourceOfferTable")
            .field("len", &self.offers.len())
            .field("capacity", &CAP)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_rows_reserve_only_psram_offer_metadata() {
        type Table = FixedHeapPendingResourceOfferTable<4>;
        let table = Table::default();
        assert_eq!(table.capacity(), 4);
        assert_eq!(table.offers.capacity(), 4);
        assert!(table.offers().is_empty());
        assert_eq!(
            Table::RESERVED_ROW_BYTES,
            4 * core::mem::size_of::<PendingResourceOffer>(),
        );
        assert_eq!(
            core::mem::size_of::<Table>(),
            3 * core::mem::size_of::<usize>()
        );
    }
}
