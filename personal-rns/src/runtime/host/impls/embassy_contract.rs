//! `EmbassyContractHost` — the embassy-executor [`Host`] for the contract runtime.
//!
//! The embassy twin of [`LinuxSync`](super::LinuxSync): it owns the embassy-time
//! clock, an injected CSPRNG draw, and the shared [`WakeSignal`] every interface
//! seam pokes. [`wait`](EmbassyContractHost::wait) genuinely suspends on
//! `select(wake.wait(), Timer::at(deadline))`, so an embassy executor drives the
//! generic [`run_contract`](crate::runtime::run_contract) loop. Inbound flows
//! through the interfaces' handles, drained by the
//! [`ContractRuntime`](crate::runtime::ContractRuntime) — not the host.
//!
//! There is no `clock_base`: the embassy clock is global, and the interface seams
//! stamp `arrived_at` off the same `Instant::now()`, so arrival and the cycle clock
//! already share a timebase with nothing to thread through.

use embassy_futures::select::select;
use embassy_time::{Instant as EmbassyInstant, Timer};

use super::super::{CycleStamp, Host};
use crate::engine::{EngineCycleEntropySeed, InstantMillis, NextScheduledEngineWork};
use crate::runtime::channels::embassy_seam::WakeSignal;

/// The embassy contract host: the shared wake the interface seams poke, an injected
/// CSPRNG draw, and the embassy-time clock + sleep. Hand it to
/// `ContractRuntime::new(state, started, host)`, then drive it from an embassy task
/// with `run_contract(runtime, observe).await`. `E` is the host's per-cycle CSPRNG
/// draw (the one substrate piece `personal-rns` can't name itself, e.g. the ESP
/// hardware RNG).
pub struct EmbassyContractHost<E> {
    wake: &'static WakeSignal,
    draw_entropy: E,
}

impl<E> EmbassyContractHost<E>
where
    E: FnMut() -> EngineCycleEntropySeed,
{
    /// `wake` is the host's one shared signal — every interface seam holds a
    /// `&'static` to it and signals on `submit` / `report`, so a suspended `wait`
    /// returns the moment any interface has something. `draw_entropy` is the device
    /// CSPRNG.
    pub fn new(wake: &'static WakeSignal, draw_entropy: E) -> Self {
        Self { wake, draw_entropy }
    }
}

impl<E> Host for EmbassyContractHost<E>
where
    E: FnMut() -> EngineCycleEntropySeed,
{
    async fn wait(&mut self, wake: NextScheduledEngineWork) -> CycleStamp {
        // Suspend until the engine's next deadline or an interface pokes the wake —
        // whichever first. This is the genuine `.await`: the executor sleeps the core
        // here. The signal is coalesced (a pending poke makes the next wait return at
        // once) and the runtime drains every interface's ring each cycle, so a missed
        // poke only costs the next deadline, never a packet.
        match wake {
            NextScheduledEngineWork::Immediate => {}
            NextScheduledEngineWork::At(deadline) => {
                let at = EmbassyInstant::from_millis(deadline.0);
                let _ = select(self.wake.wait(), Timer::at(at)).await;
            }
            NextScheduledEngineWork::Idle => {
                self.wake.wait().await;
            }
        }

        CycleStamp {
            now: InstantMillis(EmbassyInstant::now().as_millis()),
            seed: (self.draw_entropy)(),
        }
    }
}
