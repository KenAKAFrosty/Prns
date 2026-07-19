pub mod core;
mod impls;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod interface_index;
    }
}

pub use impls::{FixedArrayRouteTable, FixedIndexedRouteTable};

cfg_if::cfg_if! {
    if #[cfg(feature = "external-alloc")] {
        pub use impls::FixedHeapRouteTable;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        pub use impls::{HeapRouteTable, LinearHeapRouteTable, RoaringHeapRouteTable};
    } else if #[cfg(feature = "alloc")] {
        pub use impls::{HeapRouteTable, LinearHeapRouteTable};
    }
}

pub use self::core::*;
