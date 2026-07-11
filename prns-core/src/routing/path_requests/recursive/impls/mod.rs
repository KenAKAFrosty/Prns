mod fixed;
pub use fixed::FixedRecursivePathRequestTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::{HeapRecursivePathRequestTable, DEFAULT_MAX_RECURSIVE_PATH_REQUESTS};
