use embassy_sync::blocking_mutex::raw::RawMutex;
use heapless::Vec as HeaplessVec;

use crate::engine::{EngineReaction, FanTarget, InstantMillis, Journaled};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{InterfaceDescriptor, InterfaceId, InterfaceKind};
use crate::reactor::announce_pacer::{AnnouncePacer, FixedPacerQueue};
use crate::reactor::grant::AnyGrantProducer;
use crate::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::reactor::kernel::{
    route_reaction as route_engine_reaction, AnnounceDirective, DirectiveEgress,
};

use super::EmbassyGrantProducer;

fn lane_serves(lane_key: InterfaceId, target: InterfaceId) -> bool {
    if lane_key == target {
        return true;
    }
    match (lane_key.kind(), target.kind()) {
        (Some(supervisor), Some(child)) => supervisor.member_kind() == Some(child),
        _ => false,
    }
}

/// Nonblocking direct and fleet egress.
pub trait ReactorEgress {
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]);
    fn enqueue_broadcast(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]);
    fn lane_for(&self, target: InterfaceId) -> Option<InterfaceId> {
        Some(target)
    }
    fn fleet_lane(&self, _supervisor: InterfaceKind) -> Option<InterfaceId> {
        None
    }
}

/// Fixed-set egress with erased slot sizes, allowing heterogeneous lanes in one borrowed slice without allocation.
pub struct EmbassyEgress<'a> {
    lanes: &'a mut [(InterfaceId, &'a mut dyn AnyGrantProducer)],
}

impl<'a> EmbassyEgress<'a> {
    #[must_use]
    pub fn new(lanes: &'a mut [(InterfaceId, &'a mut dyn AnyGrantProducer)]) -> Self {
        Self { lanes }
    }
}

impl ReactorEgress for EmbassyEgress<'_> {
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]) {
        for (id, producer) in self.lanes.iter_mut() {
            if lane_serves(*id, target) {
                let _ = producer.try_fill_frame_for(target, bytes);
                return;
            }
        }
    }

    fn enqueue_broadcast(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]) {
        for (id, producer) in self.lanes.iter_mut() {
            if id.kind() == Some(supervisor) {
                let _ = producer.try_fill_frame_fan(fan, bytes);
                return;
            }
        }
    }

    fn lane_for(&self, target: InterfaceId) -> Option<InterfaceId> {
        self.lanes
            .iter()
            .map(|(id, _)| *id)
            .find(|id| lane_serves(*id, target))
    }

    fn fleet_lane(&self, supervisor: InterfaceKind) -> Option<InterfaceId> {
        self.lanes
            .iter()
            .map(|(id, _)| *id)
            .find(|id| id.kind() == Some(supervisor))
    }
}

pub(super) const MAX_PACED_INTERFACES: usize = 2;
const PACER_DEPTH: usize = 2;

pub(super) struct InterfacePacer {
    pub(super) id: InterfaceId,
    pacer: AnnouncePacer<FixedPacerQueue<PACER_DEPTH>>,
}

impl InterfacePacer {
    pub(super) fn from_descriptor(id: InterfaceId, descriptor: &InterfaceDescriptor) -> Self {
        Self {
            id,
            pacer: AnnouncePacer::new(descriptor.announce_bandwidth_cap, descriptor.bitrate),
        }
    }
}

pub(super) fn route_reaction(
    reaction: EngineReaction<'_>,
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    app: &mut impl FnMut(Journaled<'_>),
) {
    let mut directive_egress = EmbassyDirectiveEgress {
        egress,
        ifacs,
        pacers,
        now,
    };
    route_engine_reaction(reaction, &mut directive_egress, app);
}

struct EmbassyDirectiveEgress<'a, E> {
    egress: &'a mut E,
    ifacs: &'a [InterfaceIfac],
    pacers: &'a mut [InterfacePacer],
    now: InstantMillis,
}

impl<E: ReactorEgress> DirectiveEgress for EmbassyDirectiveEgress<'_, E> {
    fn send(&mut self, target: InterfaceId, bytes: &[u8]) {
        enqueue_for_wire(self.egress, self.ifacs, target, bytes);
    }

    fn send_announce(&mut self, target: InterfaceId, announce: AnnounceDirective<'_>) {
        offer_to_pacer(
            self.pacers,
            target,
            announce.bytes(),
            announce.hops(),
            self.now,
            self.egress,
            self.ifacs,
        );
    }

    fn send_to_fleet(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]) {
        enqueue_broadcast_for_wire(self.egress, self.ifacs, supervisor, fan, bytes);
    }

    fn send_announce_to_fleet(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        announce: AnnounceDirective<'_>,
    ) {
        enqueue_broadcast_for_wire(self.egress, self.ifacs, supervisor, fan, announce.bytes());
    }

    fn emit_frame(
        &mut self,
        target: InterfaceId,
        _size_hint: usize,
        fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
    ) {
        emit_for_wire(self.egress, self.ifacs, target, fill);
    }
}

