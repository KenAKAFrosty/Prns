mod fixed_discovery_path_request_columns;
pub use fixed_discovery_path_request_columns::FixedDiscoveryPathRequestColumns;

#[cfg(feature = "alloc")]
mod heap_discovery_path_request_columns;
#[cfg(feature = "alloc")]
pub use heap_discovery_path_request_columns::{
    HeapDiscoveryPathRequestColumns, DEFAULT_MAX_DISCOVERY_PATH_REQUESTS,
};
