mod fixed;
pub use fixed::FixedSeenPathRequestTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::{HeapSeenPathRequestTable, DEFAULT_MAX_SEEN_PATH_REQUESTS};