/// Erased slot sizes require one bounded stack buffer before the frame enters its lane. `fill` runs exactly once even when the lane is full.
fn emit_for_wire(
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
) {
    let mut frame = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
    if let Some(len) = fill(&mut frame) {
        enqueue_for_wire(egress, ifacs, target, &frame[..len]);
    }
}

pub(super) fn ifac_for(ifacs: &[InterfaceIfac], id: InterfaceId) -> Option<&InterfaceIfac> {
    if ifacs.is_empty() {
        return None;
    }
    ifacs.iter().find(|entry| entry.id == id)
}

pub(super) fn enqueue_for_wire(
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
) {
    let lane = egress.lane_for(target).unwrap_or(target);
    match ifac_for(ifacs, lane) {
        Some(entry) => {
            let mut wire = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
            if let Some(masked_len) = entry.context.mask_outbound(bytes, &mut wire) {
                egress.enqueue(target, &wire[..masked_len]);
            }
        }
        None => egress.enqueue(target, bytes),
    }
}

pub(super) fn enqueue_broadcast_for_wire(
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
    supervisor: InterfaceKind,
    fan: FanTarget,
    bytes: &[u8],
) {
    match egress
        .fleet_lane(supervisor)
        .and_then(|lane| ifac_for(ifacs, lane))
    {
        Some(entry) => {
            let mut wire = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
            if let Some(masked_len) = entry.context.mask_outbound(bytes, &mut wire) {
                egress.enqueue_broadcast(supervisor, fan, &wire[..masked_len]);
            }
        }
        None => egress.enqueue_broadcast(supervisor, fan, bytes),
    }
}

/// Only exact lane owners receive a pacer; fleet members share the supervisor's lane and pacing budget.
pub(super) fn owns_dedicated_lane<C>(lanes: &[(InterfaceId, C)], id: InterfaceId) -> bool {
    lanes.iter().any(|(lane_id, _)| *lane_id == id)
}

fn offer_to_pacer(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    bytes: &[u8],
    hops: u8,
    now: InstantMillis,
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
) {
    match pacers.iter_mut().find(|entry| entry.id == target) {
        Some(entry) => {
            entry.pacer.offer(bytes, hops, now, |frame| {
                enqueue_for_wire(egress, ifacs, target, frame)
            });
        }
        None => enqueue_for_wire(egress, ifacs, target, bytes),
    }
}

pub(super) fn flush_due_pacers(
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    egress: &mut impl ReactorEgress,
    ifacs: &[InterfaceIfac],
) {
    for entry in pacers.iter_mut() {
        let target = entry.id;
        entry
            .pacer
            .release_due(now, |frame| enqueue_for_wire(egress, ifacs, target, frame));
    }
}

pub(super) fn soonest_pacer_release(pacers: &[InterfacePacer]) -> Option<InstantMillis> {
    pacers
        .iter()
        .filter_map(|entry| entry.pacer.next_release())
        .min_by_key(|deadline| deadline.0)
}

/// Dynamic egress over a fixed, uniformly sized lane pool tagged by interface or fleet supervisor.
pub struct PooledEgress<M: RawMutex + 'static, const SLOT: usize, const N: usize> {
    pub(super) lanes: HeaplessVec<(InterfaceId, EmbassyGrantProducer<'static, M, SLOT>), N>,
}

impl<M: RawMutex + 'static, const SLOT: usize, const N: usize> PooledEgress<M, SLOT, N> {
    #[must_use]
    pub fn new(
        lanes: HeaplessVec<(InterfaceId, EmbassyGrantProducer<'static, M, SLOT>), N>,
    ) -> Self {
        Self { lanes }
    }

    pub(crate) fn activate(&mut self, slot: usize, id: InterfaceId) {
        if let Some(entry) = self.lanes.get_mut(slot) {
            entry.0 = id;
        }
    }

    pub(crate) fn has_slot(&self, slot: usize) -> bool {
        slot < self.lanes.len()
    }

    pub(crate) fn retag(&mut self, old_id: InterfaceId, new_id: InterfaceId) {
        for (id, _) in self.lanes.iter_mut() {
            if *id == old_id {
                *id = new_id;
            }
        }
    }
}

impl<M: RawMutex + 'static, const SLOT: usize, const N: usize> ReactorEgress
    for PooledEgress<M, SLOT, N>
{
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]) {
        for (id, producer) in self.lanes.iter_mut() {
            if lane_serves(*id, target) {
                let _ = producer.try_fill_frame_for(target, bytes);
                return;
            }
        }
    }

    fn enqueue_broadcast(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]) {
        for (id, producer) in self.lanes.iter_mut() {
            if id.kind() == Some(supervisor) {
                let _ = producer.try_fill_frame_fan(fan, bytes);
                return;
            }
        }
    }

    fn lane_for(&self, target: InterfaceId) -> Option<InterfaceId> {
        self.lanes
            .iter()
            .map(|(id, _)| *id)
            .find(|id| lane_serves(*id, target))
    }

    fn fleet_lane(&self, supervisor: InterfaceKind) -> Option<InterfaceId> {
        self.lanes
            .iter()
            .map(|(id, _)| *id)
            .find(|id| id.kind() == Some(supervisor))
    }
}

#[cfg(test)]
mod tests;
