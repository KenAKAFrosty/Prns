mod fixed;
pub use fixed::FixedReceiptTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap;

        pub use heap::{HeapReceiptTable, DEFAULT_MAX_OUTSTANDING_RECEIPTS};
    }
}
