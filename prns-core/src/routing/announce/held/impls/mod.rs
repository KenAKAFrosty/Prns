mod soa;
pub use soa::{DirEntry, HeldCold, Probe, SoaColumns, SoaHeldAnnounceTable};

mod fixed;
pub use fixed::{FixedHeldAnnounceTable, FixedSoaColumns};

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        mod fixed_heap;

        pub use fixed_heap::{FixedHeapHeldAnnounceTable, FixedHeapSoaColumns};
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapHeldAnnounceTable;
    }
}
