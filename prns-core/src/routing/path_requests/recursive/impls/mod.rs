mod fixed;
pub use fixed::FixedRecursivePathRequestTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapRecursivePathRequestTable;
    }
}
