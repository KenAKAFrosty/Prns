mod fixed;
pub use fixed::FixedGroupKeyTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapGroupKeyTable;
    }
}
