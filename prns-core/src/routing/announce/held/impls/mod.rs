mod fixed_held_announce_pool;
pub use fixed_held_announce_pool::FixedHeldAnnouncePool;

#[cfg(feature = "alloc")]
mod heap_held_announce_pool;
#[cfg(feature = "alloc")]
pub use heap_held_announce_pool::HeapHeldAnnouncePool;

#[cfg(feature = "external-alloc")]
mod fixed_heap_held_announce_pool;
#[cfg(feature = "external-alloc")]
pub use fixed_heap_held_announce_pool::FixedHeapHeldAnnouncePool;
