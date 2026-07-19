mod fixed_array;
pub use fixed_array::FixedArrayRouteTable;

mod fixed_indexed;
pub use fixed_indexed::FixedIndexedRouteTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        mod fixed_heap;

        pub use fixed_heap::FixedHeapRouteTable;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        mod heap;

        pub use heap::{HeapRouteTable, LinearHeapRouteTable, RoaringHeapRouteTable};
    } else if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::{HeapRouteTable, LinearHeapRouteTable};
    }
}
