use alloc::string::String;
use alloc::vec::Vec;
use core::convert::Infallible;

use crate::identity::IdentityHash;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::blackhole::{BlackholeExpiry, BlackholeTable, BlackholedIdentity};

#[derive(Debug, Default)]
pub struct HeapBlackholeTable {
    identities: Vec<IdentityHash>,
    sources: Vec<IdentityHash>,
    expiries: Vec<BlackholeExpiry>,
    reasons: Vec<Option<String>>,
    index: HeapLemireIndex,
}

impl BlackholeTable for HeapBlackholeTable {
    type InsertError = Infallible;

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
        self.reasons[index].as_deref()
    }

    fn push(&mut self, entry: BlackholedIdentity<&str>) -> Result<(), Self::InsertError> {
        let row = self.identities.len();
        self.identities.push(entry.identity);
        self.sources.push(entry.source);
        self.expiries.push(entry.expiry);
        self.reasons.push(entry.reason.map(String::from));
        self.index.insert(row, &self.identities);
        Ok(())
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.identities.len() {
            return;
        }
        let last = self.identities.len() - 1;
        self.index.remove_slot(index, &self.identities);
        if index != last {
            self.index.repoint_slot(last, index, &self.identities);
        }
        self.identities.swap_remove(index);
        self.sources.swap_remove(index);
        self.expiries.swap_remove(index);
        self.reasons.swap_remove(index);
    }
}
