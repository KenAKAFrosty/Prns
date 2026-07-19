mod linear;

pub use linear::LinearRouteInterfaceIndex;

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        mod roaring;

        pub use roaring::RoaringRouteInterfaceIndex;
    }
}
