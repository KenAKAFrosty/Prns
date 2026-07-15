mod fixed;
pub use fixed::FixedTunnelTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapTunnelTable;
    }
}
