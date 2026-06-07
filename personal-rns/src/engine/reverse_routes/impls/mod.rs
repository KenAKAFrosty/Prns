mod fixed_reverse_route_columns;
pub use fixed_reverse_route_columns::FixedReverseRouteColumns;

#[cfg(feature = "alloc")]
mod heap_reverse_route_columns;
#[cfg(feature = "alloc")]
pub use heap_reverse_route_columns::{HeapReverseRouteColumns, DEFAULT_MAX_REVERSE_ROUTES};
