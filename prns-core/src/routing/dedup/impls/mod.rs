mod fixed;
pub use fixed::FixedPacketHashHistory;

mod fixed_indexed;
pub use fixed_indexed::FixedIndexedPacketHashHistory;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapPacketHashHistory;
    }
}
