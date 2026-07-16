mod fixed;
mod fixed_array;
mod none;

pub use fixed::{known_destination_index_buckets, FixedIndexedKnownDestinationTable};
pub use fixed_array::FixedArrayKnownDestinationTable;
pub use none::{NoKnownDestinationAppData, NoKnownDestinationTable};

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapKnownDestinationTable;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        mod fixed_heap;

        pub use fixed_heap::FixedHeapKnownDestinationTable;
    }
}
