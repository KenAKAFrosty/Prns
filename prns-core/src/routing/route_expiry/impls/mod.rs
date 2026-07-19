mod linear;

pub use linear::LinearRouteExpiryIndex;

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        mod roaring;

        pub use roaring::RoaringRouteExpiryIndex;
    }
}
