use crate::engine::InstantMillis;
use crate::routing::links::table::{LinkColumns, LinkPhase, TrackLinkError};
use crate::routing::links::LinkId;

#[derive(Debug)]
pub struct FixedLinkColumns<const MAX_LINKS: usize> {
    len: usize,
    link_ids: [LinkId; MAX_LINKS],
    timeout_ats: [Option<InstantMillis>; MAX_LINKS],
    phases: [LinkPhase; MAX_LINKS],
}

impl<const MAX_LINKS: usize> Default for FixedLinkColumns<MAX_LINKS> {
    fn default() -> Self {
        Self {
            len: 0,
            link_ids: [LinkId::new([0u8; 16]); MAX_LINKS],
            timeout_ats: [None; MAX_LINKS],
            phases: core::array::from_fn(|_| LinkPhase::vacant()),
        }
    }
}

impl<const MAX_LINKS: usize> LinkColumns for FixedLinkColumns<MAX_LINKS> {
    fn capacity(&self) -> usize {
        MAX_LINKS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn link_ids(&self) -> &[LinkId] {
        &self.link_ids[..self.len]
    }
    fn timeout_ats(&self) -> &[Option<InstantMillis>] {
        &self.timeout_ats[..self.len]
    }
    fn phases(&self) -> &[LinkPhase] {
        &self.phases[..self.len]
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
        if self.len >= MAX_LINKS {
            return Err(TrackLinkError::TableFull);
        }
        let i = self.len;
        self.link_ids[i] = link_id;
        self.timeout_ats[i] = timeout_at;
        self.phases[i] = phase;
        self.len += 1;
        Ok(i)
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        self.link_ids.swap(index, last);
        self.timeout_ats.swap(index, last);
        self.phases.swap(index, last);
        self.phases[last] = LinkPhase::vacant();
        self.len = last;
    }
}
