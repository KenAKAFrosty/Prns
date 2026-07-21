use heapless::Vec;

use super::{discovery_validation_index_buckets, DiscoveryValidationCache};
use crate::interface_discovery::StampValue;
use crate::lemire_index::{IndexRow, LemireIndex};

#[derive(Debug)]
struct FixedValidEntry<const MAX_PACKED_BYTES: usize> {
    payload_hash: [u8; 32],
    packed_advertisement: Vec<u8, MAX_PACKED_BYTES>,
    stamp_value: StampValue,
}

impl<const MAX_PACKED_BYTES: usize> IndexRow for FixedValidEntry<MAX_PACKED_BYTES> {
    type Key = [u8; 32];

    fn index_key(&self) -> &Self::Key {
        &self.payload_hash
    }
}

#[derive(Debug)]
struct FixedInsufficientEntry {
    payload_hash: [u8; 32],
    stamp_value: StampValue,
}

impl IndexRow for FixedInsufficientEntry {
    type Key = [u8; 32];

    fn index_key(&self) -> &Self::Key {
        &self.payload_hash
    }
}

/// Inline, allocation-free discovery validation caches for embedded and custom storage profiles.
///
/// Each cache is independently FIFO-bounded and uses stable ring slots plus a Lemire side index.
/// A valid advertisement larger than `MAX_PACKED_BYTES` is simply not memoized; it still passes
/// through normal validation. The bucket counts must provide the headroom returned by
/// [`discovery_validation_index_buckets`].
/// `FixedDiscoveryValidationCache<0, 0, 0, 1, 1>` disables memoization without disabling
/// discovery.
#[derive(Debug)]
pub struct FixedDiscoveryValidationCache<
    const VALID: usize,
    const INSUFFICIENT: usize,
    const MAX_PACKED_BYTES: usize,
    const VALID_BUCKETS: usize,
    const INSUFFICIENT_BUCKETS: usize,
> {
    valid: Vec<FixedValidEntry<MAX_PACKED_BYTES>, VALID>,
    valid_index: LemireIndex<VALID_BUCKETS>,
    valid_next_evict: usize,
    insufficient: Vec<FixedInsufficientEntry, INSUFFICIENT>,
    insufficient_index: LemireIndex<INSUFFICIENT_BUCKETS>,
    insufficient_next_evict: usize,
}

impl<
        const VALID: usize,
        const INSUFFICIENT: usize,
        const MAX_PACKED_BYTES: usize,
        const VALID_BUCKETS: usize,
        const INSUFFICIENT_BUCKETS: usize,
    > Default
    for FixedDiscoveryValidationCache<
        VALID,
        INSUFFICIENT,
        MAX_PACKED_BYTES,
        VALID_BUCKETS,
        INSUFFICIENT_BUCKETS,
    >
{
    fn default() -> Self {
        assert!(VALID <= u16::MAX as usize);
        assert!(INSUFFICIENT <= u16::MAX as usize);
        assert!(VALID_BUCKETS >= discovery_validation_index_buckets(VALID));
        assert!(INSUFFICIENT_BUCKETS >= discovery_validation_index_buckets(INSUFFICIENT));
        Self {
            valid: Vec::new(),
            valid_index: LemireIndex::new(),
            valid_next_evict: 0,
            insufficient: Vec::new(),
            insufficient_index: LemireIndex::new(),
            insufficient_next_evict: 0,
        }
    }
}

