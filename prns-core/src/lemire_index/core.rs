pub(crate) const fn buckets_for_two_thirds_load(entries: usize) -> usize {
    if entries == 0 {
        return 1;
    }
    entries.saturating_add(entries.div_ceil(2))
}

#[cfg(any(feature = "alloc", test))]
pub(super) const fn exceeds_two_thirds_load(entries: usize, buckets: usize) -> bool {
    entries > buckets.saturating_sub(buckets.div_ceil(3))
}

pub trait IndexKey: Copy + Eq {
    fn lemire_key(&self) -> u64;
}

pub trait IndexRow {
    type Key: IndexKey;

    fn index_key(&self) -> &Self::Key;
}

impl<K: IndexKey> IndexRow for K {
    type Key = K;

    fn index_key(&self) -> &Self::Key {
        self
    }
}

pub(super) fn lemire_key_from_prefix(bytes: &[u8]) -> u64 {
    let b = bytes;
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}
