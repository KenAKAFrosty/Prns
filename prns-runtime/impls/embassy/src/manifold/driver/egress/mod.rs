use heapless::Vec as HeaplessVec;

use crate::engine::{EngineReaction, FanTarget, InstantMillis, Journaled};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{InterfaceDescriptor, InterfaceId, InterfaceKind};
use crate::manifold::announce_pacer::{
    AnnouncePacer, FixedPacerQueue, PacerDelivery, PacerRetryPolicy,
};
use crate::manifold::grant::{FrameTarget, LaneWriteOutcome, ManifoldLaneWriter};
use crate::manifold::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::manifold::kernel::{
    route_reaction as route_engine_reaction, AnnounceDirective, DirectiveEgress,
};

fn lane_serves(lane_key: InterfaceId, target: InterfaceId) -> bool {
    if lane_key == target {
        return true;
    }
    match (lane_key.kind(), target.kind()) {
        (Some(supervisor), Some(child)) => supervisor.member_kind() == Some(child),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum EgressOutcome {
    Enqueued,
    LaneFull {
        lane: InterfaceId,
    },
    FrameTooLarge {
        lane: InterfaceId,
        frame_len: usize,
        capacity: usize,
    },
    NoLane,
}

fn egress_outcome(lane: InterfaceId, outcome: LaneWriteOutcome) -> EgressOutcome {
    match outcome {
        LaneWriteOutcome::Written => EgressOutcome::Enqueued,
        LaneWriteOutcome::Full => EgressOutcome::LaneFull { lane },
        LaneWriteOutcome::FrameTooLarge {
            frame_len,
            capacity,
        } => EgressOutcome::FrameTooLarge {
            lane,
            frame_len,
            capacity,
        },
    }
}

/// Nonblocking direct and fleet egress.
pub trait ManifoldEgress {
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]) -> EgressOutcome;
    fn enqueue_broadcast(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &[u8],
    ) -> EgressOutcome;
    fn lane_for(&self, target: InterfaceId) -> Option<InterfaceId> {
        Some(target)
    }
    fn fleet_lane(&self, _supervisor: InterfaceKind) -> Option<InterfaceId> {
        None
    }
}

/// Fixed-set egress with erased slot sizes, allowing heterogeneous lanes in one borrowed slice without allocation.
pub struct EmbassyEgress<'a> {
    lanes: &'a mut [(InterfaceId, &'a mut dyn ManifoldLaneWriter)],
}

impl<'a> EmbassyEgress<'a> {
    #[must_use]
    pub fn new(lanes: &'a mut [(InterfaceId, &'a mut dyn ManifoldLaneWriter)]) -> Self {
        Self { lanes }
    }
}

impl ManifoldEgress for EmbassyEgress<'_> {
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]) -> EgressOutcome {
        for (id, producer) in self.lanes.iter_mut() {
            if lane_serves(*id, target) {
                return egress_outcome(*id, producer.try_write(FrameTarget::Direct(target), bytes));
            }
        }
        EgressOutcome::NoLane
    }

    fn enqueue_broadcast(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &[u8],
    ) -> EgressOutcome {
        for (id, producer) in self.lanes.iter_mut() {
            if id.kind() == Some(supervisor) {
                return egress_outcome(*id, producer.try_write(FrameTarget::Fan(fan), bytes));
            }
        }
        EgressOutcome::NoLane
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
const EMBASSY_ANNOUNCE_RETRY_POLICY: PacerRetryPolicy = PacerRetryPolicy::new(250, 5_000);

pub(super) struct InterfacePacer {
    pub(super) id: InterfaceId,
    pacer: AnnouncePacer<FixedPacerQueue<PACER_DEPTH, FrameTarget>, FrameTarget>,
}

impl InterfacePacer {
    pub(super) fn from_descriptor(id: InterfaceId, descriptor: &InterfaceDescriptor) -> Self {
        Self {
            id,
            pacer: AnnouncePacer::new(
                descriptor.announce_bandwidth_cap,
                descriptor.bitrate,
                EMBASSY_ANNOUNCE_RETRY_POLICY,
            ),
        }
    }
}

pub(super) fn route_reaction(
    reaction: EngineReaction<'_>,
    egress: &mut impl ManifoldEgress,
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

impl<E: ManifoldEgress> EmbassyDirectiveEgress<'_, E> {
    fn offer_to_fleet_pacer(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &[u8],
        hops: u8,
    ) {
        let Some(lane) = self.egress.fleet_lane(supervisor) else {
            enqueue_broadcast_for_wire(self.egress, self.ifacs, supervisor, fan, bytes);
            return;
        };
        match self.pacers.iter_mut().find(|entry| entry.id == lane) {
            Some(entry) => {
                let _ = entry.pacer.offer_tagged(
                    bytes,
                    hops,
                    self.now,
                    FrameTarget::Fan(fan),
                    |frame, target| {
                        enqueue_paced_for_wire(self.egress, self.ifacs, lane, target, frame)
                    },
                );
            }
            None => {
                enqueue_broadcast_for_wire(self.egress, self.ifacs, supervisor, fan, bytes);
            }
        }
    }
}

impl<E: ManifoldEgress> DirectiveEgress for EmbassyDirectiveEgress<'_, E> {
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
        self.offer_to_fleet_pacer(supervisor, fan, announce.bytes(), announce.hops());
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
    egress: &mut impl ManifoldEgress,
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
    egress: &mut impl ManifoldEgress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
) {
    let _ = attempt_enqueue_for_wire(egress, ifacs, target, bytes);
}

