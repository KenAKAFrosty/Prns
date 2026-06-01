//! `EmbassyHost` — the embassy-executor [`Host`] for any embassy-net device
//! (ESP32-S3, ESP32-C6, nRF, …).
//!
//! Owns the embassy substrate: the embassy-time clock, the shared inbound
//! `Channel` the worker tasks stamp into, and the sleep. [`wait`](EmbassyHost::wait)
//! genuinely suspends on `select(inbound.receive(), Timer::at(deadline))`, so an
//! embassy executor drives the generic [`run`](crate::runtime::run) loop. The
//! device-specific entropy source (e.g. the ESP32 hardware RNG) is injected as a
//! closure, since `personal-rns` names no specific HAL.

use embassy_futures::select::{select, Either};
use embassy_time::{Instant as EmbassyInstant, Timer};
use heapless::Vec as HVec;

use super::super::{CycleStamp, Host};
use crate::engine::{
    EngineCycleEntropySeed, InboundPacket, InstantMillis, NextScheduledEngineWork,
};
use crate::runtime::channels::embassy::{InboundReceiver, InboxEntry, INBOX_DEPTH};

/// The embassy host: the inbound `Channel` the worker tasks stamp into, an
/// injected entropy draw, and the embassy-time clock + sleep. `PACKET_BUFFER_SIZE`
/// is the registered worker's `InterfaceWorker::PACKET_BUFFER_SIZE` (it sizes the
/// mailbox); `E` is the host's CSPRNG draw.
pub struct EmbassyHost<const PACKET_BUFFER_SIZE: usize, E> {
    inbound: InboundReceiver<PACKET_BUFFER_SIZE>,
    draw_entropy: E,
    batch: HVec<InboxEntry<PACKET_BUFFER_SIZE>, INBOX_DEPTH>,
}

impl<const PACKET_BUFFER_SIZE: usize, E> EmbassyHost<PACKET_BUFFER_SIZE, E>
where
    E: FnMut() -> EngineCycleEntropySeed,
{
    /// `inbound` is the draining end of the mailbox the worker tasks stamp into;
    /// `draw_entropy` is the host's CSPRNG (the one substrate piece `personal-rns`
    /// can't name itself, e.g. the ESP32 hardware RNG).
    pub fn new(inbound: InboundReceiver<PACKET_BUFFER_SIZE>, draw_entropy: E) -> Self {
        Self {
            inbound,
            draw_entropy,
            batch: HVec::new(),
        }
    }
}

impl<const PACKET_BUFFER_SIZE: usize, E> Host for EmbassyHost<PACKET_BUFFER_SIZE, E>
where
    E: FnMut() -> EngineCycleEntropySeed,
{
    async fn wait(&mut self, wake: NextScheduledEngineWork) -> CycleStamp {
        self.batch.clear();

        // Suspend until the engine's next deadline or a stamped packet. This is
        // the genuine `.await`: the embassy executor sleeps the core here.
        match wake {
            NextScheduledEngineWork::Immediate => {}
            NextScheduledEngineWork::At(deadline) => {
                let at = EmbassyInstant::from_millis(deadline.0);
                if let Either::First(entry) = select(self.inbound.receive(), Timer::at(at)).await {
                    let _ = self.batch.push(entry);
                }
            }
            NextScheduledEngineWork::Idle => {
                let entry = self.inbound.receive().await;
                let _ = self.batch.push(entry);
            }
        }
        // Drain whatever else is queued, up to the mailbox depth.
        while !self.batch.is_full() {
            match self.inbound.try_receive() {
                Ok(entry) => {
                    let _ = self.batch.push(entry);
                }
                Err(_) => break,
            }
        }

        CycleStamp {
            now: InstantMillis(EmbassyInstant::now().as_millis()),
            seed: (self.draw_entropy)(),
        }
    }

    fn inbound_packets(&self) -> impl Iterator<Item = InboundPacket<'_>> {
        self.batch.iter().map(|entry| InboundPacket {
            arrived_at: entry.arrived_at,
            source_interface: entry.source,
            bytes: &entry.bytes,
        })
    }
}
