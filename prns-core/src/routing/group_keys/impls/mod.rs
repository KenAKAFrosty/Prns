mod fixed;
pub use fixed::FixedGroupKeyTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapGroupKeyTable;
