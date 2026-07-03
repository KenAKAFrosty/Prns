use crate::engine::InstantMillis;

/// The reactor never reads a clock itself: the host owns time entirely (a
/// deterministic sim hands it any `now`).
#[allow(async_fn_in_trait)]
pub trait Host {
    fn now(&self) -> InstantMillis;
    async fn sleep_until(&self, deadline: InstantMillis);
    fn fill_entropy(&mut self, bytes: &mut [u8]);
}

pub mod airtime;
pub mod announce_pacer;
pub mod duty_gate;
pub mod throughput;
pub(crate) mod window_ring;

pub mod grant;

#[cfg(feature = "embassy-seam")]
pub mod timebase;

#[cfg(any(feature = "tokio-host", feature = "embassy-host"))]
pub mod driver;

pub mod interface_seam;
