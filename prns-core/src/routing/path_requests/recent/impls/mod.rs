mod fixed;
pub use fixed::FixedRecentPathRequestTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapRecentPathRequestTable;
    }
}
