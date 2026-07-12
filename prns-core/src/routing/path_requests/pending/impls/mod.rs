mod fixed;
pub use fixed::FixedPendingPathRequestTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapPendingPathRequestTable;
