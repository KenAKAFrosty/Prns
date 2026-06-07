mod fixed_seen_path_request_columns;
pub use fixed_seen_path_request_columns::FixedSeenPathRequestColumns;

#[cfg(feature = "alloc")]
mod heap_seen_path_request_columns;
#[cfg(feature = "alloc")]
pub use heap_seen_path_request_columns::{
    HeapSeenPathRequestColumns, DEFAULT_MAX_SEEN_PATH_REQUESTS,
};
