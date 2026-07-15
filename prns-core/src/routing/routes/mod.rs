pub mod core;
mod impls;

pub use impls::{FixedArrayRouteTable, FixedIndexedRouteTable};

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        pub use impls::{FixedHeapRouteTable, HeapRouteTable};
    } else if #[cfg(feature = "alloc")] {
        pub use impls::HeapRouteTable;
    }
}

pub use self::core::*;
