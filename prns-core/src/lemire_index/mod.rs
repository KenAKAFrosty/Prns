//! A side index answering "which row holds this key" without scanning the table.
//!
//! Each bucket holds one row number.
//! A key picks its bucket by multiplying its own leading bytes against the bucket count and keeping the high half.
//! Our keys are usually truncated hashes, already uniform, so their bytes need no extra hashing (Lemire's reduction).
//! A taken bucket sends the newcomer one to the right, a lookup walks the same way, and the first empty bucket it meets is proof the key is absent.
//! Removal therefore can't simply empty a bucket mid-run (a later key's walk would stop short at the hole) so the entries after it re-pack, each moving back only if its own home bucket still reaches it there.
//!
//! [`LemireIndex`]'s callers hold two invariants with const asserts: `BUCKETS` keeps free headroom over the table's capacity through their domain sizing functions, so a missing key always meets an empty bucket rather than walking forever; and tables stay below `u16::MAX` rows because slot numbers are `u16` and reserve their top value as the empty marker.
//! [`HeapLemireIndex`] serves the growable tables and holds both invariants itself: its buckets double (re-placing every key) whenever one more row would pass 2/3 full, and its slots widen to `u32`.

mod core;
mod impls;
mod keys;

pub(crate) use core::buckets_for_two_thirds_load;
pub use core::{IndexKey, IndexRow};
pub use impls::LemireIndex;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        pub use impls::HeapLemireIndex;
    }
}

#[cfg(test)]
mod tests;
