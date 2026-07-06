//! The fixed-capacity, heap-backed twin of [`FixedHeldAnnouncePool`]: the pool's rows, intrusive
//! links, and per-interface chain index live in a caller-chosen heap region (PSRAM on the S3) via `A`.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::routing::announce::held::{
    vacant_held_announce, HeldAnnounce, HeldAnnouncePool, HeldInterfaceChain, HeldSlot, NO_SLOT,
};

pub struct FixedHeapHeldAnnouncePool<const MAX_HELD: usize, A: Allocator = Global> {
    rows: Box<[HeldAnnounce], A>,
    links: Box<[HeldSlot], A>,
    free_head: HeldSlot,
    chains: Vec<HeldInterfaceChain, A>,
}

impl<const MAX_HELD: usize, A: Allocator + Default> Default
    for FixedHeapHeldAnnouncePool<MAX_HELD, A>
{
    fn default() -> Self {
        let mut rows = Vec::with_capacity_in(MAX_HELD, A::default());
        rows.resize(MAX_HELD, vacant_held_announce());

        let mut links = Vec::with_capacity_in(MAX_HELD, A::default());
        for i in 0..MAX_HELD {
            links.push(if i + 1 < MAX_HELD {
                (i + 1) as HeldSlot
            } else {
                NO_SLOT
            });
        }

        Self {
            rows: rows.into_boxed_slice(),
            links: links.into_boxed_slice(),
            free_head: if MAX_HELD == 0 { NO_SLOT } else { 0 },
            chains: Vec::with_capacity_in(MAX_HELD, A::default()),
        }
    }
}

impl<const MAX_HELD: usize, A: Allocator> HeldAnnouncePool
    for FixedHeapHeldAnnouncePool<MAX_HELD, A>
{
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
        None
    }
}
