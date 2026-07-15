mod fixed;
pub use fixed::FixedDestinationAnnounceLimitTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapDestinationAnnounceLimitTable;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        mod fixed_heap;

        pub use fixed_heap::FixedHeapDestinationAnnounceLimitTable;
    }
}
