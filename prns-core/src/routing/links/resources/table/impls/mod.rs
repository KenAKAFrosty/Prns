mod fixed;
pub use fixed::FixedResourceTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::{HeapResourceTable, DEFAULT_MAX_RESOURCES};
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        mod fixed_heap;

        pub use fixed_heap::FixedHeapResourceTable;
    }
}
