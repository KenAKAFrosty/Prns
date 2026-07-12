mod fixed;
#[cfg(feature = "alloc")]
mod heap;

pub use fixed::FixedTunnelTable;
#[cfg(feature = "alloc")]
pub use heap::HeapTunnelTable;
