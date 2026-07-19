mod core;
mod impls;

pub use core::RouteInterfaceIndex;

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        pub use impls::{LinearRouteInterfaceIndex, RoaringRouteInterfaceIndex};
    } else {
        pub use impls::LinearRouteInterfaceIndex;
    }
}
