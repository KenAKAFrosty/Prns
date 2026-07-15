mod fixed;
pub use fixed::FixedInterfacePathRequestLimitTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapInterfacePathRequestLimitTable;
    }
}
