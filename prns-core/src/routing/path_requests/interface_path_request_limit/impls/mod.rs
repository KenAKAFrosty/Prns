mod fixed;
pub use fixed::FixedInterfacePathRequestLimitTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapInterfacePathRequestLimitTable;
