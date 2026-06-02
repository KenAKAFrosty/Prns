//! Concrete storage backends. Each file is one impl of one trait from the
//! parent [`storage`](crate::routing::storage) module.

mod fixed_array_retained_announce_columns;
mod fixed_array_route_columns;
mod packed_app_data_arena;
mod tiered_announce_id_history;

pub use fixed_array_retained_announce_columns::FixedArrayRetainedAnnounceColumns;
pub use fixed_array_route_columns::FixedArrayRouteColumns;
pub use packed_app_data_arena::PackedAppDataArena;
pub use tiered_announce_id_history::TieredAnnounceIdHistory;

#[cfg(feature = "alloc")]
pub use heap_route_columns::HeapRouteColumns;
#[cfg(feature = "alloc")]
mod heap_route_columns;
