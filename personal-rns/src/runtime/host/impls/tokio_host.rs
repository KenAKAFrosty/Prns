use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::super::{CycleStamp, Host, NextWake};
use crate::engine::{EngineCycleEntropySeed, InstantMillis, ENGINE_CYCLE_ENTROPY_LEN};
use crate::interfaces::substrate::{TokioHostSubstrate, TokioInterfaceHandle, TokioInterfaceSeam};
use crate::interfaces::{Interface, InterfaceId, StartedInterface};

/// Upper bound on one wait, so a far-off deadline or an idle engine still
/// loops back periodically. A daemon host is mains-powered; the cap is free.
const MAX_WAIT: Duration = Duration::from_secs(1);

pub struct TokioHost {
    wake: Arc<Notify>,
    clock_base: Instant,
}

impl TokioHost {
    pub fn new() -> Self {
        Self {
            wake: Arc::new(Notify::new()),
            clock_base: Instant::now(),
        }
    }

    pub fn glue_seam<const MTU: usize>(
        &self,
        id: InterfaceId,
        max_buffered_packets: usize,
    ) -> TokioInterfaceSeam<MTU> {
        TokioInterfaceSeam::new(id, self.clock_base, max_buffered_packets, self.wake.clone())
    }

    pub fn attach<I, const MTU: usize>(
        &self,
        interface: I,
        max_buffered_packets: usize,
    ) -> StartedInterface<TokioInterfaceHandle<MTU>, I::Worker>
    where
        I: Interface<TokioHostSubstrate<MTU>>,
    {
        let id = interface.descriptor().id;
        self.glue_seam(id, max_buffered_packets)
            .start_interface(interface)
    }

    fn now(&self) -> InstantMillis {
        InstantMillis(self.clock_base.elapsed().as_millis() as u64)
    }
}

impl Default for TokioHost {
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

impl Host for TokioHost {
    #[allow(clippy::expect_used)]
    async fn wait(&mut self, wake: NextWake) -> CycleStamp {
        // Suspend until the engine's next deadline or an interface pokes the
        // wake — whichever first — yielding the executor to the worker tasks
        // meanwhile. The wake is coalesced (`Notify` holds one permit); the
        // runtime drains every interface's ring each cycle regardless, so a
        // missed poke only costs the next deadline, never a packet.
        let timeout = wait_for(wake, self.now(), MAX_WAIT);
        if !timeout.is_zero() {
            let _ = tokio::time::timeout(timeout, self.wake.notified()).await;
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

    #[tokio::test]
    async fn wait_returns_promptly_when_an_interface_pokes_the_wake() {
        let mut host = TokioHost::new();
        let mut seam = host.glue_seam::<8>(InterfaceId::new([0; 16]), 1);

        let poker = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            seam.worker_context
                .inbound
                .submit(|buf| {
                    buf[0] = 1;
                    1
                })
                .unwrap();
        });

        let _stamp = tokio::time::timeout(Duration::from_secs(5), host.wait(NextWake::Idle))
            .await
            .expect("the poke must cut the idle wait short, not run out the 1s cap");
        poker.await.unwrap();
    }

    #[tokio::test]
    async fn an_immediate_wake_does_not_wait() {
        let mut host = TokioHost::new();
        let stamp = host.wait(NextWake::Immediate).await;
        assert!(stamp.now.0 < 100);
    }
}
