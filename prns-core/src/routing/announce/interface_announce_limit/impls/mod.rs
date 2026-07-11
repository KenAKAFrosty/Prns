mod fixed;
pub use fixed::FixedInterfaceAnnounceLimitTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapInterfaceAnnounceLimitTable;
