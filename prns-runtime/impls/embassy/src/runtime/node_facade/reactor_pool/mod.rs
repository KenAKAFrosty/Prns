use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use heapless::Vec as HeaplessVec;
use portable_atomic::{AtomicBool, Ordering};
use static_cell::{ConstStaticCell, StaticCell};

use crate::engine::IssuedCommand;
use crate::interfaces::InterfaceId;
use crate::reactor::driver::{
    embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyInterfaceSeam,
    InterfaceLifecycle, PooledEgress,
};
use crate::reactor::grant::FrameSlot;

use super::command_handle::PrnsNodeHandle;
use super::interface_lifecycle::{Fleet, FleetWire};
use super::node_lifecycle::ReactorPlumbing;

const UNCLAIMED_LANE_ID: InterfaceId = InterfaceId::new([0xff; 8]);

type LaneBuffer<const FRAME: usize, const DEPTH: usize> = [FrameSlot<FRAME>; DEPTH];
type LaneChannel<M, const FRAME: usize> = zerocopy_channel::Channel<'static, M, FrameSlot<FRAME>>;

#[derive(Debug, PartialEq, Eq)]
pub enum ReactorPoolError {
    AlreadyTaken,
    StorageUnavailable,
    LaneAlreadyTaken { slot: usize },
}

pub struct StaticReactorPool<
    M: RawMutex + 'static,
    const FRAME: usize,
    const DEPTH: usize,
    const LANES: usize,
> {
    taken: AtomicBool,
    inbound_buffers: [ConstStaticCell<LaneBuffer<FRAME, DEPTH>>; LANES],
    inbound_channels: [StaticCell<LaneChannel<M, FRAME>>; LANES],
    outbound_buffers: [ConstStaticCell<LaneBuffer<FRAME, DEPTH>>; LANES],
    outbound_channels: [StaticCell<LaneChannel<M, FRAME>>; LANES],
}

impl<M: RawMutex + 'static, const FRAME: usize, const DEPTH: usize, const LANES: usize>
    StaticReactorPool<M, FRAME, DEPTH, LANES>
{
    #[must_use]
    pub const fn new() -> Self {
        Self {
            taken: AtomicBool::new(false),
            inbound_buffers: [const { ConstStaticCell::new([const { FrameSlot::empty() }; DEPTH]) };
                LANES],
            inbound_channels: [const { StaticCell::new() }; LANES],
            outbound_buffers: [const { ConstStaticCell::new([const { FrameSlot::empty() }; DEPTH]) };
                LANES],
            outbound_channels: [const { StaticCell::new() }; LANES],
        }
    }

    pub fn try_take(&'static self) -> Result<ReactorPool<M, FRAME, LANES>, ReactorPoolError> {
        if self
            .taken
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ReactorPoolError::AlreadyTaken);
        }

        let mut inbound = HeaplessVec::new();
        let mut egress = HeaplessVec::new();
        let mut lanes = core::array::from_fn(|_| None);
        for (slot, lane) in lanes.iter_mut().enumerate() {
            let inbound_buffer = self.inbound_buffers[slot]
                .try_take()
                .ok_or(ReactorPoolError::StorageUnavailable)?;
            let inbound_channel = self.inbound_channels[slot]
                .try_init(zerocopy_channel::Channel::new(inbound_buffer))
                .ok_or(ReactorPoolError::StorageUnavailable)?;
            let (interface_inbound, reactor_inbound) = embassy_grant_lane(inbound_channel);

            let outbound_buffer = self.outbound_buffers[slot]
                .try_take()
                .ok_or(ReactorPoolError::StorageUnavailable)?;
            let outbound_channel = self.outbound_channels[slot]
                .try_init(zerocopy_channel::Channel::new(outbound_buffer))
                .ok_or(ReactorPoolError::StorageUnavailable)?;
            let (reactor_outbound, interface_outbound) = embassy_grant_lane(outbound_channel);

            if inbound.push((UNCLAIMED_LANE_ID, reactor_inbound)).is_err() {
                return Err(ReactorPoolError::StorageUnavailable);
            }
            if egress.push((UNCLAIMED_LANE_ID, reactor_outbound)).is_err() {
                return Err(ReactorPoolError::StorageUnavailable);
            }
            *lane = Some(InterfaceLane {
                inbound: interface_inbound,
                outbound: interface_outbound,
            });
        }

        Ok(ReactorPool {
            inbound,
            egress: PooledEgress::new(egress),
            lanes,
        })
    }
}

