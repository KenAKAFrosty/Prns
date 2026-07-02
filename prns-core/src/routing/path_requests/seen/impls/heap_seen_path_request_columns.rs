use alloc::vec::Vec;

use crate::routing::path_requests::seen::{PathRequestIdBytes, SeenPathRequestColumns};
use crate::wire::DestinationHash;

/// A daemon-grade ceiling on remembered path-request ids — far above any real
/// in-flight discovery count, a backstop against unbounded growth, matching the
/// other engine tables' hygiene.
pub const DEFAULT_MAX_SEEN_PATH_REQUESTS: usize = 1024;

#[derive(Debug, Default)]
pub struct HeapSeenPathRequestColumns {
    destinations: Vec<DestinationHash>,
    ids: Vec<PathRequestIdBytes>,
}

impl SeenPathRequestColumns for HeapSeenPathRequestColumns {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_SEEN_PATH_REQUESTS
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
        if self.destinations.len() >= DEFAULT_MAX_SEEN_PATH_REQUESTS {
            self.destinations.remove(0);
            self.ids.remove(0);
        }
        self.destinations.push(destination);
        self.ids.push(id);
    }
}
