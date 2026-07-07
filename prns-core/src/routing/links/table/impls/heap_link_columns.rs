use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::links::table::{LinkColumns, LinkPhase, TrackLinkError};
use crate::routing::links::LinkId;

const EMPTY: usize = usize::MAX;
const MIN_BUCKETS: usize = 8;

#[derive(Debug)]
pub struct HeapLinkColumns {
    link_ids: Vec<LinkId>,
    timeout_ats: Vec<Option<InstantMillis>>,
    phases: Vec<LinkPhase>,
    index: Vec<usize>,
}

impl Default for HeapLinkColumns {
    fn default() -> Self {
        let mut index = Vec::new();
        index.resize(MIN_BUCKETS, EMPTY);
        Self {
            link_ids: Vec::new(),
            timeout_ats: Vec::new(),
            phases: Vec::new(),
            index,
        }
    }
}

impl HeapLinkColumns {
    fn key(link_id: &LinkId) -> u64 {
        let b = link_id.as_bytes();
        u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
    }

    fn bucket(&self, key: u64) -> usize {
        ((key as u128 * self.index.len() as u128) >> u64::BITS) as usize
    }

    fn index_position(&self, link_id: &LinkId) -> Option<usize> {
        let n = self.index.len();
        let mut pos = self.bucket(Self::key(link_id));
        loop {
            let slot = self.index[pos];
            if slot == EMPTY {
                return None;
            }
            if self.link_ids[slot] == *link_id {
                return Some(pos);
            }
            pos = (pos + 1) % n;
        }
    }

    fn index_insert(&mut self, slot: usize) {
        let n = self.index.len();
        let mut pos = self.bucket(Self::key(&self.link_ids[slot]));
        while self.index[pos] != EMPTY {
            pos = (pos + 1) % n;
        }
        self.index[pos] = slot;
    }

    fn index_delete(&mut self, link_id: &LinkId) {
        let Some(mut hole) = self.index_position(link_id) else {
            return;
        };
        let n = self.index.len();
        loop {
            self.index[hole] = EMPTY;
            let mut scan = hole;
            loop {
                scan = (scan + 1) % n;
                let slot = self.index[scan];
                if slot == EMPTY {
                    return;
                }
                let home = self.bucket(Self::key(&self.link_ids[slot]));
                let blocks_move = if hole <= scan {
                    home > hole && home <= scan
                } else {
                    home > hole || home <= scan
                };
                if !blocks_move {
                    self.index[hole] = slot;
                    hole = scan;
                    break;
                }
            }
        }
    }

    fn index_repoint(&mut self, link_id: &LinkId, slot: usize) {
        if let Some(pos) = self.index_position(link_id) {
            self.index[pos] = slot;
        }
    }

    fn grow_index_if_loaded(&mut self) {
        if (self.link_ids.len() + 1) * 3 > self.index.len() * 2 {
            let new_buckets = self.index.len() * 2;
            self.index.clear();
            self.index.resize(new_buckets, EMPTY);
            for slot in 0..self.link_ids.len() {
                self.index_insert(slot);
            }
        }
    }
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

    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.index_position(link_id).map(|pos| self.index[pos])
    }

    fn push(
        &mut self,
        link_id: LinkId,
        phase: LinkPhase,
        timeout_at: Option<InstantMillis>,
    ) -> Result<usize, TrackLinkError> {
        self.grow_index_if_loaded();
        let slot = self.link_ids.len();
        self.link_ids.push(link_id);
        self.timeout_ats.push(timeout_at);
        self.phases.push(phase);
        self.index_insert(slot);
        Ok(slot)
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.link_ids.len() {
            return;
        }
        let last = self.link_ids.len() - 1;
        let removed = self.link_ids[index];
        self.index_delete(&removed);
        if index != last {
            let moved = self.link_ids[last];
            self.index_repoint(&moved, index);
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
        let mut columns = HeapLinkColumns::default();
        for n in 0..300u16 {
            columns.push(link(n), LinkPhase::vacant(), None).unwrap();
        }
        for n in 0..300u16 {
            let index = columns.index_of(&link(n)).expect("every link is present");
            assert_eq!(
                columns.link_ids()[index],
                link(n),
                "index points at its own slot"
            );
        }

        for n in (0..300u16).step_by(3) {
            let index = columns
                .index_of(&link(n))
                .expect("still present before removal");
            columns.swap_remove(index);
        }
        assert_eq!(columns.len(), 200);

        for n in 0..300u16 {
            match columns.index_of(&link(n)) {
                Some(index) => {
                    assert_ne!(n % 3, 0, "a removed link must not be found");
                    assert_eq!(
                        columns.link_ids()[index],
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
        let mut columns = HeapLinkColumns::default();
        columns.push(link(1), LinkPhase::vacant(), None).unwrap();
        columns.push(link(2), LinkPhase::vacant(), None).unwrap();
        let index = columns.index_of(&link(1)).unwrap();
        columns.swap_remove(index);
        assert_eq!(columns.index_of(&link(1)), None);
        columns.push(link(1), LinkPhase::vacant(), None).unwrap();
        let index = columns.index_of(&link(1)).expect("found after re-insert");
        assert_eq!(columns.link_ids()[index], link(1));
        assert!(columns.index_of(&link(2)).is_some());
    }
}
