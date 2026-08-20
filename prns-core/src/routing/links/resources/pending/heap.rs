use alloc::vec::Vec;

use super::{PendingResourceOffer, PendingResourceOfferTable, PendingResourceOfferTableFull};

pub const HOST_PENDING_RESOURCE_OFFER_CAPACITY: usize = 256;

#[derive(Debug, Default)]
pub struct HeapPendingResourceOfferTable {
    pub(super) offers: Vec<PendingResourceOffer>,
}

impl PendingResourceOfferTable for HeapPendingResourceOfferTable {
    fn capacity(&self) -> usize {
        HOST_PENDING_RESOURCE_OFFER_CAPACITY
    }

    fn offers(&self) -> &[PendingResourceOffer] {
        &self.offers
    }

    fn push(&mut self, offer: PendingResourceOffer) -> Result<(), PendingResourceOfferTableFull> {
        if self.offers.len() >= HOST_PENDING_RESOURCE_OFFER_CAPACITY {
            return Err(PendingResourceOfferTableFull);
        }
        self.offers.push(offer);
        Ok(())
    }

    fn swap_remove(&mut self, index: usize) -> PendingResourceOffer {
        self.offers.swap_remove(index)
    }
}
