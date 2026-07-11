mod fixed;
pub use fixed::FixedPendingPathRequestTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::{HeapPendingPathRequestTable, DEFAULT_MAX_PENDING_PATH_REQUESTS};
