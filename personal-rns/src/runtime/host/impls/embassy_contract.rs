use embassy_futures::select::select;
use embassy_time::{Duration, Timer};

use super::super::{Host, NextWake};
use crate::engine::InstantMillis;
use crate::interfaces::substrate::{
    EmbassyHostSubstrate, EmbassyInterfaceChannels, EmbassyInterfaceHandle, EmbassyInterfaceSeam,
    EmbassyTimebase, WakeSignal,
};
use crate::interfaces::{Interface, InterfaceId, StartedInterface};

pub struct EmbassyContractHost<E> {
    wake: &'static WakeSignal,
    draw_entropy: E,
    timebase: EmbassyTimebase,
}

impl<E> EmbassyContractHost<E>
where
    E: FnMut(&mut [u8]),
{
    pub fn new(wake: &'static WakeSignal, draw_entropy: E) -> Self {
        Self::new_with_timebase(wake, EmbassyTimebase::capture_now(), draw_entropy)
    }

    pub fn new_with_timebase(
        wake: &'static WakeSignal,
        timebase: EmbassyTimebase,
        draw_entropy: E,
    ) -> Self {
        Self {
            wake,
            draw_entropy,
            timebase,
        }
    }

    pub fn glue_seam<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>(
        &self,
        id: InterfaceId,
        channels: &'static EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>,
    ) -> EmbassyInterfaceSeam<MTU, MAX_BUFFERED_PACKETS> {
        EmbassyInterfaceSeam::split_with_timebase(id, channels, self.wake, self.timebase)
    }

    pub fn attach<I, const MTU: usize, const MAX_BUFFERED_PACKETS: usize>(
        &self,
        interface: I,
        channels: &'static EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>,
    ) -> StartedInterface<EmbassyInterfaceHandle<MTU>, I::Worker>
    where
        I: Interface<EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>>,
    {
        let id = interface.descriptor().id;
        self.glue_seam(id, channels).start_interface(interface)
    }
}

impl<E> Host for EmbassyContractHost<E>
where
    E: FnMut(&mut [u8]),
{
    async fn wait(&mut self, wake: NextWake) -> InstantMillis {
        // Suspend until the engine's next deadline or an interface pokes the wake —
        // whichever first. This is the genuine `.await`: the executor sleeps the core
        // here. The signal is coalesced (a pending poke makes the next wait return at
        // once) and the runtime drains every interface's ring each cycle, so a missed
        // poke only costs the next deadline, never a packet.
        match wake {
            NextWake::Immediate => {}
            NextWake::At(deadline) => {
                let now = self.timebase.now();
                let timeout = Duration::from_millis(deadline.0.saturating_sub(now.0));
                if timeout != Duration::from_millis(0) {
                    let _ = select(self.wake.wait(), Timer::after(timeout)).await;
                }
            }
            NextWake::Idle => {
                self.wake.wait().await;
            }
        }
        self.timebase.now()
    }

    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        (self.draw_entropy)(bytes);
    }
}
