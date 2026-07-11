mod fixed;
pub use fixed::FixedLinkTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapLinkTable;
