use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::vec::Vec;

use crate::identity::IdentityHash;
use crate::lemire_index::LemireIndex;
use crate::routing::blackhole::{
    blackhole_index_buckets, BlackholeExpiry, BlackholeInsertFailure, BlackholeTable,
    BlackholedIdentity, FixedBlackholeInsertError,
};

#[derive(Debug)]
pub struct FixedHeapBlackholeTable<
    const CAPACITY: usize,
    const INDEX_BUCKETS: usize,
    const REASON_BYTES: usize,
    A: Allocator = Global,
> {
    identities: Vec<IdentityHash, A>,
    sources: Vec<IdentityHash, A>,
    expiries: Vec<BlackholeExpiry, A>,
    reasons: Vec<Option<heapless::String<REASON_BYTES>>, A>,
    index: LemireIndex<INDEX_BUCKETS>,
}

impl<
        const CAPACITY: usize,
        const INDEX_BUCKETS: usize,
        const REASON_BYTES: usize,
        A: Allocator + Default,
    > Default for FixedHeapBlackholeTable<CAPACITY, INDEX_BUCKETS, REASON_BYTES, A>
{
    fn default() -> Self {
        const {
            assert!(
                INDEX_BUCKETS >= blackhole_index_buckets(CAPACITY),
                "INDEX_BUCKETS must preserve two-thirds-load headroom over CAPACITY",
            );
            assert!(
                CAPACITY < u16::MAX as usize,
                "FixedHeapBlackholeTable indexes slots as u16",
            );
        }
        Self {
            identities: Vec::with_capacity_in(CAPACITY, A::default()),
            sources: Vec::with_capacity_in(CAPACITY, A::default()),
            expiries: Vec::with_capacity_in(CAPACITY, A::default()),
            reasons: Vec::with_capacity_in(CAPACITY, A::default()),
            index: LemireIndex::default(),
        }
    }
}

impl<
        const CAPACITY: usize,
        const INDEX_BUCKETS: usize,
        const REASON_BYTES: usize,
        A: Allocator,
    > BlackholeTable for FixedHeapBlackholeTable<CAPACITY, INDEX_BUCKETS, REASON_BYTES, A>
{
    type InsertError = FixedBlackholeInsertError;

    fn classify_insert_error(error: Self::InsertError) -> BlackholeInsertFailure {
        match error {
            FixedBlackholeInsertError::TableFull => BlackholeInsertFailure::CapacityExhausted,
            FixedBlackholeInsertError::ReasonTooLong => BlackholeInsertFailure::ReasonTooLong,
        }
    }

    fn len(&self) -> usize {
        self.identities.len()
    }

    fn index_of(&self, identity: &IdentityHash) -> Option<usize> {
        self.index.get(identity, &self.identities)
    }

    fn identities(&self) -> &[IdentityHash] {
        &self.identities
    }

    fn sources(&self) -> &[IdentityHash] {
        &self.sources
    }

    fn expiries(&self) -> &[BlackholeExpiry] {
        &self.expiries
    }

    fn reason_at(&self, index: usize) -> Option<&str> {
        self.reasons[index].as_ref().map(|reason| reason.as_str())
    }

    fn push(&mut self, entry: BlackholedIdentity<&str>) -> Result<(), Self::InsertError> {
        if self.identities.len() >= CAPACITY {
            return Err(FixedBlackholeInsertError::TableFull);
        }
        let reason = match entry.reason {
            Some(reason) => Some(
                heapless::String::try_from(reason)
                    .map_err(|_| FixedBlackholeInsertError::ReasonTooLong)?,
            ),
            None => None,
        };
        let index = self.identities.len();
        self.identities.push(entry.identity);
        self.sources.push(entry.source);
        self.expiries.push(entry.expiry);
        self.reasons.push(reason);
        self.index.insert(index, &self.identities);
        Ok(())
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.identities.len() - 1;
        self.index.remove_slot(index, &self.identities);
        if index != last {
            let moved = self.identities[last];
            self.index.repoint(&moved, index, &self.identities);
        }
        self.identities.swap_remove(index);
        self.sources.swap_remove(index);
        self.expiries.swap_remove(index);
        self.reasons.swap_remove(index);
    }
}
