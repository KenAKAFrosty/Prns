mod fixed;
pub use fixed::FixedScheduledAnnounceQueue;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapScheduledAnnounceQueue;

#[cfg(feature = "external-alloc")]
mod fixed_heap;
#[cfg(feature = "external-alloc")]
pub use fixed_heap::FixedHeapScheduledAnnounceQueue;
