mod fixed;
pub use fixed::FixedRemoteControlAccessTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapRemoteControlAccessTable;
    }
}
