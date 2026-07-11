mod fixed;
pub use fixed::FixedReceiptTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::{HeapReceiptTable, DEFAULT_MAX_OUTSTANDING_RECEIPTS};
