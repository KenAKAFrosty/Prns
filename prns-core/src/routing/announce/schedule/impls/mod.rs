mod fixed;
pub use fixed::FixedScheduledAnnounceQueue;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapScheduledAnnounceQueue;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        mod fixed_heap;

        pub use fixed_heap::FixedHeapScheduledAnnounceQueue;
    }
}
