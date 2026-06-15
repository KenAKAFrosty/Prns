mod fixed_recent_path_request_columns;
pub use fixed_recent_path_request_columns::FixedRecentPathRequestColumns;

#[cfg(feature = "alloc")]
mod heap_recent_path_request_columns;
#[cfg(feature = "alloc")]
pub use heap_recent_path_request_columns::{
    HeapRecentPathRequestColumns, DEFAULT_MAX_RECENT_PATH_REQUESTS,
};
