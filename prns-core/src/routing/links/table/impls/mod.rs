mod fixed;
pub use fixed::FixedLinkTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapLinkTable;
    }
}
