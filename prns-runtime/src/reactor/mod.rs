//! The core reactor seams, re-exported beside the two reactor bodies: paths under
//! `reactor::…` resolve here whether they name a seam (`grant`, `interface_seam`,
//! `driver`, [`Host`]) or a body (`impls::tokio_reactor`, `impls::embassy_reactor`).

pub use prns_core::reactor::*;

#[cfg(any(feature = "tokio-host", feature = "embassy-host"))]
pub mod impls;