fn attempt_enqueue_for_wire(
    egress: &mut impl ManifoldEgress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
) -> PacerDelivery {
    let lane = egress.lane_for(target).unwrap_or(target);
    let outcome = match ifac_for(ifacs, lane) {
        Some(entry) => {
            let mut wire = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
            let Some(masked_len) = entry.context.mask_outbound(bytes, &mut wire) else {
                return PacerDelivery::Discarded;
            };
            egress.enqueue(target, &wire[..masked_len])
        }
        None => egress.enqueue(target, bytes),
    };
    delivery_for_egress_outcome(outcome)
}

fn delivery_for_egress_outcome(outcome: EgressOutcome) -> PacerDelivery {
    match outcome {
        EgressOutcome::Enqueued => PacerDelivery::Admitted,
        EgressOutcome::LaneFull { .. } => PacerDelivery::Backpressured,
        EgressOutcome::FrameTooLarge { .. } | EgressOutcome::NoLane => PacerDelivery::Discarded,
    }
}

pub(super) fn enqueue_broadcast_for_wire(
    egress: &mut impl ManifoldEgress,
    ifacs: &[InterfaceIfac],
    supervisor: InterfaceKind,
    fan: FanTarget,
    bytes: &[u8],
) {
    let _ = attempt_enqueue_broadcast_for_wire(egress, ifacs, supervisor, fan, bytes);
}

fn attempt_enqueue_broadcast_for_wire(
    egress: &mut impl ManifoldEgress,
    ifacs: &[InterfaceIfac],
    supervisor: InterfaceKind,
    fan: FanTarget,
    bytes: &[u8],
) -> PacerDelivery {
    let outcome = match egress
        .fleet_lane(supervisor)
        .and_then(|lane| ifac_for(ifacs, lane))
    {
        Some(entry) => {
            let mut wire = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
            let Some(masked_len) = entry.context.mask_outbound(bytes, &mut wire) else {
                return PacerDelivery::Discarded;
            };
            egress.enqueue_broadcast(supervisor, fan, &wire[..masked_len])
        }
        None => egress.enqueue_broadcast(supervisor, fan, bytes),
    };
    delivery_for_egress_outcome(outcome)
}

fn offer_to_pacer(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    bytes: &[u8],
    hops: u8,
    now: InstantMillis,
    egress: &mut impl ManifoldEgress,
    ifacs: &[InterfaceIfac],
) {
    let lane = egress.lane_for(target).unwrap_or(target);
    match pacers.iter_mut().find(|entry| entry.id == lane) {
        Some(entry) => {
            let _ = entry.pacer.offer_tagged(
                bytes,
                hops,
                now,
                FrameTarget::Direct(target),
                |frame, target| enqueue_paced_for_wire(egress, ifacs, lane, target, frame),
            );
        }
        None => enqueue_for_wire(egress, ifacs, target, bytes),
    }
}

fn enqueue_paced_for_wire(
    egress: &mut impl ManifoldEgress,
    ifacs: &[InterfaceIfac],
    lane: InterfaceId,
    target: FrameTarget,
    bytes: &[u8],
) -> PacerDelivery {
    match target {
        FrameTarget::Direct(target) => attempt_enqueue_for_wire(egress, ifacs, target, bytes),
        FrameTarget::Fan(fan) => {
            let Some(supervisor) = lane.kind() else {
                return PacerDelivery::Discarded;
            };
            attempt_enqueue_broadcast_for_wire(egress, ifacs, supervisor, fan, bytes)
        }
    }
}

pub(super) fn flush_due_pacers(
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    egress: &mut impl ManifoldEgress,
    ifacs: &[InterfaceIfac],
) {
    for entry in pacers.iter_mut() {
        let lane = entry.id;
        let _ = entry.pacer.release_due_tagged(now, |frame, target| {
            enqueue_paced_for_wire(egress, ifacs, lane, target, frame)
        });
    }
}

pub(super) fn soonest_pacer_release(pacers: &[InterfacePacer]) -> Option<InstantMillis> {
    pacers
        .iter()
        .filter_map(|entry| entry.pacer.next_release())
        .min_by_key(|deadline| deadline.0)
}

pub struct PooledEgress<const LANE_COUNT: usize> {
    pub(crate) lanes: HeaplessVec<(InterfaceId, &'static mut dyn ManifoldLaneWriter), LANE_COUNT>,
}

impl<const LANE_COUNT: usize> PooledEgress<LANE_COUNT> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            lanes: HeaplessVec::new(),
        }
    }

    pub(crate) fn push(
        &mut self,
        id: InterfaceId,
        producer: &'static mut dyn ManifoldLaneWriter,
    ) -> Result<(), &'static mut dyn ManifoldLaneWriter> {
        self.lanes
            .push((id, producer))
            .map_err(|(_, producer)| producer)
    }

    pub(crate) fn retag(&mut self, old_id: InterfaceId, new_id: InterfaceId) {
        for (id, _) in self.lanes.iter_mut() {
            if *id == old_id {
                *id = new_id;
            }
        }
    }
}

impl<const LANE_COUNT: usize> ManifoldEgress for PooledEgress<LANE_COUNT> {
    fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]) -> EgressOutcome {
        for (id, producer) in self.lanes.iter_mut() {
            if lane_serves(*id, target) {
                return egress_outcome(*id, producer.try_write(FrameTarget::Direct(target), bytes));
            }
        }
        EgressOutcome::NoLane
    }

    fn enqueue_broadcast(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &[u8],
    ) -> EgressOutcome {
        for (id, producer) in self.lanes.iter_mut() {
            if id.kind() == Some(supervisor) {
                return egress_outcome(*id, producer.try_write(FrameTarget::Fan(fan), bytes));
            }
        }
        EgressOutcome::NoLane
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

impl<const LANE_COUNT: usize> Default for PooledEgress<LANE_COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
