use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use portable_atomic::{AtomicU32, Ordering};

use crate::interfaces::PacketPhyStats;
use crate::manifold::grant::{
    FrameSlot, FrameTarget, GrantConsumer, GrantProducer, LaneWriteOutcome, ManifoldLaneReader,
    ManifoldLaneWriter,
};

/// Splits caller-owned storage into a zero-copy frame lane.
pub fn embassy_grant_lane<'a, M: RawMutex, const FRAME: usize>(
    channel: &'a mut zerocopy_channel::Channel<'a, M, FrameSlot<FRAME>>,
) -> (
    EmbassyGrantProducer<'a, M, FRAME>,
    EmbassyGrantConsumer<'a, M, FRAME>,
) {
    let (sender, receiver) = channel.split();
    (
        EmbassyGrantProducer {
            sender,
            granted: false,
            wake: None,
            pressure_events: None,
        },
        EmbassyGrantConsumer {
            receiver,
            peeked: false,
        },
    )
}

pub struct EmbassyGrantProducer<'a, M: RawMutex, const FRAME: usize> {
    sender: zerocopy_channel::Sender<'a, M, FrameSlot<FRAME>>,
    granted: bool,
    wake: Option<&'a Signal<M, ()>>,
    pressure_events: Option<&'a AtomicU32>,
}

impl<'a, M: RawMutex, const FRAME: usize> EmbassyGrantProducer<'a, M, FRAME> {
    pub fn set_outbound_wake(&mut self, wake: &'a Signal<M, ()>) {
        self.wake = Some(wake);
    }

    pub fn set_pressure_counter(&mut self, pressure_events: &'a AtomicU32) {
        self.pressure_events = Some(pressure_events);
    }

    pub(crate) fn note_pressure(&self) {
        if let Some(pressure_events) = self.pressure_events {
            let _ = pressure_events.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_add(1))
            });
        }
    }
}

impl<M: RawMutex, const FRAME: usize> GrantProducer<FRAME> for EmbassyGrantProducer<'_, M, FRAME> {
    fn try_grant(&mut self) -> Option<&mut FrameSlot<FRAME>> {
        let granted = &mut self.granted;
        let slot = self.sender.try_send()?;
        *granted = true;
        Some(slot)
    }

    async fn grant(&mut self) -> &mut FrameSlot<FRAME> {
        let granted = &mut self.granted;
        let slot = self.sender.send().await;
        *granted = true;
        slot
    }

    fn commit(&mut self) {
        if self.granted {
            self.granted = false;
            self.sender.send_done();
            if let Some(wake) = self.wake {
                wake.signal(());
            }
        }
    }
}

impl<M: RawMutex + Sync, const FRAME: usize> ManifoldLaneWriter
    for EmbassyGrantProducer<'_, M, FRAME>
{
    fn try_write(&mut self, target: FrameTarget, frame: &[u8]) -> LaneWriteOutcome {
        if frame.len() > FRAME {
            return LaneWriteOutcome::FrameTooLarge {
                frame_len: frame.len(),
                capacity: FRAME,
            };
        }
        let Some(slot) = GrantProducer::try_grant(self) else {
            self.note_pressure();
            return LaneWriteOutcome::Full;
        };
        match target {
            FrameTarget::Direct(interface_id) => slot.fill_for(interface_id, frame),
            FrameTarget::Fan(fan) => slot.fill_for_fan(fan, frame),
        }
        GrantProducer::commit(self);
        LaneWriteOutcome::Written
    }
}

pub struct EmbassyGrantConsumer<'a, M: RawMutex, const FRAME: usize> {
    receiver: zerocopy_channel::Receiver<'a, M, FrameSlot<FRAME>>,
    peeked: bool,
}

impl<M: RawMutex, const FRAME: usize> GrantConsumer<FRAME> for EmbassyGrantConsumer<'_, M, FRAME> {
    fn try_peek(&mut self) -> Option<&mut FrameSlot<FRAME>> {
        let peeked = &mut self.peeked;
        let slot = self.receiver.try_receive()?;
        *peeked = true;
        Some(slot)
    }

    async fn peek(&mut self) -> &mut FrameSlot<FRAME> {
        let peeked = &mut self.peeked;
        let slot = self.receiver.receive().await;
        *peeked = true;
        slot
    }

    fn release(&mut self) {
        if self.peeked {
            self.peeked = false;
            self.receiver.receive_done();
        }
    }
}

impl<M: RawMutex + Sync, const FRAME: usize> ManifoldLaneReader
    for EmbassyGrantConsumer<'_, M, FRAME>
{
    fn try_read(&mut self) -> Option<(FrameTarget, PacketPhyStats, &mut [u8])> {
        let slot = GrantConsumer::try_peek(self)?;
        let target = slot.target;
        let packet_phy = slot.packet_phy;
        Some((target, packet_phy, slot.frame_mut()))
    }

    fn release(&mut self) {
        GrantConsumer::release(self);
    }
}

/// A heap-backed grant lane for host-side tests of Embassy interfaces.
#[cfg(any(test, feature = "std"))]
pub fn leaked_grant_lane<const FRAME: usize>(
    depth: usize,
) -> (
    EmbassyGrantProducer<
        'static,
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        FRAME,
    >,
    EmbassyGrantConsumer<
        'static,
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        FRAME,
    >,
) {
    let slots: std::vec::Vec<FrameSlot<FRAME>> = (0..depth).map(|_| FrameSlot::empty()).collect();
    let channel = std::boxed::Box::leak(std::boxed::Box::new(zerocopy_channel::Channel::new(
        std::boxed::Box::leak(slots.into_boxed_slice()),
    )));
    embassy_grant_lane(channel)
}
