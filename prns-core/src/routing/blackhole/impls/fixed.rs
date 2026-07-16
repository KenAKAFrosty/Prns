use crate::identity::IdentityHash;
use crate::lemire_index::LemireIndex;
use crate::routing::blackhole::{
    blackhole_index_buckets, BlackholeExpiry, BlackholeInsertFailure, BlackholeTable,
    BlackholedIdentity,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedBlackholeInsertError {
    TableFull,
    ReasonTooLong,
}

#[derive(Debug)]
pub struct FixedBlackholeTable<
    const CAPACITY: usize,
    const INDEX_BUCKETS: usize,
    const REASON_BYTES: usize,
> {
    len: usize,
    identities: [IdentityHash; CAPACITY],
    sources: [IdentityHash; CAPACITY],
    expiries: [BlackholeExpiry; CAPACITY],
    reasons: [Option<heapless::String<REASON_BYTES>>; CAPACITY],
    index: LemireIndex<INDEX_BUCKETS>,
}

impl<const CAPACITY: usize, const INDEX_BUCKETS: usize, const REASON_BYTES: usize> Default
    for FixedBlackholeTable<CAPACITY, INDEX_BUCKETS, REASON_BYTES>
{
    fn default() -> Self {
        const {
            assert!(
                INDEX_BUCKETS >= blackhole_index_buckets(CAPACITY),
                "INDEX_BUCKETS must preserve two-thirds-load headroom over CAPACITY",
            );
            assert!(
                CAPACITY < u16::MAX as usize,
                "FixedBlackholeTable indexes slots as u16",
            );
        }
        Self {
            len: 0,
            identities: [IdentityHash::new([0; 16]); CAPACITY],
            sources: [IdentityHash::new([0; 16]); CAPACITY],
            expiries: [BlackholeExpiry::Indefinite; CAPACITY],
            reasons: [const { None }; CAPACITY],
            index: LemireIndex::default(),
        }
    }
}

impl<const CAPACITY: usize, const INDEX_BUCKETS: usize, const REASON_BYTES: usize> BlackholeTable
    for FixedBlackholeTable<CAPACITY, INDEX_BUCKETS, REASON_BYTES>
{
    type InsertError = FixedBlackholeInsertError;

    fn classify_insert_error(error: Self::InsertError) -> BlackholeInsertFailure {
        match error {
            FixedBlackholeInsertError::TableFull => BlackholeInsertFailure::CapacityExhausted,
            FixedBlackholeInsertError::ReasonTooLong => BlackholeInsertFailure::ReasonTooLong,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn index_of(&self, identity: &IdentityHash) -> Option<usize> {
        self.index.get(identity, &self.identities)
    }

    fn identities(&self) -> &[IdentityHash] {
        &self.identities[..self.len]
    }

    fn sources(&self) -> &[IdentityHash] {
        &self.sources[..self.len]
    }

    fn expiries(&self) -> &[BlackholeExpiry] {
        &self.expiries[..self.len]
    }

    fn reason_at(&self, index: usize) -> Option<&str> {
        self.reasons[index].as_ref().map(|reason| reason.as_str())
    }

    fn push(&mut self, entry: BlackholedIdentity<&str>) -> Result<(), Self::InsertError> {
        if self.len >= CAPACITY {
            return Err(FixedBlackholeInsertError::TableFull);
        }
        let reason = match entry.reason {
            Some(reason) => Some(
                heapless::String::try_from(reason)
                    .map_err(|_| FixedBlackholeInsertError::ReasonTooLong)?,
            ),
            None => None,
        };
        let index = self.len;
        self.identities[index] = entry.identity;
        self.sources[index] = entry.source;
        self.expiries[index] = entry.expiry;
        self.reasons[index] = reason;
        self.len += 1;
        self.index.insert(index, &self.identities);
        Ok(())
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        self.index.remove_slot(index, &self.identities);
        if index != last {
            let moved = self.identities[last];
            self.index.repoint(&moved, index, &self.identities);
            self.identities[index] = self.identities[last];
            self.sources[index] = self.sources[last];
            self.expiries[index] = self.expiries[last];
            self.reasons[index] = self.reasons[last].take();
        } else {
            self.reasons[index] = None;
        }
        self.len = last;
    }
}
