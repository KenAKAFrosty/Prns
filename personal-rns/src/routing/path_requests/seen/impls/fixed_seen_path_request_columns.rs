use crate::routing::path_requests::seen::{PathRequestIdBytes, SeenPathRequestColumns};
use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};

/// A fixed-capacity FIFO ring: `write_cursor` overwrites the oldest tag once the
/// array is full, so the set always holds the most recent `MAX_SEEN_PATH_REQUESTS`.
#[derive(Debug)]
pub struct FixedSeenPathRequestColumns<const MAX_SEEN_PATH_REQUESTS: usize> {
    len: usize,
    write_cursor: usize,
    destinations: [DestinationHash; MAX_SEEN_PATH_REQUESTS],
    ids: [PathRequestIdBytes; MAX_SEEN_PATH_REQUESTS],
}

impl<const MAX_SEEN_PATH_REQUESTS: usize> Default
    for FixedSeenPathRequestColumns<MAX_SEEN_PATH_REQUESTS>
{
    fn default() -> Self {
        Self {
            len: 0,
            write_cursor: 0,
            destinations: [DestinationHash::new([0u8; 16]); MAX_SEEN_PATH_REQUESTS],
            ids: [[0u8; TRUNCATED_HASH_BYTE_LEN]; MAX_SEEN_PATH_REQUESTS],
        }
    }
}

impl<const MAX_SEEN_PATH_REQUESTS: usize> SeenPathRequestColumns
    for FixedSeenPathRequestColumns<MAX_SEEN_PATH_REQUESTS>
{
    fn capacity(&self) -> usize {
        MAX_SEEN_PATH_REQUESTS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations[..self.len]
    }
    fn ids(&self) -> &[PathRequestIdBytes] {
        &self.ids[..self.len]
    }

    fn remember(&mut self, destination: DestinationHash, id: PathRequestIdBytes) {
        if MAX_SEEN_PATH_REQUESTS == 0 {
            return;
        }
        let i = self.write_cursor;
        self.destinations[i] = destination;
        self.ids[i] = id;
        self.write_cursor = (i + 1) % MAX_SEEN_PATH_REQUESTS;
        self.len = (self.len + 1).min(MAX_SEEN_PATH_REQUESTS);
    }
}
