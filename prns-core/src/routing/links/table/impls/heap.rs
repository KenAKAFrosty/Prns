use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::links::table::{LinkPhase, LinkTable, TrackLinkError};
use crate::routing::links::LinkId;

#[derive(Debug, Default)]
pub struct HeapLinkTable {
    link_ids: Vec<LinkId>,
    timeout_ats: Vec<Option<InstantMillis>>,
    phases: Vec<LinkPhase>,
    index: HeapLemireIndex,
}

impl LinkTable for HeapLinkTable {
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

    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.index.get(link_id, &self.link_ids)
    }

    fn push(
        &mut self,
        link_id: LinkId,
        phase: LinkPhase,
        timeout_at: Option<InstantMillis>,
    ) -> Result<usize, TrackLinkError> {
        let slot = self.link_ids.len();
        self.link_ids.push(link_id);
        self.timeout_ats.push(timeout_at);
        self.phases.push(phase);
        self.index.insert(slot, &self.link_ids);
        Ok(slot)
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.link_ids.len() {
            return;
        }
        let last = self.link_ids.len() - 1;
        let removed = self.link_ids[index];
        self.index.remove(&removed, &self.link_ids);
        if index != last {
            let moved = self.link_ids[last];
            self.index.repoint(&moved, index, &self.link_ids);
        }
        self.link_ids.swap_remove(index);
        self.timeout_ats.swap_remove(index);
        self.phases.swap_remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(n: u16) -> LinkId {
        let mut bytes = [0u8; 16];
        bytes[0] = (n.wrapping_mul(2654) >> 5) as u8;
        bytes[1] = n as u8;
        bytes[4] = (n >> 3) as u8;
        bytes[7] = (n.wrapping_mul(97)) as u8;
        bytes[15] = (n >> 8) as u8;
        LinkId::new(bytes)
    }

    #[test]
    fn the_index_tracks_every_link_across_rehashes_and_swap_removes() {
        let mut table = HeapLinkTable::default();
        for n in 0..300u16 {
            table.push(link(n), LinkPhase::vacant(), None).unwrap();
        }
        for n in 0..300u16 {
            let index = table.index_of(&link(n)).expect("every link is present");
            assert_eq!(
                table.link_ids()[index],
                link(n),
                "index points at its own slot"
            );
        }

        for n in (0..300u16).step_by(3) {
            let index = table
                .index_of(&link(n))
                .expect("still present before removal");
            table.swap_remove(index);
        }
        assert_eq!(table.len(), 200);

        for n in 0..300u16 {
            match table.index_of(&link(n)) {
                Some(index) => {
                    assert_ne!(n % 3, 0, "a removed link must not be found");
                    assert_eq!(
                        table.link_ids()[index],
                        link(n),
                        "the moved tail stays pinned to the right slot",
                    );
                }
                None => assert_eq!(n % 3, 0, "only the removed links are absent"),
            }
        }
    }

    #[test]
    fn a_reinserted_link_after_removal_is_found_again() {
        let mut table = HeapLinkTable::default();
        table.push(link(1), LinkPhase::vacant(), None).unwrap();
        table.push(link(2), LinkPhase::vacant(), None).unwrap();
        let index = table.index_of(&link(1)).unwrap();
        table.swap_remove(index);
        assert_eq!(table.index_of(&link(1)), None);
        table.push(link(1), LinkPhase::vacant(), None).unwrap();
        let index = table.index_of(&link(1)).expect("found after re-insert");
        assert_eq!(table.link_ids()[index], link(1));
        assert!(table.index_of(&link(2)).is_some());
    }
}
