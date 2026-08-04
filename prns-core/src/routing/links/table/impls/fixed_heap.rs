//! Fixed-capacity link state whose columns live in a caller-selected heap.
//!
//! The table keeps its small vector descriptors inline while reserving every link row directly in
//! `A`. On ESP32-S3 that allocator is PSRAM, so increasing retained link capacity does not consume
//! the internal-RAM construction stack.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::links::table::{LinkPhase, LinkTable, TrackLinkError};
use crate::routing::links::LinkId;

pub struct FixedHeapLinkTable<const MAX_LINKS: usize, A: Allocator = Global> {
    link_ids: Vec<LinkId, A>,
    timeout_ats: Vec<Option<InstantMillis>, A>,
    phases: Vec<LinkPhase, A>,
}

impl<const MAX_LINKS: usize, A: Allocator + Default> Default for FixedHeapLinkTable<MAX_LINKS, A> {
    fn default() -> Self {
        Self {
            link_ids: Vec::with_capacity_in(MAX_LINKS, A::default()),
            timeout_ats: Vec::with_capacity_in(MAX_LINKS, A::default()),
            phases: Vec::with_capacity_in(MAX_LINKS, A::default()),
        }
    }
}

impl<const MAX_LINKS: usize, A: Allocator> LinkTable for FixedHeapLinkTable<MAX_LINKS, A> {
    fn capacity(&self) -> usize {
        MAX_LINKS
    }

    fn len(&self) -> usize {
        self.link_ids.len()
    }

    fn link_ids(&self) -> &[LinkId] {
        &self.link_ids
    }

    fn timeout_ats(&self) -> &[Option<InstantMillis>] {
        &self.timeout_ats
    }

    fn phases(&self) -> &[LinkPhase] {
        &self.phases
    }

    fn phase_mut(&mut self, index: usize) -> &mut LinkPhase {
        &mut self.phases[index]
    }

    fn set_timeout_at(&mut self, index: usize, timeout_at: Option<InstantMillis>) {
        self.timeout_ats[index] = timeout_at;
    }

    fn push(
        &mut self,
        link_id: LinkId,
        phase: LinkPhase,
        timeout_at: Option<InstantMillis>,
    ) -> Result<usize, TrackLinkError> {
        if self.len() >= MAX_LINKS {
            return Err(TrackLinkError::TableFull);
        }
        let index = self.len();
        self.link_ids.push(link_id);
        self.timeout_ats.push(timeout_at);
        self.phases.push(phase);
        Ok(index)
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.len() {
            return;
        }
        self.link_ids.swap_remove(index);
        self.timeout_ats.swap_remove(index);
        self.phases.swap_remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    #[test]
    fn capacity_is_bounded_and_rows_stay_aligned_through_removal() {
        let mut table = FixedHeapLinkTable::<2>::default();
        assert_eq!(table.capacity(), 2);
        assert_eq!(table.push(link(1), LinkPhase::vacant(), None), Ok(0));
        assert_eq!(
            table.push(link(2), LinkPhase::vacant(), Some(InstantMillis(200))),
            Ok(1)
        );
        assert_eq!(
            table.push(link(3), LinkPhase::vacant(), None),
            Err(TrackLinkError::TableFull)
        );

        table.swap_remove(0);
        assert_eq!(table.link_ids(), &[link(2)]);
        assert_eq!(table.timeout_ats(), &[Some(InstantMillis(200))]);
        assert_eq!(table.phases().len(), 1);
    }
}
