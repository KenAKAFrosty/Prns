mod fixed_local_destination_columns;
pub use fixed_local_destination_columns::FixedLocalDestinationColumns;

#[cfg(feature = "alloc")]
mod heap_local_destination_columns;
#[cfg(feature = "alloc")]
pub use heap_local_destination_columns::HeapLocalDestinationColumns;
