mod fixed;
pub use fixed::FixedSelfRatchetTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::{HeapSelfRatchetTable, DEFAULT_RETAINED_RATCHETS};
