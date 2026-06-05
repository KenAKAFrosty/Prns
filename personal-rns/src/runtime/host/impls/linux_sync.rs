use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::{Duration, Instant};

use super::super::{CycleStamp, Host, NextWake};
use crate::engine::{EngineCycleEntropySeed, InstantMillis, ENGINE_CYCLE_ENTROPY_LEN};
use crate::interfaces::substrate::{StdHostSubstrate, StdInterfaceHandle, StdInterfaceSeam};
use crate::interfaces::{Interface, InterfaceId, StartedInterface};

/// Upper bound on one blocking wait, so a far-off deadline or an idle engine still
/// loops back periodically. A daemon host is mains-powered; the cap is free.
const MAX_WAIT: Duration = Duration::from_secs(1);

pub struct LinuxSync {
    wake: Receiver<()>,
    wake_sender: SyncSender<()>,
    clock_base: Instant,
}

impl LinuxSync {
    pub fn new() -> Self {
        let (wake_sender, wake) = sync_channel::<()>(1);
        Self {
            wake,
            wake_sender,
            clock_base: Instant::now(),
        }
    }

    pub fn glue_seam<const MTU: usize>(
        &self,
        id: InterfaceId,
        max_buffered_packets: usize,
    ) -> StdInterfaceSeam<MTU> {
        StdInterfaceSeam::new(
            id,
            self.clock_base,
            max_buffered_packets,
            self.wake_sender.clone(),
        )
    }

    pub fn attach<I, const MTU: usize>(
        &self,
        interface: I,
        max_buffered_packets: usize,
    ) -> StartedInterface<StdInterfaceHandle<MTU>, I::Worker>
    where
        I: Interface<StdHostSubstrate<MTU>>,
    {
        let id = interface.descriptor().id;
        self.glue_seam(id, max_buffered_packets)
            .start_interface(interface)
    }

    fn now(&self) -> InstantMillis {
        InstantMillis(self.clock_base.elapsed().as_millis() as u64)
    }
}

impl Default for LinuxSync {
    fn default() -> Self {
        Self::new()
    }
}

fn wait_for(next: NextWake, now: InstantMillis, max: Duration) -> Duration {
    match next {
        NextWake::Immediate => Duration::ZERO,
        NextWake::Idle => max,
        NextWake::At(deadline) => Duration::from_millis(deadline.0.saturating_sub(now.0)).min(max),
    }
}

impl Host for LinuxSync {
    #[allow(clippy::expect_used)]
    async fn wait(&mut self, wake: NextWake) -> CycleStamp {
        // Block until the engine's next deadline or an interface pokes the wake —
        // whichever first. `recv_timeout` blocks the thread (this never `.await`s),
        // so `block_on` carries the loop straight through with no executor. The
        // wake is coalesced (a pending poke just returns immediately); the runtime
        // drains every interface's ring each cycle regardless, so a missed poke
        // only costs the next deadline, never a packet.
        let timeout = wait_for(wake, self.now(), MAX_WAIT);
        if !timeout.is_zero() {
            let _ = self.wake.recv_timeout(timeout);
        }

        let mut seed = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        getrandom::getrandom(&mut seed).expect("OS CSPRNG must provide cycle entropy");
        CycleStamp {
            now: self.now(),
            seed: EngineCycleEntropySeed::new(seed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::InboundSink;
    use crate::runtime::block_on;

    #[test]
    fn wait_returns_promptly_when_an_interface_pokes_the_wake() {
        let mut host = LinuxSync::new();
        let mut seam = host.glue_seam::<8>(InterfaceId::new([0; 16]), 1);
        seam.worker_context
            .inbound
            .submit(|buf| {
                buf[0] = 1;
                1
            })
            .unwrap();
        let _stamp = block_on(host.wait(NextWake::Idle));
    }
}
