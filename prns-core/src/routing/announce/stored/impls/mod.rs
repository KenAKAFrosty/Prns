mod fixed_announce_id_history;
mod fixed_array_announce_record_table;
mod packed_app_data_arena;

pub use fixed_announce_id_history::FixedAnnounceIdHistory;
pub use fixed_array_announce_record_table::FixedArrayAnnounceRecordTable;
pub use packed_app_data_arena::PackedAppDataArena;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod heap_announce_app_data;
        mod heap_announce_id_history;
        mod heap_announce_record_table;

        pub use heap_announce_app_data::HeapAnnounceAppData;
        pub use heap_announce_id_history::HeapAnnounceIdHistory;
        pub use heap_announce_record_table::HeapAnnounceRecordTable;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        mod fixed_heap_announce_id_history;
        mod fixed_heap_announce_record_table;
        mod fixed_heap_packed_app_data_arena;

        pub use fixed_heap_announce_id_history::FixedHeapAnnounceIdHistory;
        pub use fixed_heap_announce_record_table::FixedHeapAnnounceRecordTable;
        pub use fixed_heap_packed_app_data_arena::FixedHeapPackedAppDataArena;
    }
}
