mod fixed;
pub use fixed::FixedDestinationAnnounceLimitTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapDestinationAnnounceLimitTable;

#[cfg(feature = "external-alloc")]
mod fixed_heap;
#[cfg(feature = "external-alloc")]
pub use fixed_heap::FixedHeapDestinationAnnounceLimitTable;
