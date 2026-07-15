mod fixed;
pub use fixed::FixedSeenPathRequestTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapSeenPathRequestTable;
    }
}
