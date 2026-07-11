mod fixed;
pub use fixed::FixedHeldIdentityTable;

#[cfg(feature = "alloc")]
mod heap;
#[cfg(feature = "alloc")]
pub use heap::HeapHeldIdentityTable;
