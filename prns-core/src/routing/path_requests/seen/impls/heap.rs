use alloc::vec::Vec;

use crate::routing::path_requests::seen::{PathRequestIdBytes, SeenPathRequestTable};
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapSeenPathRequestTable {
    write_cursor: usize,
    destinations: Vec<DestinationHash>,
    ids: Vec<PathRequestIdBytes>,
}

impl HeapSeenPathRequestTable {
    /// RNS 1.4.2 `Transport.max_pr_tags`: the reference's own bound on remembered path-request tags, dropped oldest-first past it.
    pub const RNS_MAX_PR_TAGS: usize = 32_000;
}

impl SeenPathRequestTable for HeapSeenPathRequestTable {
    fn capacity(&self) -> usize {
        Self::RNS_MAX_PR_TAGS
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn ids(&self) -> &[PathRequestIdBytes] {
        &self.ids
    }

    fn remember(&mut self, destination: DestinationHash, id: PathRequestIdBytes) {
        if self.destinations.len() < Self::RNS_MAX_PR_TAGS {
            self.destinations.push(destination);
            self.ids.push(id);
            return;
        }
        let i = self.write_cursor;
        self.destinations[i] = destination;
        self.ids[i] = id;
        self.write_cursor = (i + 1) % Self::RNS_MAX_PR_TAGS;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::path_requests::seen::{PathRequestNovelty, SeenPathRequests};

    fn dest(n: u32) -> DestinationHash {
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&n.to_be_bytes());
        DestinationHash::new(bytes)
    }

    #[test]
    fn past_the_reference_bound_the_oldest_tag_is_overwritten_in_place() {
        let mut seen: SeenPathRequests<HeapSeenPathRequestTable> = SeenPathRequests::default();
        for n in 0..HeapSeenPathRequestTable::RNS_MAX_PR_TAGS as u32 {
            assert_eq!(seen.observe(dest(n), [0xAA; 16]), PathRequestNovelty::Fresh);
        }
        assert_eq!(seen.len(), HeapSeenPathRequestTable::RNS_MAX_PR_TAGS);

        let next = HeapSeenPathRequestTable::RNS_MAX_PR_TAGS as u32;
        assert_eq!(
            seen.observe(dest(next), [0xAA; 16]),
            PathRequestNovelty::Fresh
        );
        assert_eq!(
            seen.len(),
            HeapSeenPathRequestTable::RNS_MAX_PR_TAGS,
            "the table holds its bound instead of growing",
        );
        assert_eq!(
            seen.observe(dest(0), [0xAA; 16]),
            PathRequestNovelty::Fresh,
            "the oldest tag made way for the newcomer",
        );
        assert_eq!(
            seen.observe(dest(next), [0xAA; 16]),
            PathRequestNovelty::Duplicate,
            "the newcomer is still remembered",
        );
    }
}
