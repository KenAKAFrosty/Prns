pub mod core;
mod dirty;
#[cfg(any(feature = "alloc", test, feature = "test-support"))]
mod impls;
pub use dirty::DirtyInterfaceSet;
#[cfg(any(feature = "alloc", test, feature = "test-support"))]
pub use impls::*;

pub use self::core::*;
