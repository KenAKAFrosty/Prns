//! `EmbassyContractHost` — the embassy-executor [`Host`] for the runtime.
//!
//! The embassy twin of [`LinuxSync`](super::LinuxSync): it owns the embassy-time
//! clock, an injected CSPRNG draw, and the shared [`WakeSignal`] every interface
//! seam pokes. [`wait`](EmbassyContractHost::wait) genuinely suspends on
//! `select(wake.wait(), Timer::at(deadline))`, so an embassy executor drives the
//! generic [`Runtime::run`](crate::runtime::Runtime::run) loop. Inbound flows
//! through the interfaces' handles, drained by the
//! [`Runtime`](crate::runtime::Runtime) — not the host.
//!
//! There is no `clock_base`: the embassy clock is global, and the interface seams
//! stamp `arrived_at` off the same `Instant::now()`, so arrival and the cycle clock
//! already share a timebase with nothing to thread through.

use embassy_futures::select::select;
use embassy_time::{Instant as EmbassyInstant, Timer};

use super::super::{CycleStamp, Host};
use crate::engine::{EngineCycleEntropySeed, InstantMillis, NextScheduledEngineWork};
use crate::interfaces::substrate::{
    EmbassyHostSubstrate, EmbassyInterfaceChannels, EmbassyInterfaceHandle, EmbassyInterfaceSeam,
    WakeSignal,
};
use crate::interfaces::{Interface, InterfaceId, StartedInterface};

/// The embassy contract host: the shared wake the interface seams poke, an injected
/// CSPRNG draw, and the embassy-time clock + sleep. Hand it to
/// `Runtime::new(state, started, host)`, then drive it from an embassy task
/// with `runtime.run(on_snapshot).await`. `E` is the host's per-cycle CSPRNG
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

    /// Glue an interface seam bound to this host's shared wake. The board owns the
    /// `'static` channels (no heap); the host supplies the one wake every seam pokes.
    /// `MTU`/`MAX_BUFFERED_PACKETS` are inferred from the channels' type.
    pub fn glue_seam<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>(
        &self,
        id: InterfaceId,
        channels: &'static EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>,
    ) -> EmbassyInterfaceSeam<MTU, MAX_BUFFERED_PACKETS> {
        EmbassyInterfaceSeam::split(id, channels, self.wake)
    }

    /// Attach `interface` to this host and hand back the [`StartedInterface`] the
    /// runtime pools: glue a seam (from the board's `'static` channels) keyed by the id
    /// the interface already carries, then start the interface onto it. The `glue_seam`
    /// + `start_interface` dance, collapsed — reach for `glue_seam` directly only when a
    /// host splits the seam by hand (a multi-interface board unifying heterogeneous
    /// handles).
    pub fn attach<I, const MTU: usize, const MAX_BUFFERED_PACKETS: usize>(
        &self,
        interface: I,
        channels: &'static EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>,
    ) -> StartedInterface<EmbassyInterfaceHandle<MTU, MAX_BUFFERED_PACKETS>, I::Worker>
    where
        I: Interface<EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>>,
    {
        let id = interface.descriptor().id;
        self.glue_seam(id, channels).start_interface(interface)
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
