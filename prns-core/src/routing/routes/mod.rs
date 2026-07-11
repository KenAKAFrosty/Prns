pub mod core;
mod impls;
pub use impls::FixedArrayRouteTable;
#[cfg(feature = "external-alloc")]
pub use impls::FixedHeapRouteTable;
pub use impls::FixedIndexedRouteTable;
#[cfg(feature = "alloc")]
pub use impls::HeapRouteTable;

pub use self::core::*;
