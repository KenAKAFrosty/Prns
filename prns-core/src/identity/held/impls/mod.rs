mod fixed;
pub use fixed::FixedHeldIdentityTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapHeldIdentityTable;
    }
}
