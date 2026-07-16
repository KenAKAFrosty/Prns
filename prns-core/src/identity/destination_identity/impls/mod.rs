mod fixed;
mod fixed_array;
mod none;

pub use fixed::{destination_identity_index_buckets, FixedIndexedDestinationIdentityTable};
pub use fixed_array::FixedArrayDestinationIdentityTable;
pub use none::{NoDestinationIdentityAppData, NoDestinationIdentityTable};

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::HeapDestinationIdentityTable;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        mod fixed_heap;

        pub use fixed_heap::FixedHeapDestinationIdentityTable;
    }
}
