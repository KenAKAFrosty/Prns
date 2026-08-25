use core::marker::PhantomData;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use heapless::Vec as HeaplessVec;
use portable_atomic::{AtomicBool, AtomicU32, Ordering};
use static_cell::{ConstStaticCell, StaticCell};

use crate::engine::IssuedCommand;
use crate::interfaces::{IfacContext, InterfaceDescriptor, InterfaceId, InterfaceIfac};
use crate::manifold::driver::{
    embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyInterfaceSeam,
    InterfaceLifecycle, PooledEgress,
};
use crate::manifold::grant::{FrameSlot, ManifoldLaneReader, ManifoldLaneWriter};
use crate::manifold::interface_seam::EMBEDDED_MAX_LINK_MTU;

use super::command_handle::PrnsNodeHandle;
use super::interface_lifecycle::{Fleet, FleetWire};
use super::node_lifecycle::ManifoldWiring;

#[must_use]
pub const fn minimum_manifold_notification_capacity(lane_count: usize, lane_depth: usize) -> usize {
    assert!(lane_count > 0);
    assert!(lane_depth > 0);
    assert!(lane_count <= usize::MAX / lane_depth);
    lane_count * lane_depth
}

type LaneBuffer<const FRAME: usize, const DEPTH: usize> = [FrameSlot<FRAME>; DEPTH];
type LaneChannel<M, const FRAME: usize> = zerocopy_channel::Channel<'static, M, FrameSlot<FRAME>>;

#[derive(Debug, PartialEq, Eq)]
pub enum LaneClaimError {
    AlreadyClaimed,
    DuplicateInterfaceId { id: InterfaceId },
    LaneSetFull { capacity: usize },
    NotificationCapacityExceeded { required: usize, capacity: usize },
    FrameCapacityExceeded { required: usize, capacity: usize },
    EmptyOutboundBuffer,
}

pub struct StaticManifoldLane<
    M: RawMutex + 'static,
    const FRAME: usize,
    const INBOUND_DEPTH: usize,
    const OUTBOUND_DEPTH: usize = INBOUND_DEPTH,
> {
    taken: AtomicBool,
    ingress_pressure_events: AtomicU32,
    egress_pressure_events: AtomicU32,
    inbound_buffer: ConstStaticCell<LaneBuffer<FRAME, INBOUND_DEPTH>>,
    inbound_channel: StaticCell<LaneChannel<M, FRAME>>,
    manifold_inbound: StaticCell<EmbassyGrantConsumer<'static, M, FRAME>>,
    outbound_buffer: ConstStaticCell<LaneBuffer<FRAME, OUTBOUND_DEPTH>>,
    outbound_channel: StaticCell<LaneChannel<M, FRAME>>,
    manifold_outbound: StaticCell<EmbassyGrantProducer<'static, M, FRAME>>,
}

impl<
        M: RawMutex + Sync + 'static,
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    > StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>
{
    #[must_use]
    pub const fn new() -> Self {
        Self {
            taken: AtomicBool::new(false),
            ingress_pressure_events: AtomicU32::new(0),
            egress_pressure_events: AtomicU32::new(0),
            inbound_buffer: ConstStaticCell::new([const { FrameSlot::empty() }; INBOUND_DEPTH]),
            inbound_channel: StaticCell::new(),
            manifold_inbound: StaticCell::new(),
            outbound_buffer: ConstStaticCell::new([const { FrameSlot::empty() }; OUTBOUND_DEPTH]),
            outbound_channel: StaticCell::new(),
            manifold_outbound: StaticCell::new(),
        }
    }

    fn try_take(
        &'static self,
        id: InterfaceId,
        outbound_wake: Option<&'static Signal<M, ()>>,
        external_outbound: Option<&'static mut [FrameSlot<FRAME>]>,
    ) -> Result<TakenManifoldLane<M, FRAME>, LaneClaimError> {
        if external_outbound
            .as_ref()
            .map_or(OUTBOUND_DEPTH == 0, |buffer| buffer.is_empty())
        {
            return Err(LaneClaimError::EmptyOutboundBuffer);
        }
        if self
            .taken
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(LaneClaimError::AlreadyClaimed);
        }

        let inbound_channel = self
            .inbound_channel
            .init(zerocopy_channel::Channel::new(self.inbound_buffer.take()));
        let (mut interface_inbound, manifold_inbound) = embassy_grant_lane(inbound_channel);
        interface_inbound.set_pressure_counter(&self.ingress_pressure_events);

        let outbound_buffer =
            external_outbound.unwrap_or_else(|| self.outbound_buffer.take().as_mut_slice());
        let outbound_channel = self
            .outbound_channel
            .init(zerocopy_channel::Channel::new(outbound_buffer));
        let (mut manifold_outbound, interface_outbound) = embassy_grant_lane(outbound_channel);
        manifold_outbound.set_pressure_counter(&self.egress_pressure_events);
        if let Some(wake) = outbound_wake {
            manifold_outbound.set_outbound_wake(wake);
        }

        Ok(TakenManifoldLane {
            interface: InterfaceLane {
                id,
                inbound: interface_inbound,
                outbound: interface_outbound,
            },
            manifold_inbound: self.manifold_inbound.init(manifold_inbound),
            manifold_outbound: self.manifold_outbound.init(manifold_outbound),
        })
    }

    #[must_use]
    pub fn ingress_pressure_events(&self) -> u32 {
        self.ingress_pressure_events.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn egress_pressure_events(&self) -> u32 {
        self.egress_pressure_events.load(Ordering::Relaxed)
    }
}

