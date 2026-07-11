mod fixed;
pub use fixed::FixedPacketHashHistory;

mod fixed_indexed;
pub use fixed_indexed::FixedIndexedPacketHashHistory;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapPacketHashHistory;
