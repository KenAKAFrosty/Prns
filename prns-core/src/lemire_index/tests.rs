use super::core::{buckets_for_two_thirds_load, exceeds_two_thirds_load};
use super::{IndexKey, LemireIndex};
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;

#[test]
fn bucket_sizing_preserves_two_thirds_headroom_without_overflow() {
    assert_eq!(buckets_for_two_thirds_load(0), 1);
    assert_eq!(buckets_for_two_thirds_load(1), 2);
    assert_eq!(buckets_for_two_thirds_load(2), 3);
    assert_eq!(buckets_for_two_thirds_load(3), 5);
    assert_eq!(buckets_for_two_thirds_load(1_024), 1_536);
    assert_eq!(buckets_for_two_thirds_load(usize::MAX), usize::MAX);
    assert!(!exceeds_two_thirds_load(5, 8));
    assert!(exceeds_two_thirds_load(6, 8));
    assert!(!exceeds_two_thirds_load(usize::MAX / 2, usize::MAX));
}

#[test]
fn interface_ids_sharing_a_kind_byte_still_spread_across_buckets() {
    let a = InterfaceId::new([0x07, 0, 0, 0, 0, 0, 0, 0x11]);
    let b = InterfaceId::new([0x07, 0, 0, 0, 0, 0, 0, 0x22]);
    assert_eq!(a.lemire_key() & 0xff, 0x07);
    assert_ne!(a.lemire_key() >> 56, b.lemire_key() >> 56);
}

#[test]
fn fixed_slot_removal_preserves_a_newer_duplicate_key() {
    let duplicate = DestinationHash::new([7; 16]);
    let other = DestinationHash::new([8; 16]);
    let keys = [duplicate, other, duplicate];
    let mut index = LemireIndex::<8>::default();
    for slot in 0..keys.len() {
        index.insert(slot, &keys);
    }

    index.remove_slot(0, &keys);

    assert_eq!(index.get(&duplicate, &keys), Some(2));
    assert_eq!(index.get(&other, &keys), Some(1));

    index.remove_slot(2, &keys);

    assert_eq!(index.get(&duplicate, &keys), None);
    assert_eq!(index.get(&other, &keys), Some(1));
}

#[cfg(feature = "alloc")]
mod heap {
    use super::super::HeapLemireIndex;
    use crate::wire::DestinationHash;

    fn dest_n(n: u32) -> DestinationHash {
        let key = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&key.to_be_bytes());
        b[8..12].copy_from_slice(&n.to_be_bytes());
        DestinationHash::new(b)
    }

    #[test]
    fn every_key_stays_findable_across_rebuilds_and_absent_keys_miss() {
        let mut index = HeapLemireIndex::default();
        let mut keys = std::vec::Vec::new();
        for n in 0..1_000u32 {
            keys.push(dest_n(n));
            index.insert(keys.len() - 1, &keys);
        }
        for (slot, key) in keys.iter().enumerate() {
            assert_eq!(index.get(key, &keys), Some(slot));
        }
        assert_eq!(index.get(&dest_n(1_000), &keys), None);
    }

    #[test]
    fn removal_re_packs_so_later_keys_survive_and_repoint_follows_a_swap() {
        let mut index = HeapLemireIndex::default();
        let mut keys = std::vec::Vec::new();
        for n in 0..100u32 {
            keys.push(dest_n(n));
            index.insert(keys.len() - 1, &keys);
        }

        let removed = keys[10];
        index.remove(&removed, &keys);
        let moved = keys[keys.len() - 1];
        index.repoint(&moved, 10, &keys);
        keys.swap_remove(10);

        assert_eq!(index.get(&removed, &keys), None);
        for (slot, key) in keys.iter().enumerate() {
            assert_eq!(index.get(key, &keys), Some(slot));
        }
    }

    #[test]
    fn slot_removal_and_repoint_preserve_duplicate_keys() {
        let duplicate = dest_n(7);
        let mut keys = std::vec![duplicate, dest_n(8), duplicate];
        let mut index = HeapLemireIndex::default();
        for slot in 0..keys.len() {
            index.insert(slot, &keys);
        }

        index.remove_slot(0, &keys);
        index.repoint_slot(2, 0, &keys);
        keys.swap_remove(0);

        assert_eq!(index.get(&duplicate, &keys), Some(0));
        assert_eq!(index.get(&dest_n(8), &keys), Some(1));
        index.remove_slot(0, &keys);
        index.repoint_slot(1, 0, &keys);
        keys.swap_remove(0);
        assert_eq!(index.get(&duplicate, &keys), None);
        assert_eq!(index.get(&dest_n(8), &keys), Some(0));
    }

    #[test]
    fn clear_empties_every_bucket_without_shrinking() {
        let mut index = HeapLemireIndex::default();
        let mut keys = std::vec::Vec::new();
        for n in 0..100u32 {
            keys.push(dest_n(n));
            index.insert(keys.len() - 1, &keys);
        }
        index.clear();
        assert_eq!(index.get(&dest_n(0), &keys), None);

        keys.clear();
        keys.push(dest_n(7));
        index.insert(0, &keys);
        assert_eq!(index.get(&dest_n(7), &keys), Some(0));
    }
}