impl<M: RawMutex + 'static, const FRAME: usize, const DEPTH: usize, const LANES: usize> Default
    for StaticReactorPool<M, FRAME, DEPTH, LANES>
{
    fn default() -> Self {
        Self::new()
    }
}

pub struct ReactorPool<M: RawMutex + 'static, const FRAME: usize, const LANES: usize> {
    inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, FRAME>), LANES>,
    egress: PooledEgress<M, FRAME, LANES>,
    lanes: [Option<InterfaceLane<M, FRAME>>; LANES],
}

impl<M: RawMutex + 'static, const FRAME: usize, const LANES: usize> ReactorPool<M, FRAME, LANES> {
    pub fn take_interface<const SLOT: usize>(
        &mut self,
    ) -> Result<InterfaceLane<M, FRAME>, ReactorPoolError> {
        const { assert!(SLOT < LANES) };
        self.lanes[SLOT]
            .take()
            .ok_or(ReactorPoolError::LaneAlreadyTaken { slot: SLOT })
    }

    pub fn take_supervisor<const SLOT: usize>(
        &mut self,
        outbound_wake: &'static Signal<M, ()>,
    ) -> Result<SupervisorLane<M, FRAME>, ReactorPoolError> {
        const { assert!(SLOT < LANES) };
        if self.lanes[SLOT].is_none() {
            return Err(ReactorPoolError::LaneAlreadyTaken { slot: SLOT });
        }
        let Some(producer) = self.egress.producer_mut(SLOT) else {
            return Err(ReactorPoolError::StorageUnavailable);
        };
        producer.set_outbound_wake(outbound_wake);
        let lane = self.lanes[SLOT]
            .take()
            .ok_or(ReactorPoolError::StorageUnavailable)?;
        Ok(SupervisorLane {
            lane,
            outbound_wake,
        })
    }

    pub fn into_plumbing<
        const NOTIFY: usize,
        const COMMANDS: usize,
        const LIFECYCLE: usize,
        const COMPLETIONS: usize,
    >(
        self,
        notify: Receiver<'static, M, InterfaceId, NOTIFY>,
        commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
        lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
        handle: PrnsNodeHandle<'static, M, COMMANDS, COMPLETIONS>,
    ) -> ReactorPlumbing<M, FRAME, LANES, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS> {
        ReactorPlumbing::new(
            self.inbound,
            self.egress,
            notify,
            commands,
            lifecycle,
            handle,
        )
    }
}

pub struct InterfaceLane<M: RawMutex + 'static, const FRAME: usize> {
    inbound: EmbassyGrantProducer<'static, M, FRAME>,
    outbound: EmbassyGrantConsumer<'static, M, FRAME>,
}

impl<M: RawMutex + 'static, const FRAME: usize> InterfaceLane<M, FRAME> {
    pub fn into_seam<const NOTIFY: usize>(
        self,
        id: InterfaceId,
        notify: Sender<'static, M, InterfaceId, NOTIFY>,
        fill_entropy: fn(&mut [u8]),
    ) -> EmbassyInterfaceSeam<'static, M, NOTIFY, FRAME> {
        EmbassyInterfaceSeam::new(id, self.inbound, notify, self.outbound, fill_entropy)
    }
}

pub struct SupervisorLane<M: RawMutex + 'static, const FRAME: usize> {
    lane: InterfaceLane<M, FRAME>,
    outbound_wake: &'static Signal<M, ()>,
}

impl<M: RawMutex + 'static, const FRAME: usize> SupervisorLane<M, FRAME> {
    pub fn into_fleet<const NOTIFY: usize, const LIFECYCLE: usize>(
        self,
        notify: Sender<'static, M, InterfaceId, NOTIFY>,
        lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
    ) -> Fleet<M, FRAME, NOTIFY, LIFECYCLE> {
        Fleet::new(
            FleetWire {
                inbound: self.lane.inbound,
                outbound: self.lane.outbound,
                notify,
                outbound_wake: self.outbound_wake,
            },
            lifecycle,
        )
    }
}

#[cfg(test)]
mod tests;
