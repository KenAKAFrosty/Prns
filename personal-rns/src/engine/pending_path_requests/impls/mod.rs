mod fixed_pending_path_request_columns;
pub use fixed_pending_path_request_columns::FixedPendingPathRequestColumns;

#[cfg(feature = "alloc")]
mod heap_pending_path_request_columns;
#[cfg(feature = "alloc")]
pub use heap_pending_path_request_columns::{
    HeapPendingPathRequestColumns, DEFAULT_MAX_PENDING_PATH_REQUESTS,
};
