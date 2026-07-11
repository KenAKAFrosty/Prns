mod fixed_announce_id_history;
mod fixed_array_announce_record_table;
mod packed_app_data_arena;

pub use fixed_announce_id_history::FixedAnnounceIdHistory;
pub use fixed_array_announce_record_table::FixedArrayAnnounceRecordTable;
pub use packed_app_data_arena::PackedAppDataArena;

#[cfg(feature = "alloc")]
mod heap_announce_app_data;
#[cfg(feature = "alloc")]
mod heap_announce_id_history;
#[cfg(feature = "alloc")]
mod heap_announce_record_table;

#[cfg(feature = "alloc")]
pub use heap_announce_app_data::HeapAnnounceAppData;
#[cfg(feature = "alloc")]
pub use heap_announce_id_history::HeapAnnounceIdHistory;
#[cfg(feature = "alloc")]
pub use heap_announce_record_table::HeapAnnounceRecordTable;

#[cfg(feature = "external-alloc")]
mod fixed_heap_announce_id_history;
#[cfg(feature = "external-alloc")]
mod fixed_heap_announce_record_table;
#[cfg(feature = "external-alloc")]
mod fixed_heap_packed_app_data_arena;

#[cfg(feature = "external-alloc")]
pub use fixed_heap_announce_id_history::FixedHeapAnnounceIdHistory;
#[cfg(feature = "external-alloc")]
pub use fixed_heap_announce_record_table::FixedHeapAnnounceRecordTable;
#[cfg(feature = "external-alloc")]
pub use fixed_heap_packed_app_data_arena::FixedHeapPackedAppDataArena;