impl<
        const VALID: usize,
        const INSUFFICIENT: usize,
        const MAX_PACKED_BYTES: usize,
        const VALID_BUCKETS: usize,
        const INSUFFICIENT_BUCKETS: usize,
    > DiscoveryValidationCache
    for FixedDiscoveryValidationCache<
        VALID,
        INSUFFICIENT,
        MAX_PACKED_BYTES,
        VALID_BUCKETS,
        INSUFFICIENT_BUCKETS,
    >
{
    fn valid(&self, payload_hash: &[u8; 32]) -> Option<(&[u8], StampValue)> {
        let slot = self.valid_index.get(payload_hash, self.valid.as_slice())?;
        let entry = &self.valid[slot];
        Some((entry.packed_advertisement.as_slice(), entry.stamp_value))
    }

    fn insufficient(&self, payload_hash: &[u8; 32]) -> Option<StampValue> {
        let slot = self
            .insufficient_index
            .get(payload_hash, self.insufficient.as_slice())?;
        Some(self.insufficient[slot].stamp_value)
    }

    fn remember_valid(
        &mut self,
        payload_hash: [u8; 32],
        packed_advertisement: &[u8],
        stamp_value: StampValue,
    ) {
        if VALID == 0
            || self
                .valid_index
                .contains(&payload_hash, self.valid.as_slice())
        {
            return;
        }
        let mut packed = Vec::new();
        if packed.extend_from_slice(packed_advertisement).is_err() {
            return;
        }
        let entry = FixedValidEntry {
            payload_hash,
            packed_advertisement: packed,
            stamp_value,
        };
        if self.valid.len() < VALID {
            let slot = self.valid.len();
            let _ = self.valid.push(entry);
            self.valid_index.insert(slot, self.valid.as_slice());
        } else {
            let slot = self.valid_next_evict;
            self.valid_index.remove_slot(slot, self.valid.as_slice());
            self.valid[slot] = entry;
            self.valid_index.insert(slot, self.valid.as_slice());
            self.valid_next_evict = (slot + 1) % VALID;
        }
    }

    fn remember_insufficient(&mut self, payload_hash: [u8; 32], stamp_value: StampValue) {
        if INSUFFICIENT == 0
            || self
                .insufficient_index
                .contains(&payload_hash, self.insufficient.as_slice())
        {
            return;
        }
        let entry = FixedInsufficientEntry {
            payload_hash,
            stamp_value,
        };
        if self.insufficient.len() < INSUFFICIENT {
            let slot = self.insufficient.len();
            let _ = self.insufficient.push(entry);
            self.insufficient_index
                .insert(slot, self.insufficient.as_slice());
        } else {
            let slot = self.insufficient_next_evict;
            self.insufficient_index
                .remove_slot(slot, self.insufficient.as_slice());
            self.insufficient[slot] = entry;
            self.insufficient_index
                .insert(slot, self.insufficient.as_slice());
            self.insufficient_next_evict = (slot + 1) % INSUFFICIENT;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_cache_bounds_both_fifos_and_tolerates_oversize_values() {
        let mut cache = FixedDiscoveryValidationCache::<
            2,
            2,
            3,
            { discovery_validation_index_buckets(2) },
            { discovery_validation_index_buckets(2) },
        >::default();
        let stamp = StampValue::new(16).unwrap();
        for byte in 1..=3 {
            cache.remember_valid([byte; 32], &[byte; 3], stamp);
            cache.remember_insufficient([byte; 32], stamp);
        }
        assert!(cache.valid(&[1; 32]).is_none());
        assert!(cache.insufficient(&[1; 32]).is_none());
        assert_eq!(cache.valid(&[3; 32]), Some((&[3; 3][..], stamp)));

        cache.remember_valid([4; 32], &[4; 4], stamp);
        assert!(cache.valid(&[4; 32]).is_none());
        assert!(
            cache.valid(&[2; 32]).is_some(),
            "oversize input evicts nothing"
        );
    }

    #[test]
    fn indexed_ring_replacement_preserves_full_hash_equality_and_fifo_order() {
        type Cache = FixedDiscoveryValidationCache<3, 3, 1, 5, 5>;
        let mut cache = Cache::default();
        let stamp = StampValue::new(16).unwrap();

        // Every key has the same Lemire key (the first eight SHA-256 bytes), exercising
        // probe-cluster deletion while the stable physical slots wrap more than once.
        for suffix in 1..=8 {
            let mut hash = [0xA5; 32];
            hash[31] = suffix;
            cache.remember_valid(hash, &[suffix], stamp);
            cache.remember_insufficient(hash, stamp);
        }

        for suffix in 1..=5 {
            let mut hash = [0xA5; 32];
            hash[31] = suffix;
            assert!(cache.valid(&hash).is_none());
            assert!(cache.insufficient(&hash).is_none());
        }
        for suffix in 6..=8 {
            let mut hash = [0xA5; 32];
            hash[31] = suffix;
            assert_eq!(cache.valid(&hash), Some((&[suffix][..], stamp)));
            assert_eq!(cache.insufficient(&hash), Some(stamp));
        }
    }

    #[test]
    fn zero_capacity_disables_only_memoization() {
        let mut cache = FixedDiscoveryValidationCache::<0, 0, 0, 1, 1>::default();
        let stamp = StampValue::new(16).unwrap();
        cache.remember_valid([1; 32], b"x", stamp);
        cache.remember_insufficient([1; 32], stamp);
        assert!(cache.valid(&[1; 32]).is_none());
        assert!(cache.insufficient(&[1; 32]).is_none());
    }
}
