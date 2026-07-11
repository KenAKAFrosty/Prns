mod fixed;
pub use fixed::FixedReverseRouteTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::{HeapReverseRouteTable, DEFAULT_MAX_REVERSE_ROUTES};
