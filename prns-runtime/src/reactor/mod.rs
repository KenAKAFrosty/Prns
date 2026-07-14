//! The core reactor seams, re-exported beside the two reactor bodies: paths under
//! `reactor::…` resolve here whether they name a seam (`grant`, `interface_seam`,
//! `driver`, [`Host`]) or a body (`impls::tokio_reactor`, `impls::embassy_reactor`).

pub use prns_core::reactor::*;

#[cfg(any(feature = "tokio-host", feature = "embassy-host"))]
pub mod impls;

/// The app's synchronous judgment seams, consulted inline on the reactor: RNS 1.3.5 `PROVE_APP` and `ACCEPT_APP`.
pub struct AppDeciders<P, A>
where
    P: FnMut(&prns_core::routing::proof::ProofRequest) -> bool,
    A: FnMut(&prns_core::routing::links::resources::ResourceOffer) -> bool,
{
    pub should_prove: P,
    pub should_accept_resource: A,
}

/// Every offer declined, every proof withheld: the posture of a reactor whose host installed no deciders.
#[must_use]
pub fn decline_all() -> AppDeciders<
    impl FnMut(&prns_core::routing::proof::ProofRequest) -> bool,
    impl FnMut(&prns_core::routing::links::resources::ResourceOffer) -> bool,
> {
    AppDeciders {
        should_prove: |_| false,
        should_accept_resource: |_| false,
    }
}

#[cfg(feature = "embassy-host")]
pub mod timebase;
