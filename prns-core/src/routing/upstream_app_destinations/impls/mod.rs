mod fixed;
pub use fixed::FixedUpstreamAppDestinationTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapUpstreamAppDestinationTable;
