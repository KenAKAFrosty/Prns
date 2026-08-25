use heapless::Vec;

use super::{PendingResourceOffer, PendingResourceOfferTable, PendingResourceOfferTableFull};

#[derive(Debug, Default)]
pub struct NoPendingResourceOfferTable;

impl PendingResourceOfferTable for NoPendingResourceOfferTable {
    fn capacity(&self) -> usize {
        0
    }

    fn offers(&self) -> &[PendingResourceOffer] {
        &[]
    }

    fn push(&mut self, _offer: PendingResourceOffer) -> Result<(), PendingResourceOfferTableFull> {
        Err(PendingResourceOfferTableFull)
    }

    fn swap_remove(&mut self, _index: usize) -> PendingResourceOffer {
        unreachable!("the zero-capacity pending Resource table has no rows")
    }
}

#[derive(Debug, Default)]
pub struct FixedPendingResourceOfferTable<const CAP: usize> {
    offers: Vec<PendingResourceOffer, CAP>,
}

impl<const CAP: usize> PendingResourceOfferTable for FixedPendingResourceOfferTable<CAP> {
    fn capacity(&self) -> usize {
        CAP
    }

    fn offers(&self) -> &[PendingResourceOffer] {
        &self.offers
    }

    fn push(&mut self, offer: PendingResourceOffer) -> Result<(), PendingResourceOfferTableFull> {
        self.offers
            .push(offer)
            .map_err(|_offer| PendingResourceOfferTableFull)
    }

    fn swap_remove(&mut self, index: usize) -> PendingResourceOffer {
        self.offers.swap_remove(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_no_queue_backend_adds_no_inline_storage() {
        assert_eq!(core::mem::size_of::<NoPendingResourceOfferTable>(), 0);
        assert_eq!(NoPendingResourceOfferTable.capacity(), 0);
    }
}
