use crate::identity::IdentityHash;
use crate::lemire_index::buckets_for_two_thirds_load;
use crate::units::InstantMillis;

pub const fn blackhole_index_buckets(entries: usize) -> usize {
    buckets_for_two_thirds_load(entries)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackholeExpiry {
    Indefinite,
    At(InstantMillis),
}

impl BlackholeExpiry {
    pub const fn is_expired_at(self, now: InstantMillis) -> bool {
        match self {
            Self::Indefinite => false,
            Self::At(deadline) => deadline.0 < now.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackholedIdentity<Reason> {
    pub identity: IdentityHash,
    pub source: IdentityHash,
    pub expiry: BlackholeExpiry,
    pub reason: Option<Reason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackholeIdentityOutcome {
    Added,
    AlreadyPresent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnblackholeIdentityOutcome {
    Removed,
    NotFound,
}

pub trait BlackholeTable {
    type InsertError;

    fn len(&self) -> usize;
    fn index_of(&self, identity: &IdentityHash) -> Option<usize>;
    fn identities(&self) -> &[IdentityHash];
    fn sources(&self) -> &[IdentityHash];
    fn expiries(&self) -> &[BlackholeExpiry];
    fn reason_at(&self, index: usize) -> Option<&str>;
    fn push(&mut self, entry: BlackholedIdentity<&str>) -> Result<(), Self::InsertError>;
    fn swap_remove(&mut self, index: usize);

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug)]
pub struct IdentityBlackholes<Table> {
    table: Table,
}

impl<Table: BlackholeTable + Default> Default for IdentityBlackholes<Table> {
    fn default() -> Self {
        Self {
            table: Table::default(),
        }
    }
}

impl<Table: BlackholeTable> IdentityBlackholes<Table> {
    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn is_blackholed(&self, identity: &IdentityHash) -> bool {
        self.table.index_of(identity).is_some()
    }

    pub fn entries(&self) -> impl Iterator<Item = BlackholedIdentity<&str>> + '_ {
        (0..self.table.len()).map(|index| BlackholedIdentity {
            identity: self.table.identities()[index],
            source: self.table.sources()[index],
            expiry: self.table.expiries()[index],
            reason: self.table.reason_at(index),
        })
    }

    pub fn blackhole_identity(
        &mut self,
        entry: BlackholedIdentity<&str>,
    ) -> Result<BlackholeIdentityOutcome, Table::InsertError> {
        if self.is_blackholed(&entry.identity) {
            return Ok(BlackholeIdentityOutcome::AlreadyPresent);
        }
        self.table.push(entry)?;
        Ok(BlackholeIdentityOutcome::Added)
    }

    pub fn unblackhole_identity(&mut self, identity: &IdentityHash) -> UnblackholeIdentityOutcome {
        let Some(index) = self.table.index_of(identity) else {
            return UnblackholeIdentityOutcome::NotFound;
        };
        self.table.swap_remove(index);
        UnblackholeIdentityOutcome::Removed
    }

    pub fn cull_expired(&mut self, now: InstantMillis) -> usize {
        let mut removed = 0;
        let mut index = 0;
        while index < self.table.len() {
            if self.table.expiries()[index].is_expired_at(now) {
                self.table.swap_remove(index);
                removed += 1;
            } else {
                index += 1;
            }
        }
        removed
    }
}
