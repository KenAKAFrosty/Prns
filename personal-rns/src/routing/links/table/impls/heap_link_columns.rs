use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::links::table::{LinkColumns, LinkPhase, TrackLinkError};
use crate::routing::links::LinkId;

#[derive(Debug, Default)]
pub struct HeapLinkColumns {
    link_ids: Vec<LinkId>,
    timeout_ats: Vec<Option<InstantMillis>>,
    phases: Vec<LinkPhase>,
}

impl LinkColumns for HeapLinkColumns {
    fn capacity(&self) -> usize {
        usize::MAX
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
        self.link_ids.push(link_id);
        self.timeout_ats.push(timeout_at);
        self.phases.push(phase);
        Ok(self.link_ids.len() - 1)
    }

    fn swap_remove(&mut self, index: usize) {
        self.link_ids.swap_remove(index);
        self.timeout_ats.swap_remove(index);
        self.phases.swap_remove(index);
    }
}
