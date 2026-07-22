use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use heapless::Vec as HeaplessVec;
use portable_atomic::{AtomicBool, Ordering};
use static_cell::{ConstStaticCell, StaticCell};

use crate::engine::IssuedCommand;
use crate::interfaces::{IfacContext, InterfaceDescriptor, InterfaceId, InterfaceIfac};
use crate::reactor::driver::{
    embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyInterfaceSeam,
    InterfaceLifecycle, PooledEgress,
};
use crate::reactor::grant::FrameSlot;

use super::command_handle::PrnsNodeHandle;
use super::interface_lifecycle::{Fleet, FleetWire};
use super::node_lifecycle::ReactorWiring;

const UNCLAIMED_LANE_ID: InterfaceId = InterfaceId::new([0xff; 8]);

#[must_use]
pub const fn minimum_reactor_notification_capacity(lane_count: usize, lane_depth: usize) -> usize {
    assert!(lane_count > 0);
    assert!(lane_depth > 0);
    lane_count
        .checked_mul(lane_depth)
        .expect("reactor notification capacity overflow")
}

type LaneBuffer<const FRAME: usize, const DEPTH: usize> = [FrameSlot<FRAME>; DEPTH];
type LaneChannel<M, const FRAME: usize> = zerocopy_channel::Channel<'static, M, FrameSlot<FRAME>>;

#[derive(Debug, PartialEq, Eq)]
pub enum PoolTakeError {
    AlreadyTaken,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LaneClaimError {
    AlreadyClaimed { slot: usize },
}

pub struct StaticReactorPool<
    M: RawMutex + 'static,
    const FRAME: usize,
    const DEPTH: usize,
    const LANE_COUNT: usize,
> {
    taken: AtomicBool,
    inbound_buffers: [ConstStaticCell<LaneBuffer<FRAME, DEPTH>>; LANE_COUNT],
    inbound_channels: [StaticCell<LaneChannel<M, FRAME>>; LANE_COUNT],
    outbound_buffers: [ConstStaticCell<LaneBuffer<FRAME, DEPTH>>; LANE_COUNT],
    outbound_channels: [StaticCell<LaneChannel<M, FRAME>>; LANE_COUNT],
}

impl<M: RawMutex + 'static, const FRAME: usize, const DEPTH: usize, const LANE_COUNT: usize>
    StaticReactorPool<M, FRAME, DEPTH, LANE_COUNT>
{
    #[must_use]
    pub const fn new() -> Self {
        Self {
            taken: AtomicBool::new(false),
            inbound_buffers: [const { ConstStaticCell::new([const { FrameSlot::empty() }; DEPTH]) };
                LANE_COUNT],
            inbound_channels: [const { StaticCell::new() }; LANE_COUNT],
            outbound_buffers: [const { ConstStaticCell::new([const { FrameSlot::empty() }; DEPTH]) };
                LANE_COUNT],
            outbound_channels: [const { StaticCell::new() }; LANE_COUNT],
        }
    }

    pub fn try_take(
        &'static self,
    ) -> Result<ReactorPool<M, FRAME, DEPTH, LANE_COUNT>, PoolTakeError> {
        if self
            .taken
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(PoolTakeError::AlreadyTaken);
        }

        let mut inbound = HeaplessVec::new();
        let mut egress = HeaplessVec::new();
        let mut lanes = core::array::from_fn(|_| None);
        for (slot, lane) in lanes.iter_mut().enumerate() {
            let inbound_buffer = self.inbound_buffers[slot].take();
            let inbound_channel =
                self.inbound_channels[slot].init(zerocopy_channel::Channel::new(inbound_buffer));
            let (interface_inbound, reactor_inbound) = embassy_grant_lane(inbound_channel);

            let outbound_buffer = self.outbound_buffers[slot].take();
            let outbound_channel =
                self.outbound_channels[slot].init(zerocopy_channel::Channel::new(outbound_buffer));
            let (reactor_outbound, interface_outbound) = embassy_grant_lane(outbound_channel);

            assert!(inbound.push((UNCLAIMED_LANE_ID, reactor_inbound)).is_ok());
            assert!(egress.push((UNCLAIMED_LANE_ID, reactor_outbound)).is_ok());
            *lane = Some(InterfaceLane {
                id: UNCLAIMED_LANE_ID,
                inbound: interface_inbound,
                outbound: interface_outbound,
            });
        }

        Ok(ReactorPool {
            inbound,
            egress: PooledEgress::new(egress),
            lanes,
            initial: HeaplessVec::new(),
            ifacs: HeaplessVec::new(),
        })
    }
}

impl<M: RawMutex + 'static, const FRAME: usize, const DEPTH: usize, const LANE_COUNT: usize> Default
    for StaticReactorPool<M, FRAME, DEPTH, LANE_COUNT>
{
    fn default() -> Self {
        Self::new()
    }
}

pub struct ReactorPool<
    M: RawMutex + 'static,
    const FRAME: usize,
    const DEPTH: usize,
    const LANE_COUNT: usize,
> {
    inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, FRAME>), LANE_COUNT>,
    egress: PooledEgress<M, FRAME, LANE_COUNT>,
    lanes: [Option<InterfaceLane<M, FRAME>>; LANE_COUNT],
    initial: HeaplessVec<InterfaceDescriptor, LANE_COUNT>,
    ifacs: HeaplessVec<InterfaceIfac, LANE_COUNT>,
}

