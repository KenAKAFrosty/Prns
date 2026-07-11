mod fixed_array;
pub use fixed_array::FixedArrayRouteTable;

mod fixed_indexed;
pub use fixed_indexed::FixedIndexedRouteTable;

#[cfg(feature = "external-alloc")]
mod fixed_heap;
#[cfg(feature = "external-alloc")]
pub use fixed_heap::FixedHeapRouteTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapRouteTable;
