//! Growable-heap held pool: rows, links, and the per-interface chain index all grow on demand.
//! A freed slot is recycled through the free list before the pool grows, and each interface's chain
//! is capped at the parity per-interface limit, so total growth tracks the live peer count.

use alloc::vec::Vec;

use crate::routing::announce::held::{
    vacant_held_announce, HeldAnnounce, HeldAnnouncePool, HeldInterfaceChain, HeldSlot, NO_SLOT,
};

#[derive(Debug)]
pub struct HeapHeldAnnouncePool {
    rows: Vec<HeldAnnounce>,
    links: Vec<HeldSlot>,
    free_head: HeldSlot,
    chains: Vec<HeldInterfaceChain>,
}

impl Default for HeapHeldAnnouncePool {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            links: Vec::new(),
            free_head: NO_SLOT,
            chains: Vec::new(),
        }
    }
}

impl HeldAnnouncePool for HeapHeldAnnouncePool {
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
        self.chains.push(chain);
    }

    fn swap_remove_chain(&mut self, index: usize) {
        self.chains.swap_remove(index);
    }

    fn grow_one(&mut self) -> Option<HeldSlot> {
        let slot = self.rows.len() as HeldSlot;
        self.rows.push(vacant_held_announce());
        self.links.push(NO_SLOT);
        Some(slot)
    }
}
