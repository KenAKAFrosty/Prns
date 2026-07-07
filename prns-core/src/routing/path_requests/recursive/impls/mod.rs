mod fixed_recursive_path_request_columns;
pub use fixed_recursive_path_request_columns::FixedRecursivePathRequestColumns;

#[cfg(feature = "alloc")]
mod heap_recursive_path_request_columns;
#[cfg(feature = "alloc")]
pub use heap_recursive_path_request_columns::{
    HeapRecursivePathRequestColumns, DEFAULT_MAX_RECURSIVE_PATH_REQUESTS,
};
