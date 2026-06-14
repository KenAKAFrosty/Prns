mod fixed_resource_columns;
pub use fixed_resource_columns::FixedResourceColumns;

#[cfg(feature = "alloc")]
mod heap_resource_columns;
#[cfg(feature = "alloc")]
pub use heap_resource_columns::{HeapResourceColumns, DEFAULT_MAX_RESOURCES};

#[cfg(feature = "external-alloc")]
mod fixed_heap_resource_columns;
#[cfg(feature = "external-alloc")]
pub use fixed_heap_resource_columns::FixedHeapResourceColumns;
