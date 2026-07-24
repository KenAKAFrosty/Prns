use alloc::vec::Vec;
use core::future::Future;

use crate::{LinkId, ResourceHash};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceStreamId(u64);

impl ResourceStreamId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceAvailable {
    pub stream_id: ResourceStreamId,
    pub link_id: LinkId,
    pub hash: ResourceHash,
    pub metadata: Option<Vec<u8>>,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceChunk {
    Data(Vec<u8>),
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceReadError {
    InvalidMaximumBytes,
    StreamClosed,
    BackendFailed(alloc::string::String),
}

pub trait ResourceReader {
    fn read_chunk(
        &mut self,
        maximum_bytes: usize,
    ) -> impl Future<Output = Result<ResourceChunk, ResourceReadError>>;
}
