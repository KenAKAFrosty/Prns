mod fixed;
pub use fixed::FixedPendingPathRequestTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapPendingPathRequestTable;
    }
}
