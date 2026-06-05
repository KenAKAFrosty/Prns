mod fixed_packet_hash_history;
pub use fixed_packet_hash_history::FixedPacketHashHistory;

#[cfg(feature = "alloc")]
mod heap_packet_hash_history;
#[cfg(feature = "alloc")]
pub use heap_packet_hash_history::HeapPacketHashHistory;
