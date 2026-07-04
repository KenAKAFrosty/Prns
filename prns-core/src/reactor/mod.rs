use crate::engine::InstantMillis;

// NOTE: allow(async_fn_in_trait) here and on every reactor trait: the lint flags the compiler-written future's missing Send bound, and the reactor is deliberately !Send single-threaded, so that bound is never wanted.
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
pub mod throughput;

pub(crate) mod window_ring;

#[cfg(any(feature = "tokio-host", feature = "embassy-host"))]
pub mod driver;
