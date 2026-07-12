mod fixed;
pub use fixed::FixedRecentPathRequestTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapRecentPathRequestTable;
