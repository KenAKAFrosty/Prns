mod fixed_upstream_app_destination_columns;
pub use fixed_upstream_app_destination_columns::FixedUpstreamAppDestinationColumns;

#[cfg(feature = "alloc")]
mod heap_upstream_app_destination_columns;
#[cfg(feature = "alloc")]
pub use heap_upstream_app_destination_columns::HeapUpstreamAppDestinationColumns;
