use crate::engine::InstantMillis;

#[allow(async_fn_in_trait)]
pub trait Host {
    fn now(&self) -> InstantMillis;
    async fn sleep_until(&self, deadline: InstantMillis);
    fn fill_entropy(&mut self, bytes: &mut [u8]);
}

pub mod airtime;
pub mod announce_pacer;
pub mod duty_gate;
pub mod grant;
pub mod interface_seam;
pub mod kernel;
pub mod throughput;
pub mod timers;

pub(crate) mod window_ring;

cfg_if::cfg_if! {
    if #[cfg(feature = "tokio-host")] {
        pub mod impls;
    }
}

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
