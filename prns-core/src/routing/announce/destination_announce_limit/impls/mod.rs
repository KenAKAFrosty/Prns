mod fixed_destination_announce_limit_columns;
pub use fixed_destination_announce_limit_columns::FixedDestinationAnnounceLimitColumns;

#[cfg(feature = "alloc")]
mod heap_destination_announce_limit_columns;
#[cfg(feature = "alloc")]
pub use heap_destination_announce_limit_columns::HeapDestinationAnnounceLimitColumns;

#[cfg(feature = "external-alloc")]
mod fixed_heap_destination_announce_limit_columns;
#[cfg(feature = "external-alloc")]
pub use fixed_heap_destination_announce_limit_columns::FixedHeapDestinationAnnounceLimitColumns;
