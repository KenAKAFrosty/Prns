mod fixed_receipt_columns;
pub use fixed_receipt_columns::FixedReceiptColumns;

#[cfg(feature = "alloc")]
mod heap_receipt_columns;
#[cfg(feature = "alloc")]
pub use heap_receipt_columns::{HeapReceiptColumns, DEFAULT_MAX_OUTSTANDING_RECEIPTS};
