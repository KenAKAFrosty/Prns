mod fixed;
pub use fixed::FixedResourceTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::{HeapResourceTable, DEFAULT_MAX_RESOURCES};

#[cfg(feature = "external-alloc")]
mod fixed_heap;
#[cfg(feature = "external-alloc")]
pub use fixed_heap::FixedHeapResourceTable;
