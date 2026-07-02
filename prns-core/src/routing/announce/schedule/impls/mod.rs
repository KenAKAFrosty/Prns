mod fixed_scheduled_announce_queue;
pub use fixed_scheduled_announce_queue::FixedScheduledAnnounceQueue;

#[cfg(feature = "alloc")]
mod heap_scheduled_announce_queue;
#[cfg(feature = "alloc")]
pub use heap_scheduled_announce_queue::HeapScheduledAnnounceQueue;

#[cfg(feature = "external-alloc")]
mod fixed_heap_scheduled_announce_queue;
#[cfg(feature = "external-alloc")]
pub use fixed_heap_scheduled_announce_queue::FixedHeapScheduledAnnounceQueue;
