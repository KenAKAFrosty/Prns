//! Attachment-scoped integrity evidence. No persisted schema or domain authority.
use super::{db_error, metadata::State, RedbError, RedbStoreOwner, StoreNamespace};
use redb::{Database, ReadableTable};
use std::{cell::RefCell, num::NonZeroUsize, sync::Arc};

/// Full-validation row work and cache reuse, not ordinary key reads, timing or
/// a memory guarantee. Counters saturate and have no effect on validation,
/// admission or persisted accounting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValidationStats {
    pub full_validations: u64,
    pub cache_hits: u64,
    pub values_inspected: u64,
    pub receipts_inspected: u64,
    pub payload_bytes_inspected: u64,
    pub key_comparisons: u64,
    pub index_entries_shifted: u64,
    pub evictions: u64,
}

/// One exclusive attachment's bounded validation evidence. Retaining the actual
/// database allocation prevents pointer reuse or substitution by a copied file.
/// Drop this together with the attachment before reporting physical store close.
///
/// It is deliberately not Clone. Every owner exclusively borrows this cache;
/// unrelated application tables may still use the database. Reserved namespace
/// tables must only be modified through that owner. A hit checks exact metadata
/// and table counts, not out-of-contract edits preserving both. Use `inspect`
/// for a full integrity scan, or create a fresh cache on a new attachment.
pub struct ValidationCache {
    database: Arc<Database>,
    entries: RefCell<Entries>,
}

impl ValidationCache {
    pub fn new(database: Arc<Database>, capacity: NonZeroUsize) -> Result<Self, RedbError> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(capacity.get())
            .map_err(|_| RedbError::Allocation)?;
        Ok(Self {
            database,
            entries: RefCell::new(Entries {
                rows,
                capacity: capacity.get(),
                next: 0,
                work: ValidationStats::default(),
            }),
        })
    }

    /// Exact allocation identity, kept alive by this cache. A file with identical
    /// bytes or incarnation is not interchangeable with this attachment.
    pub fn matches_database(&self, database: &Database) -> bool {
        std::ptr::eq(self.database.as_ref(), database)
    }

    pub fn owner(&mut self) -> RedbStoreOwner<'_> {
        RedbStoreOwner {
            database: self.database.as_ref(),
            change_witness: None,
            change_stamp: None,
            validation: Some(&mut self.entries),
        }
    }

    pub fn stats(&self) -> ValidationStats {
        self.entries.borrow().work
    }
}

pub(super) struct Entries {
    // Sorted fixed-capacity index, no row payloads. Routine hits/invalidation
    // use binary search. Cold insertion/eviction may shift this bounded index.
    rows: Vec<(StoreNamespace, Option<State>)>,
    capacity: usize,
    next: usize,
    work: ValidationStats,
}
impl Entries {
    fn locate(&mut self, namespace: StoreNamespace) -> Result<usize, usize> {
        let mut comparisons = 0_u64;
        let result = self.rows.binary_search_by(|(key, _)| {
            comparisons += 1;
            key.0.cmp(&namespace.0)
        });
        self.work.key_comparisons = self.work.key_comparisons.saturating_add(comparisons);
        result
    }
    fn invalidate(&mut self, namespace: StoreNamespace) {
        if let Ok(index) = self.locate(namespace) {
            self.rows[index].1 = None;
        }
    }
    fn remember(&mut self, namespace: StoreNamespace, state: State) {
        match self.locate(namespace) {
            Ok(index) => self.rows[index].1 = Some(state),
            Err(mut index) => {
                if self.rows.len() == self.capacity {
                    let victim = self.next;
                    self.rows.remove(victim);
                    self.work.index_entries_shifted = self
                        .work
                        .index_entries_shifted
                        .saturating_add((self.rows.len() - victim) as u64);
                    self.work.evictions = self.work.evictions.saturating_add(1);
                    self.next = (self.next + 1) % self.capacity;
                    if index > victim {
                        index -= 1;
                    }
                }
                self.work.index_entries_shifted = self
                    .work
                    .index_entries_shifted
                    .saturating_add((self.rows.len() - index) as u64);
                self.rows.insert(index, (namespace, Some(state)));
            }
        }
    }
}

pub(super) fn invalidate(cache: Option<&RefCell<Entries>>, namespace: StoreNamespace) {
    if let Some(cache) = cache {
        cache.borrow_mut().invalidate(namespace);
    }
}

pub(super) fn remember(cache: Option<&RefCell<Entries>>, namespace: StoreNamespace, state: State) {
    if let Some(cache) = cache {
        cache.borrow_mut().remember(namespace, state);
    }
}

pub(super) fn validate(
    cache: Option<&RefCell<Entries>>,
    namespace: StoreNamespace,
    state: &State,
    values: &impl ReadableTable<&'static [u8], &'static [u8]>,
    receipts: &impl ReadableTable<u64, u64>,
) -> Result<(), RedbError> {
    let Some(cache) = cache else {
        return state.validate(values, receipts);
    };
    let mut cache = cache.borrow_mut();
    if let Err(error) = state.validate_counts(values, receipts) {
        cache.invalidate(namespace);
        return Err(error);
    }
    if cache
        .locate(namespace)
        .is_ok_and(|index| cache.rows[index].1 == Some(*state))
    {
        cache.work.cache_hits = cache.work.cache_hits.saturating_add(1);
        return Ok(());
    }
    cache.invalidate(namespace);
    state.validate_with_work(values, receipts, &mut cache.work)?;
    cache.remember(namespace, *state);
    Ok(())
}

/// Forced full inspection; a domain callback can reject rows even when a
/// physical metadata/count proof was cached. Never remember a partial pass.
pub(super) fn inspect_values(
    cache: Option<&RefCell<Entries>>,
    namespace: StoreNamespace,
    state: &State,
    values: &impl ReadableTable<&'static [u8], &'static [u8]>,
    receipts: &impl ReadableTable<u64, u64>,
    inspect: &mut impl FnMut(&[u8], &[u8]) -> Result<(), RedbError>,
) -> Result<(), RedbError> {
    let Some(cache) = cache else {
        return state.validate_with_inspection(
            values,
            receipts,
            &mut super::ValidationStats::default(),
            inspect,
        );
    };
    let mut cache = cache.borrow_mut();
    cache.invalidate(namespace);
    state.validate_with_inspection(values, receipts, &mut cache.work, inspect)?;
    cache.remember(namespace, *state);
    Ok(())
}

pub(super) fn receipt(
    cache: Option<&RefCell<Entries>>,
    namespace: StoreNamespace,
    receipts: &impl ReadableTable<u64, u64>,
    sequence: u64,
) -> Result<(), RedbError> {
    let result = receipts.get(sequence).map_err(db_error);
    match result {
        Ok(Some(value)) if value.value() == sequence => Ok(()),
        other => {
            invalidate(cache, namespace);
            match other {
                Err(error) => Err(error),
                _ => Err(RedbError::Corrupt),
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