impl<M: RawMutex + 'static, const FRAME: usize, const DEPTH: usize, const LANE_COUNT: usize>
    ReactorPool<M, FRAME, DEPTH, LANE_COUNT>
{
    pub fn claim_interface<const SLOT: usize>(
        &mut self,
        descriptor: InterfaceDescriptor,
    ) -> Result<InterfaceLane<M, FRAME>, LaneClaimError> {
        let lane = self.claim::<SLOT>(descriptor.id)?;
        if self.initial.push(descriptor).is_err() {
            unreachable!()
        }
        Ok(lane)
    }

    pub fn claim_interface_with_ifac<const SLOT: usize>(
        &mut self,
        descriptor: InterfaceDescriptor,
        context: IfacContext,
    ) -> Result<InterfaceLane<M, FRAME>, LaneClaimError> {
        let lane = self.claim::<SLOT>(descriptor.id)?;
        if self.initial.push(descriptor).is_err() {
            unreachable!()
        }
        if self
            .ifacs
            .push(InterfaceIfac {
                id: descriptor.id,
                context,
            })
            .is_err()
        {
            unreachable!()
        }
        Ok(lane)
    }

    pub fn claim_supervisor<const SLOT: usize>(
        &mut self,
        supervisor: InterfaceId,
        outbound_wake: &'static Signal<M, ()>,
    ) -> Result<SupervisorLane<M, FRAME>, LaneClaimError> {
        self.claim_supervisor_configuration::<SLOT>(supervisor, outbound_wake)
    }

    pub fn claim_supervisor_with_ifac<const SLOT: usize>(
        &mut self,
        supervisor: InterfaceId,
        context: IfacContext,
        outbound_wake: &'static Signal<M, ()>,
    ) -> Result<SupervisorLane<M, FRAME>, LaneClaimError> {
        let lane = self.claim_supervisor_configuration::<SLOT>(supervisor, outbound_wake)?;
        if self
            .ifacs
            .push(InterfaceIfac {
                id: supervisor,
                context,
            })
            .is_err()
        {
            unreachable!()
        }
        Ok(lane)
    }

    fn claim_supervisor_configuration<const SLOT: usize>(
        &mut self,
        supervisor: InterfaceId,
        outbound_wake: &'static Signal<M, ()>,
    ) -> Result<SupervisorLane<M, FRAME>, LaneClaimError> {
        const { assert!(SLOT < LANE_COUNT) };
        if self.lanes[SLOT].is_none() {
            return Err(LaneClaimError::AlreadyClaimed { slot: SLOT });
        }
        let Some(producer) = self.egress.producer_mut(SLOT) else {
            unreachable!()
        };
        producer.set_outbound_wake(outbound_wake);
        let lane = self.claim::<SLOT>(supervisor)?;
        Ok(SupervisorLane {
            lane,
            outbound_wake,
        })
    }

    fn claim<const SLOT: usize>(
        &mut self,
        id: InterfaceId,
    ) -> Result<InterfaceLane<M, FRAME>, LaneClaimError> {
        const { assert!(SLOT < LANE_COUNT) };
        let Some(mut lane) = self.lanes[SLOT].take() else {
            return Err(LaneClaimError::AlreadyClaimed { slot: SLOT });
        };
        lane.id = id;
        self.inbound[SLOT].0 = id;
        self.egress.activate(SLOT, id);
        Ok(lane)
    }

    pub fn into_reactor_wiring<
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
    ) -> ReactorWiring<M, FRAME, LANE_COUNT, NOTIFY, COMMANDS, LIFECYCLE, COMPLETIONS> {
        const {
            assert!(
                NOTIFY >= minimum_reactor_notification_capacity(LANE_COUNT, DEPTH),
                "reactor notification capacity must cover every buffered inbound frame"
            )
        };
        ReactorWiring::new(
            self.inbound,
            self.egress,
            self.initial,
            self.ifacs,
            notify,
            commands,
            lifecycle,
            handle,
        )
    }
}

pub struct InterfaceLane<M: RawMutex + 'static, const FRAME: usize> {
    id: InterfaceId,
    inbound: EmbassyGrantProducer<'static, M, FRAME>,
    outbound: EmbassyGrantConsumer<'static, M, FRAME>,
}

impl<M: RawMutex + 'static, const FRAME: usize> InterfaceLane<M, FRAME> {
    pub fn into_seam<const NOTIFY: usize>(
        self,
        notify: Sender<'static, M, InterfaceId, NOTIFY>,
        fill_entropy: fn(&mut [u8]),
    ) -> EmbassyInterfaceSeam<'static, M, NOTIFY, FRAME> {
        EmbassyInterfaceSeam::new(self.id, self.inbound, notify, self.outbound, fill_entropy)
    }
}

pub struct SupervisorLane<M: RawMutex + 'static, const FRAME: usize> {
    lane: InterfaceLane<M, FRAME>,
    outbound_wake: &'static Signal<M, ()>,
}

impl<M: RawMutex + 'static, const FRAME: usize> SupervisorLane<M, FRAME> {
    pub fn into_fleet<
        const OUTBOUND_CAPACITY: usize,
        const NOTIFY: usize,
        const LIFECYCLE: usize,
    >(
        self,
        notify: Sender<'static, M, InterfaceId, NOTIFY>,
        lifecycle: Sender<'static, M, InterfaceLifecycle, LIFECYCLE>,
    ) -> Fleet<M, FRAME, OUTBOUND_CAPACITY, NOTIFY, LIFECYCLE> {
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
