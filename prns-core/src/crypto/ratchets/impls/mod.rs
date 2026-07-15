mod fixed;
pub use fixed::FixedSelfRatchetTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::{HeapSelfRatchetTable, DEFAULT_RETAINED_RATCHETS};
    }
}
