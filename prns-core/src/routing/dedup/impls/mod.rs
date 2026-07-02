mod fixed_packet_hash_history;
pub use fixed_packet_hash_history::FixedPacketHashHistory;

mod fixed_indexed_packet_hash_history;
pub use fixed_indexed_packet_hash_history::FixedIndexedPacketHashHistory;

#[cfg(feature = "alloc")]
mod heap_packet_hash_history;
#[cfg(feature = "alloc")]
pub use heap_packet_hash_history::HeapPacketHashHistory;
