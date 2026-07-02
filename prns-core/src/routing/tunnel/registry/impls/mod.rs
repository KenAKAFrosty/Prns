mod fixed;
#[cfg(feature = "alloc")]
mod heap;

pub use fixed::FixedTunnelColumns;
#[cfg(feature = "alloc")]
pub use heap::{HeapTunnelColumns, DEFAULT_MAX_TUNNELS};
