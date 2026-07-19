mod core;
mod impls;

pub use core::{RouteExpiryIndex, ROUTE_EXPIRY_QUANTUM_MS};
pub use impls::LinearRouteExpiryIndex;

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        pub use impls::RoaringRouteExpiryIndex;
    }
}

#[cfg(test)]
mod tests;
