mod fixed;
pub use fixed::{FixedRemoteControlControllerGrantTable, FixedRemoteControlTargetAccessTable};

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::{HeapRemoteControlControllerGrantTable, HeapRemoteControlTargetAccessTable};
    }
}
