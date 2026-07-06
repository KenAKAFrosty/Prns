use crate::routing::announce::held::{
    vacant_held_announce, HeldAnnounce, HeldAnnouncePool, HeldInterfaceChain, HeldSlot, NO_SLOT,
};

pub struct FixedHeldAnnouncePool<const MAX_HELD: usize> {
    rows: [HeldAnnounce; MAX_HELD],
    links: [HeldSlot; MAX_HELD],
    free_head: HeldSlot,
    chains: heapless::Vec<HeldInterfaceChain, MAX_HELD>,
}

impl<const MAX_HELD: usize> Default for FixedHeldAnnouncePool<MAX_HELD> {
    fn default() -> Self {
        let mut links = [NO_SLOT; MAX_HELD];
        for (i, link) in links.iter_mut().enumerate() {
            if i + 1 < MAX_HELD {
                *link = (i + 1) as HeldSlot;
            }
        }
        Self {
            rows: [vacant_held_announce(); MAX_HELD],
            links,
            free_head: if MAX_HELD == 0 { NO_SLOT } else { 0 },
            chains: heapless::Vec::new(),
        }
    }
}

impl<const MAX_HELD: usize> HeldAnnouncePool for FixedHeldAnnouncePool<MAX_HELD> {
    fn rows(&self) -> &[HeldAnnounce] {
        &self.rows
    }

    fn rows_mut(&mut self) -> &mut [HeldAnnounce] {
        &mut self.rows
    }

    fn links(&self) -> &[HeldSlot] {
        &self.links
    }

    fn links_mut(&mut self) -> &mut [HeldSlot] {
        &mut self.links
    }

    fn free_head(&self) -> HeldSlot {
        self.free_head
    }

    fn set_free_head(&mut self, slot: HeldSlot) {
        self.free_head = slot;
    }

    fn chains(&self) -> &[HeldInterfaceChain] {
        &self.chains
    }

    fn chains_mut(&mut self) -> &mut [HeldInterfaceChain] {
        &mut self.chains
    }

    fn push_chain(&mut self, chain: HeldInterfaceChain) {
        let _ = self.chains.push(chain);
    }

    fn swap_remove_chain(&mut self, index: usize) {
        self.chains.swap_remove(index);
    }

    fn grow_one(&mut self) -> Option<HeldSlot> {
        None
    }
}