impl<
        M: RawMutex + Sync + 'static,
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    > Default for StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

struct TakenManifoldLane<M: RawMutex + 'static, const FRAME: usize> {
    interface: InterfaceLane<M, FRAME>,
    manifold_inbound: &'static mut EmbassyGrantConsumer<'static, M, FRAME>,
    manifold_outbound: &'static mut EmbassyGrantProducer<'static, M, FRAME>,
}

pub struct ManifoldLaneSet<M: RawMutex + 'static, const LANE_COUNT: usize, const NOTIFY: usize> {
    inbound: HeaplessVec<(InterfaceId, &'static mut dyn ManifoldLaneReader), LANE_COUNT>,
    egress: PooledEgress<LANE_COUNT>,
    initial: HeaplessVec<InterfaceDescriptor, LANE_COUNT>,
    ifacs: HeaplessVec<InterfaceIfac, LANE_COUNT>,
    notification_capacity: usize,
    mutex: PhantomData<M>,
}

impl<M: RawMutex + Sync + 'static, const LANE_COUNT: usize, const NOTIFY: usize>
    ManifoldLaneSet<M, LANE_COUNT, NOTIFY>
{
    #[must_use]
    pub fn new() -> Self {
        Self {
            inbound: HeaplessVec::new(),
            egress: PooledEgress::new(),
            initial: HeaplessVec::new(),
            ifacs: HeaplessVec::new(),
            notification_capacity: 0,
            mutex: PhantomData,
        }
    }

    pub fn claim_interface<
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    >(
        &mut self,
        storage: &'static StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>,
        descriptor: InterfaceDescriptor,
    ) -> Result<InterfaceLane<M, FRAME>, LaneClaimError> {
        self.claim_interface_configuration(storage, descriptor, None, None)
    }

    /// Claims an interface whose outbound frame ring lives in caller-owned static storage.
    pub fn claim_interface_with_outbound_buffer<
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    >(
        &mut self,
        storage: &'static StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>,
        descriptor: InterfaceDescriptor,
        outbound_buffer: &'static mut [FrameSlot<FRAME>],
    ) -> Result<InterfaceLane<M, FRAME>, LaneClaimError> {
        self.claim_interface_configuration(storage, descriptor, None, Some(outbound_buffer))
    }

    pub fn claim_interface_with_ifac<
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    >(
        &mut self,
        storage: &'static StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>,
        descriptor: InterfaceDescriptor,
        context: IfacContext,
    ) -> Result<InterfaceLane<M, FRAME>, LaneClaimError> {
        self.claim_interface_configuration(storage, descriptor, Some(context), None)
    }

    fn claim_interface_configuration<
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    >(
        &mut self,
        storage: &'static StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>,
        mut descriptor: InterfaceDescriptor,
        context: Option<IfacContext>,
        external_outbound: Option<&'static mut [FrameSlot<FRAME>]>,
    ) -> Result<InterfaceLane<M, FRAME>, LaneClaimError> {
        self.validate_claim::<INBOUND_DEPTH>(descriptor.id)?;
        if let Some(mtu) = descriptor.hardware_mtu {
            descriptor.hardware_mtu = Some(mtu.min(EMBEDDED_MAX_LINK_MTU));
        }
        let required = descriptor
            .hardware_mtu
            .unwrap_or(crate::wire::BROADCAST_MTU)
            + context.as_ref().map_or(0, |ifac| ifac.ifac_size().bytes());
        if required > FRAME {
            return Err(LaneClaimError::FrameCapacityExceeded {
                required,
                capacity: FRAME,
            });
        }

        let id = descriptor.id;
        let taken = storage.try_take(id, None, external_outbound)?;
        self.register_lane::<INBOUND_DEPTH>(id, taken.manifold_inbound, taken.manifold_outbound);
        if self.initial.push(descriptor).is_err() {
            unreachable!()
        }
        if let Some(context) = context {
            if self.ifacs.push(InterfaceIfac { id, context }).is_err() {
                unreachable!()
            }
        }
        Ok(taken.interface)
    }

    pub fn claim_supervisor<
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    >(
        &mut self,
        storage: &'static StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>,
        supervisor: InterfaceId,
        outbound_wake: &'static Signal<M, ()>,
    ) -> Result<SupervisorLane<M, FRAME>, LaneClaimError> {
        self.claim_supervisor_configuration(storage, supervisor, None, outbound_wake, None)
    }

    /// Claims a supervisor whose outbound frame ring lives in caller-owned static storage.
    ///
    /// This keeps a large burst queue out of scarce internal RAM on targets with a separately
    /// initialized external-memory allocator. Inbound storage and notification accounting remain
    /// those declared by `INBOUND_DEPTH`.
    pub fn claim_supervisor_with_outbound_buffer<
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    >(
        &mut self,
        storage: &'static StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>,
        supervisor: InterfaceId,
        outbound_wake: &'static Signal<M, ()>,
        outbound_buffer: &'static mut [FrameSlot<FRAME>],
    ) -> Result<SupervisorLane<M, FRAME>, LaneClaimError> {
        self.claim_supervisor_configuration(
            storage,
            supervisor,
            None,
            outbound_wake,
            Some(outbound_buffer),
        )
    }

    pub fn claim_supervisor_with_ifac<
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    >(
        &mut self,
        storage: &'static StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>,
        supervisor: InterfaceId,
        context: IfacContext,
        outbound_wake: &'static Signal<M, ()>,
    ) -> Result<SupervisorLane<M, FRAME>, LaneClaimError> {
        self.claim_supervisor_configuration(storage, supervisor, Some(context), outbound_wake, None)
    }

    fn claim_supervisor_configuration<
        const FRAME: usize,
        const INBOUND_DEPTH: usize,
        const OUTBOUND_DEPTH: usize,
    >(
        &mut self,
        storage: &'static StaticManifoldLane<M, FRAME, INBOUND_DEPTH, OUTBOUND_DEPTH>,
        supervisor: InterfaceId,
        context: Option<IfacContext>,
        outbound_wake: &'static Signal<M, ()>,
        external_outbound: Option<&'static mut [FrameSlot<FRAME>]>,
    ) -> Result<SupervisorLane<M, FRAME>, LaneClaimError> {
        self.validate_claim::<INBOUND_DEPTH>(supervisor)?;
        let taken = storage.try_take(supervisor, Some(outbound_wake), external_outbound)?;
        self.register_lane::<INBOUND_DEPTH>(
            supervisor,
            taken.manifold_inbound,
            taken.manifold_outbound,
        );
        if let Some(context) = context {
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
        }
        Ok(SupervisorLane {
            lane: taken.interface,
            outbound_wake,
        })
    }

    fn validate_claim<const DEPTH: usize>(&self, id: InterfaceId) -> Result<(), LaneClaimError> {
        if self.inbound.iter().any(|(existing, _)| *existing == id) {
            return Err(LaneClaimError::DuplicateInterfaceId { id });
        }
        if self.inbound.len() == LANE_COUNT {
            return Err(LaneClaimError::LaneSetFull {
                capacity: LANE_COUNT,
            });
        }
        let required = self.notification_capacity.saturating_add(DEPTH);
        if required > NOTIFY {
            return Err(LaneClaimError::NotificationCapacityExceeded {
                required,
                capacity: NOTIFY,
            });
        }
        Ok(())
    }

    fn register_lane<const DEPTH: usize>(
        &mut self,
        id: InterfaceId,
        inbound: &'static mut dyn ManifoldLaneReader,
        outbound: &'static mut dyn ManifoldLaneWriter,
    ) {
        if self.inbound.push((id, inbound)).is_err() {
            unreachable!()
        }
        if self.egress.push(id, outbound).is_err() {
            unreachable!()
        }
        self.notification_capacity += DEPTH;
    }

    pub fn into_manifold_wiring<
        const COMMANDS: usize,
        const LIFECYCLE: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    >(
        self,
        notify: Receiver<'static, M, InterfaceId, NOTIFY>,
        commands: Receiver<'static, M, IssuedCommand, COMMANDS>,
        lifecycle: Receiver<'static, M, InterfaceLifecycle, LIFECYCLE>,
        handle: PrnsNodeHandle<
            'static,
            M,
            COMMANDS,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
        >,
    ) -> ManifoldWiring<
        M,
        LANE_COUNT,
        NOTIFY,
        COMMANDS,
        LIFECYCLE,
        COMPLETIONS,
        REQUEST_COMPLETIONS,
        RESPONSE_BYTES,
    > {
        ManifoldWiring {
            inbound: self.inbound,
            egress: self.egress,
            initial: self.initial,
            ifacs: self.ifacs,
            notify,
            commands,
            lifecycle,
            handle,
        }
    }
}

impl<M: RawMutex + Sync + 'static, const LANE_COUNT: usize, const NOTIFY: usize> Default
    for ManifoldLaneSet<M, LANE_COUNT, NOTIFY>
{
    fn default() -> Self {
        Self::new()
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
