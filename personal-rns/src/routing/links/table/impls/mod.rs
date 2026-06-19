mod fixed_link_columns;
pub use fixed_link_columns::FixedLinkColumns;

#[cfg(feature = "alloc")]
mod heap_link_columns;
#[cfg(feature = "alloc")]
pub use heap_link_columns::HeapLinkColumns;
