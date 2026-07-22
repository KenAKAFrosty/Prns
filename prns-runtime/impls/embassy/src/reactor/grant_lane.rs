use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;

use crate::engine::FanTarget;
use crate::interfaces::{InterfaceId, PacketPhyStats};
use crate::reactor::grant::{
    AnyGrantConsumer, AnyGrantProducer, FrameSlot, FrameTarget, GrantConsumer, GrantProducer,
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
}

impl<'a, M: RawMutex, const FRAME: usize> EmbassyGrantProducer<'a, M, FRAME> {
    pub fn set_outbound_wake(&mut self, wake: &'a Signal<M, ()>) {
        self.wake = Some(wake);
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

impl<M: RawMutex, const FRAME: usize> AnyGrantProducer for EmbassyGrantProducer<'_, M, FRAME> {
    fn try_fill_frame_for(&mut self, interface_id: InterfaceId, frame: &[u8]) -> bool {
        if frame.len() > FRAME {
            return false;
        }
        let Some(slot) = GrantProducer::try_grant(self) else {
            return false;
        };
        slot.fill_for(interface_id, frame);
        GrantProducer::commit(self);
        true
    }

    fn try_fill_frame_fan(&mut self, fan: FanTarget, frame: &[u8]) -> bool {
        if frame.len() > FRAME {
            return false;
        }
        let Some(slot) = GrantProducer::try_grant(self) else {
            return false;
        };
        slot.fill_for_fan(fan, frame);
        GrantProducer::commit(self);
        true
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

impl<M: RawMutex, const FRAME: usize> AnyGrantConsumer for EmbassyGrantConsumer<'_, M, FRAME> {
    fn try_peek_frame(&mut self) -> Option<(FrameTarget, PacketPhyStats, &mut [u8])> {
        let slot = GrantConsumer::try_peek(self)?;
        let target = slot.target;
        let packet_phy = slot.packet_phy;
        Some((target, packet_phy, slot.frame_mut()))
    }

    fn release_frame(&mut self) {
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
