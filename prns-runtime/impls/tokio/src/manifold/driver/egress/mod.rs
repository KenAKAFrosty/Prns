#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::{EngineReaction, FanTarget, InstantMillis, Journaled, NoOwedWork};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{ConnectionView, InterfaceDescriptor, InterfaceId, InterfaceKind};
use crate::manifold::announce_pacer::{
    AnnouncePacer, BoundedHeapPacerQueue, PacerDelivery, PacerRetryPolicy,
};
#[cfg(feature = "runtime-metrics")]
use crate::manifold::announce_pacer::{PacerEntry, PacerEvent, PacerOffer};
use crate::manifold::interface_seam::MAX_WIRE_FRAME_LEN;
use crate::manifold::reaction_routing::{
    route_reaction as route_engine_reaction, AnnounceDirective, DirectiveEgress,
};
#[cfg(feature = "runtime-metrics")]
use crate::runtime::{
    AnnounceBackpressureEvent, AnnounceEgressOutcome, EgressLaneMetricsSnapshot,
    EgressMetricsSnapshot,
};
use crate::wire::{WireContext, WirePacketHeader, HEADER_MAX_LEN};

use super::{HeapFrameSlot, TokioGrantProducer};
use pending::{EgressOrigin, PendingEgressQueue};

mod pending;

pub struct Egress {
    lanes: std::vec::Vec<EgressLane>,
    pending_frames: usize,

    #[cfg(feature = "runtime-metrics")]
    metrics: EgressMetricsSnapshot,
}

const TOKIO_ANNOUNCE_PACER_DEPTH: usize = 256;
const TOKIO_ANNOUNCE_RETRY_POLICY: PacerRetryPolicy = PacerRetryPolicy::new(50, 1_000);

struct EgressLane {
    id: InterfaceId,
    producer: TokioGrantProducer,
    connection: Option<ConnectionView>,
    pending: PendingEgressQueue,

    #[cfg(feature = "runtime-metrics")]
    logical_interface: InterfaceId,
}

impl EgressLane {
    fn is_available(&self) -> bool {
        self.connection
            .as_ref()
            .is_none_or(|connection| connection.connection().is_online())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EgressEnqueueOutcome {
    Enqueued,
    Deferred,
    LaneMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EgressTryEnqueueOutcome {
    Enqueued,
    LaneFull,
    LaneMissing,
}

pub(super) enum ForwardedSlotOutcome {
    Moved { vacated: HeapFrameSlot },
    CopyRequired { source: HeapFrameSlot },
}

#[derive(Clone, Copy)]
enum EgressQueue {
    Expedited,
    Bulk,
}

fn egress_queue(frame: &[u8]) -> EgressQueue {
    match WirePacketHeader::parse(frame) {
        Ok((header, _)) if header.context == WireContext::Resource => EgressQueue::Bulk,
        Ok(_) | Err(_) => EgressQueue::Expedited,
    }
}

impl Egress {
    #[must_use]
    pub fn new(lanes: std::vec::Vec<(InterfaceId, TokioGrantProducer)>) -> Self {
        let lanes = lanes
            .into_iter()
            .map(|(id, producer)| EgressLane {
                id,
                producer,
                connection: None,
                pending: PendingEgressQueue::new(),
                #[cfg(feature = "runtime-metrics")]
                logical_interface: id,
            })
            .collect::<std::vec::Vec<_>>();

        #[cfg(feature = "runtime-metrics")]
        let metrics = {
            let mut metrics = EgressMetricsSnapshot::default();
            for lane in &lanes {
                metrics.announces.register_interface(lane.logical_interface);
            }
            metrics
        };

        Self {
            lanes,
            pending_frames: 0,
            #[cfg(feature = "runtime-metrics")]
            metrics,
        }
    }

    pub(super) fn enqueue(&mut self, target: InterfaceId, bytes: &[u8]) -> EgressEnqueueOutcome {
        self.enqueue_with_origin(target, bytes, egress_queue(bytes), EgressOrigin::Internal)
    }

    fn enqueue_from_ingress(
        &mut self,
        source: InterfaceId,
        target: InterfaceId,
        bytes: &[u8],
    ) -> EgressEnqueueOutcome {
        self.enqueue_with_origin(
            target,
            bytes,
            egress_queue(bytes),
            EgressOrigin::Ingress(source),
        )
    }

    fn enqueue_with_origin(
        &mut self,
        target: InterfaceId,
        bytes: &[u8],
        queue: EgressQueue,
        origin: EgressOrigin,
    ) -> EgressEnqueueOutcome {
        let outcome = match self.try_enqueue_as(target, bytes, queue) {
            EgressTryEnqueueOutcome::Enqueued => EgressEnqueueOutcome::Enqueued,
            EgressTryEnqueueOutcome::LaneFull => self.defer(target, bytes, queue, origin),
            EgressTryEnqueueOutcome::LaneMissing => EgressEnqueueOutcome::LaneMissing,
        };
        self.record_generic_enqueue_outcome(outcome);
        outcome
    }

    fn try_enqueue(&mut self, target: InterfaceId, bytes: &[u8]) -> EgressTryEnqueueOutcome {
        self.try_enqueue_as(target, bytes, egress_queue(bytes))
    }

    fn try_enqueue_as(
        &mut self,
        target: InterfaceId,
        bytes: &[u8],
        queue: EgressQueue,
    ) -> EgressTryEnqueueOutcome {
        let has_pending = self.pending_frames > 0;
        for lane in &mut self.lanes {
            if lane.id != target {
                continue;
            }

            if has_pending && !lane.pending.is_empty() {
                return EgressTryEnqueueOutcome::LaneFull;
            }

            match lane.producer.try_grant() {
                None => return EgressTryEnqueueOutcome::LaneFull,
                Some(slot) => {
                    slot.fill(bytes);
                    match queue {
                        EgressQueue::Expedited => lane.producer.commit_expedited(),
                        EgressQueue::Bulk => lane.producer.commit(),
                    }

                    #[cfg(feature = "runtime-metrics")]
                    {
                        self.metrics.enqueued_frames =
                            self.metrics.enqueued_frames.saturating_add(1);
                    }

                    return EgressTryEnqueueOutcome::Enqueued;
                }
            }
        }
        EgressTryEnqueueOutcome::LaneMissing
    }

    pub(super) fn try_move_forwarded_slot(
        &mut self,
        target: InterfaceId,
        header: WirePacketHeader,
        payload: std::ops::Range<usize>,
        mut source: HeapFrameSlot,
    ) -> ForwardedSlotOutcome {
        if payload.start > payload.end
            || payload.end > source.len
            || header.wire_len() != payload.start
        {
            return ForwardedSlotOutcome::CopyRequired { source };
        }
        let has_pending = self.pending_frames > 0;
        for lane in &mut self.lanes {
            if lane.id != target {
                continue;
            }
            if has_pending && !lane.pending.is_empty() {
                return ForwardedSlotOutcome::CopyRequired { source };
            }
            let Some(destination) = lane.producer.try_grant() else {
                return ForwardedSlotOutcome::CopyRequired { source };
            };
            if payload.end > destination.cap {
                return ForwardedSlotOutcome::CopyRequired { source };
            }
            let Ok(header_len) = header.write(&mut source.bytes[..payload.start]) else {
                return ForwardedSlotOutcome::CopyRequired { source };
            };
            debug_assert_eq!(header_len, payload.start);

            source.bytes.truncate(payload.end);
            std::mem::swap(&mut destination.bytes, &mut source.bytes);
            destination.len = payload.end;
            destination.packet_phy = Default::default();
            source.bytes.clear();
            source.len = 0;
            source.packet_phy = Default::default();
            match egress_queue(destination.frame()) {
                EgressQueue::Expedited => lane.producer.commit_expedited(),
                EgressQueue::Bulk => lane.producer.commit(),
            }
            #[cfg(feature = "runtime-metrics")]
            {
                self.metrics.enqueued_frames = self.metrics.enqueued_frames.saturating_add(1);
            }
            return ForwardedSlotOutcome::Moved { vacated: source };
        }
        ForwardedSlotOutcome::CopyRequired { source }
    }

    #[cold]
    fn defer(
        &mut self,
        target: InterfaceId,
        bytes: &[u8],
        queue: EgressQueue,
        origin: EgressOrigin,
    ) -> EgressEnqueueOutcome {
        let Some(lane) = self.lanes.iter_mut().find(|lane| lane.id == target) else {
            return EgressEnqueueOutcome::LaneMissing;
        };
        lane.pending.push(queue, bytes.to_vec(), origin);
        self.pending_frames += 1;
        lane.producer.arm_release_wake();
        #[cfg(feature = "runtime-metrics")]
        {
            self.metrics.backpressured_frames = self.metrics.backpressured_frames.saturating_add(1);
            self.metrics.maximum_pending_frames = self
                .metrics
                .maximum_pending_frames
                .max(self.pending_len_u32());
        }
        EgressEnqueueOutcome::Deferred
    }

    pub(super) fn flush_pending(&mut self, budget: usize) -> usize {
        if self.pending_frames == 0 {
            return 0;
        }
        self.flush_pending_frames(budget)
    }

    #[cold]
    fn flush_pending_frames(&mut self, budget: usize) -> usize {
        let mut flushed = 0usize;
        'lanes: for lane in &mut self.lanes {
            let producer = &mut lane.producer;
            let pending_frames = &mut lane.pending;
            loop {
                if flushed == budget {
                    if !pending_frames.is_empty() {
                        producer.arm_release_wake();
                    }
                    break 'lanes;
                }
                if pending_frames.is_empty() {
                    producer.disarm_release_wake();
                    break;
                }
                if producer.try_grant().is_none() {
                    producer.arm_release_wake();
                }
                let Some(slot) = producer.try_grant() else {
                    break;
                };
                let Some((queue, bytes)) = pending_frames.pop() else {
                    producer.disarm_release_wake();
                    break;
                };
                self.pending_frames -= 1;
                slot.fill(&bytes);
                match queue {
                    EgressQueue::Expedited => producer.commit_expedited(),
                    EgressQueue::Bulk => producer.commit(),
                }
                flushed = flushed.saturating_add(1);
                #[cfg(feature = "runtime-metrics")]
                {
                    self.metrics.enqueued_frames = self.metrics.enqueued_frames.saturating_add(1);
                }
            }
        }
        flushed
    }

    pub(super) fn has_pending(&self) -> bool {
        self.pending_frames > 0
    }

    pub(super) fn blocks_source(&self, source: InterfaceId) -> bool {
        if self.pending_frames == 0 {
            return false;
        }
        self.pending_blocks_source(source)
    }

    #[cold]
    fn pending_blocks_source(&self, source: InterfaceId) -> bool {
        self.lanes
            .iter()
            .any(|lane| lane.pending.blocks_source(source))
    }

    #[cfg(feature = "runtime-metrics")]
    fn pending_len_u32(&self) -> u32 {
        u32::try_from(self.pending_frames).unwrap_or(u32::MAX)
    }

    #[cfg(feature = "runtime-metrics")]
    fn record_generic_enqueue_outcome(&mut self, outcome: EgressEnqueueOutcome) {
        match outcome {
            EgressEnqueueOutcome::Enqueued => {}
            EgressEnqueueOutcome::Deferred => {}
            EgressEnqueueOutcome::LaneMissing => {
                self.metrics.missing_lane_drops = self.metrics.missing_lane_drops.saturating_add(1);
            }
        }
    }

    #[cfg(not(feature = "runtime-metrics"))]
    fn record_generic_enqueue_outcome(&mut self, _outcome: EgressEnqueueOutcome) {}

    fn record_full_lane_drop(&mut self) {
        #[cfg(feature = "runtime-metrics")]
        {
            self.metrics.full_lane_drops = self.metrics.full_lane_drops.saturating_add(1);
        }
    }

    fn record_ifac_rejection(&mut self) {
        #[cfg(feature = "runtime-metrics")]
        {
            self.metrics.ifac_rejected_frames = self.metrics.ifac_rejected_frames.saturating_add(1);
        }
    }

    fn skip_unavailable(&mut self, target: InterfaceId) -> bool {
        let unavailable = self
            .lanes
            .iter()
            .find(|lane| lane.id == target)
            .is_some_and(|lane| !lane.is_available());

        #[cfg(feature = "runtime-metrics")]
        if unavailable {
            self.metrics.unavailable_frame_skips =
                self.metrics.unavailable_frame_skips.saturating_add(1);
        }

        unavailable
    }

    #[cfg(feature = "runtime-metrics")]
    fn record_announce(
        &mut self,
        target: InterfaceId,
        bytes: usize,
        origin: AnnounceOrigin,
        outcome: AnnounceEgressOutcome,
    ) {
        let logical_interface = self
            .lanes
            .iter()
            .find(|lane| lane.id == target)
            .map_or(target, |lane| lane.logical_interface);
        self.metrics
            .announces
            .record(origin, logical_interface, outcome, bytes);
    }

    #[cfg(feature = "runtime-metrics")]
    fn record_backpressure(
        &mut self,
        target: InterfaceId,
        origin: AnnounceOrigin,
        event: AnnounceBackpressureEvent,
    ) {
        let logical_interface = self
            .lanes
            .iter()
            .find(|lane| lane.id == target)
            .map_or(target, |lane| lane.logical_interface);
        self.metrics
            .announces
            .record_backpressure(origin, logical_interface, event);
    }

    /// Every lane of the supervisor's member kind that `fan` selects.
    fn broadcast_targets(
        &self,
        supervisor: InterfaceKind,
        fan: FanTarget,
    ) -> std::vec::Vec<InterfaceId> {
        let member = supervisor.member_kind();
        self.lanes
            .iter()
            .map(|lane| lane.id)
            .filter(|id| id.kind() == member)
            .filter(|id| match fan {
                FanTarget::All => true,
                FanTarget::Only(only) => *id == only,
                FanTarget::AllExcept(except) => *id != except,
            })
            .collect()
    }

    fn emit(
        &mut self,
        target: InterfaceId,
        size_hint: usize,
        fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
        discard: &mut [u8],
        origin: EgressOrigin,
    ) {
        let has_pending = self.pending_frames > 0;
        let mut lane_found = false;
        for lane in &mut self.lanes {
            if lane.id != target {
                continue;
            }
            lane_found = true;
            if !has_pending || lane.pending.is_empty() {
                if let Some(slot) = lane.producer.try_grant() {
                    let hint = size_hint.clamp(1, MAX_WIRE_FRAME_LEN);
                    if slot.bytes.len() < hint {
                        slot.bytes.resize(hint, 0);
                    }
                    if let Some(len) = fill(&mut slot.bytes[..hint]) {
                        slot.len = len.min(hint);
                        match egress_queue(slot.frame()) {
                            EgressQueue::Expedited => lane.producer.commit_expedited(),
                            EgressQueue::Bulk => lane.producer.commit(),
                        }
                        #[cfg(feature = "runtime-metrics")]
                        {
                            self.metrics.enqueued_frames =
                                self.metrics.enqueued_frames.saturating_add(1);
                        }
                    }
                    return;
                }
            }
            break;
        }
        if !lane_found {
            let _fill_result = fill(discard);
            #[cfg(feature = "runtime-metrics")]
            if _fill_result.is_some() {
                self.metrics.missing_lane_drops = self.metrics.missing_lane_drops.saturating_add(1);
            }
            return;
        }
        let fill_result = fill(discard);
        if let Some(len) = fill_result {
            let len = len.min(discard.len());
            let queue = egress_queue(&discard[..len]);
            let outcome = self.defer(target, &discard[..len], queue, origin);
            self.record_generic_enqueue_outcome(outcome);
        }
    }

    pub(super) fn add_lane(
        &mut self,
        id: InterfaceId,
        logical_interface: InterfaceId,
        producer: TokioGrantProducer,
        connection: Option<ConnectionView>,
    ) {
        #[cfg(not(feature = "runtime-metrics"))]
        let _ = logical_interface;

        self.lanes.push(EgressLane {
            id,
            producer,
            connection,
            pending: PendingEgressQueue::new(),
            #[cfg(feature = "runtime-metrics")]
            logical_interface,
        });

        #[cfg(feature = "runtime-metrics")]
        self.metrics.announces.register_interface(logical_interface);
    }

    pub(super) fn remove_lane(&mut self, id: InterfaceId) {
        let Some(index) = self.lanes.iter().position(|lane| lane.id == id) else {
            return;
        };
        let removed = self.lanes.remove(index);
        self.pending_frames -= removed.pending.len();
    }

    #[cfg(feature = "runtime-metrics")]
    pub(super) fn metrics_snapshot(
        &self,
        pacers: &[InterfacePacer],
        now: InstantMillis,
    ) -> EgressMetricsSnapshot {
        let mut snapshot = self.metrics.clone();
        snapshot.announces.reset_pacer_gauges();
        for entry in pacers {
            let oldest_deferred_age_ms = entry
                .pacer
                .oldest_deferred_at()
                .map_or(0, |at| now.0.saturating_sub(at.0));
            snapshot.announces.add_pacer_gauges(
                entry.logical_interface,
                entry.pacer.queued_len(),
                entry.pacer.deferred_len(),
                oldest_deferred_age_ms,
            );
        }
        snapshot.lanes = self
            .lanes
            .iter()
            .map(|lane| EgressLaneMetricsSnapshot {
                physical_interface: lane.id,
                logical_interface: lane.logical_interface,
                capacity: u32::try_from(lane.producer.capacity()).unwrap_or(u32::MAX),
                occupancy: u32::try_from(lane.producer.occupancy()).unwrap_or(u32::MAX),
                pending: u32::try_from(lane.pending.len()).unwrap_or(u32::MAX),
            })
            .collect();
        snapshot.pending_frames = self.pending_len_u32();
        snapshot
    }
}

#[cfg(feature = "runtime-metrics")]
pub(super) type TokioAnnouncePacer = AnnouncePacer<
    BoundedHeapPacerQueue<TOKIO_ANNOUNCE_PACER_DEPTH, AnnounceOrigin>,
    AnnounceOrigin,
>;
#[cfg(not(feature = "runtime-metrics"))]
pub(super) type TokioAnnouncePacer =
    AnnouncePacer<BoundedHeapPacerQueue<TOKIO_ANNOUNCE_PACER_DEPTH>>;

pub(super) struct InterfacePacer {
    pub(super) id: InterfaceId,
    #[cfg(feature = "runtime-metrics")]
    pub(super) logical_interface: InterfaceId,
    pub(super) pacer: TokioAnnouncePacer,
}

impl InterfacePacer {
    pub(super) fn from_descriptor(
        descriptor: &InterfaceDescriptor,
        logical_interface: InterfaceId,
    ) -> Self {
        #[cfg(not(feature = "runtime-metrics"))]
        let _ = logical_interface;

        Self {
            id: descriptor.id,
            #[cfg(feature = "runtime-metrics")]
            logical_interface,
            pacer: AnnouncePacer::new(
                descriptor.announce_bandwidth_cap,
                descriptor.bitrate,
                TOKIO_ANNOUNCE_RETRY_POLICY,
            ),
        }
    }
}

/// Heap-parked wire scratch for every emission that can't land straight in a granted slot: `emit` carries a discarded or pre-mask frame, `masked` the IFAC mask output. Boxed once per manifold — wire-sized buffers never live on a task stack.
pub(super) struct WireScratch {
    emit: std::boxed::Box<[u8]>,
    masked: std::boxed::Box<[u8]>,
}

impl WireScratch {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            emit: std::vec![0u8; cap].into_boxed_slice(),
            masked: std::vec![0u8; cap].into_boxed_slice(),
        }
    }

    pub(super) fn grow(&mut self, cap: usize) {
        if self.emit.len() < cap {
            self.emit = std::vec![0u8; cap].into_boxed_slice();
            self.masked = std::vec![0u8; cap].into_boxed_slice();
        }
    }
}

pub(super) fn route_reaction<A>(
    reaction: EngineReaction<'_>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    scratch: &mut WireScratch,
    now: InstantMillis,
    app: &mut A,
) where
    A: FnMut(Journaled<'_>),
{
    route_reaction_with_work(
        reaction,
        egress,
        ifacs,
        pacers,
        scratch,
        now,
        app,
        &mut |work: NoOwedWork| match work {},
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn route_ingress_reaction<A>(
    reaction: EngineReaction<'_>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    scratch: &mut WireScratch,
    now: InstantMillis,
    app: &mut A,
    source: InterfaceId,
) where
    A: FnMut(Journaled<'_>),
{
    route_ingress_reaction_with_work(
        reaction,
        egress,
        ifacs,
        pacers,
        scratch,
        now,
        app,
        &mut |work: NoOwedWork| match work {},
        source,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn route_ingress_reaction_with_work<A, Work, W>(
    reaction: EngineReaction<'_, Work>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    scratch: &mut WireScratch,
    now: InstantMillis,
    app: &mut A,
    fulfill: &mut W,
    source: InterfaceId,
) where
    A: FnMut(Journaled<'_>),
    W: FnMut(Work),
{
    route_reaction_with_origin(
        reaction,
        egress,
        ifacs,
        pacers,
        scratch,
        now,
        app,
        fulfill,
        EgressOrigin::Ingress(source),
    );
}

// This is the common bow-tie seam. Its borrowed arguments make all routing
// destinations visible without allocating or manufacturing a state owner.
#[allow(clippy::too_many_arguments)]
pub(super) fn route_reaction_with_work<A, Work, W>(
    reaction: EngineReaction<'_, Work>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    scratch: &mut WireScratch,
    now: InstantMillis,
    app: &mut A,
    fulfill: &mut W,
) where
    A: FnMut(Journaled<'_>),
    W: FnMut(Work),
{
    route_reaction_with_origin(
        reaction,
        egress,
        ifacs,
        pacers,
        scratch,
        now,
        app,
        fulfill,
        EgressOrigin::Internal,
    );
}

#[allow(clippy::too_many_arguments)]
fn route_reaction_with_origin<A, Work, W>(
    reaction: EngineReaction<'_, Work>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    scratch: &mut WireScratch,
    now: InstantMillis,
    app: &mut A,
    fulfill: &mut W,
    origin: EgressOrigin,
) where
    A: FnMut(Journaled<'_>),
    W: FnMut(Work),
{
    let mut directive_egress = TokioDirectiveEgress {
        egress,
        ifacs,
        pacers,
        scratch,
        now,
        origin,
    };
    route_engine_reaction(reaction, &mut directive_egress, app, fulfill);
}

struct TokioDirectiveEgress<'a> {
    egress: &'a mut Egress,
    ifacs: &'a [InterfaceIfac],
    pacers: &'a mut [InterfacePacer],
    scratch: &'a mut WireScratch,
    now: InstantMillis,
    origin: EgressOrigin,
}

impl DirectiveEgress for TokioDirectiveEgress<'_> {
    fn send(&mut self, target: InterfaceId, bytes: &[u8]) {
        enqueue_for_wire(
            self.egress,
            self.ifacs,
            target,
            bytes,
            &mut self.scratch.masked,
            self.origin,
        );
    }

    fn send_if_online(&mut self, target: InterfaceId, bytes: &[u8], on_send: &mut dyn FnMut()) {
        if self.egress.skip_unavailable(target) {
            return;
        }
        on_send();
        self.send(target, bytes);
    }

    fn send_announce(&mut self, target: InterfaceId, announce: AnnounceDirective<'_>) {
        offer_to_pacer(
            self.pacers,
            target,
            PacedAnnounce {
                bytes: announce.bytes(),
                hops: announce.hops(),
                #[cfg(feature = "runtime-metrics")]
                origin: announce.origin(),
            },
            self.now,
            self.egress,
            self.ifacs,
        );
    }

    fn send_to_fleet(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]) {
        for target in self.egress.broadcast_targets(supervisor, fan) {
            enqueue_for_wire(
                self.egress,
                self.ifacs,
                target,
                bytes,
                &mut self.scratch.masked,
                self.origin,
            );
        }
    }

    fn send_announce_to_fleet(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        announce: AnnounceDirective<'_>,
    ) {
        let bytes = announce.bytes();
        let hops = announce.hops();
        #[cfg(feature = "runtime-metrics")]
        let origin = announce.origin();
        for target in self.egress.broadcast_targets(supervisor, fan) {
            offer_to_pacer(
                self.pacers,
                target,
                PacedAnnounce {
                    bytes,
                    hops,
                    #[cfg(feature = "runtime-metrics")]
                    origin,
                },
                self.now,
                self.egress,
                self.ifacs,
            );
        }
    }

    fn emit_frame(
        &mut self,
        target: InterfaceId,
        size_hint: usize,
        fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
    ) {
        emit_for_wire(
            self.egress,
            self.ifacs,
            target,
            size_hint,
            fill,
            self.scratch,
            self.origin,
        );
    }

    fn forward_frame(&mut self, target: InterfaceId, header: WirePacketHeader, payload: &[u8]) {
        forward_for_wire(
            self.egress,
            self.ifacs,
            target,
            header,
            payload,
            self.scratch,
            self.origin,
        );
    }

    #[cfg(feature = "runtime-metrics")]
    fn send_measured_local_announce(&mut self, target: InterfaceId, bytes: &[u8]) {
        enqueue_pacerless_announce_for_wire(
            self.egress,
            self.ifacs,
            target,
            bytes,
            &mut self.scratch.masked,
            AnnounceOrigin::Local,
        );
    }

    #[cfg(feature = "runtime-metrics")]
    fn send_measured_local_announce_to_fleet(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &[u8],
    ) {
        for target in self.egress.broadcast_targets(supervisor, fan) {
            enqueue_pacerless_announce_for_wire(
                self.egress,
                self.ifacs,
                target,
                bytes,
                &mut self.scratch.masked,
                AnnounceOrigin::Local,
            );
        }
    }
}

fn emit_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    size_hint: usize,
    fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
    scratch: &mut WireScratch,
    origin: EgressOrigin,
) {
    match ifac_for(ifacs, target) {
        Some(entry) => {
            if let Some(len) = fill(&mut scratch.emit) {
                let queue = egress_queue(&scratch.emit[..len]);
                if let Ok(masked_len) = entry
                    .context
                    .try_mask_outbound(&scratch.emit[..len], &mut scratch.masked)
                {
                    egress.enqueue_with_origin(
                        target,
                        &scratch.masked[..masked_len],
                        queue,
                        origin,
                    );
                } else {
                    egress.record_ifac_rejection();
                }
            }
        }
        None => egress.emit(target, size_hint, fill, &mut scratch.emit, origin),
    }
}

fn forward_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    header: WirePacketHeader,
    payload: &[u8],
    scratch: &mut WireScratch,
    origin: EgressOrigin,
) {
    emit_for_wire(
        egress,
        ifacs,
        target,
        HEADER_MAX_LEN + payload.len(),
        &mut |slot| {
            let header_len = header.write(slot).ok()?;
            let frame_len = header_len.checked_add(payload.len())?;
            let destination = slot.get_mut(header_len..frame_len)?;
            destination.copy_from_slice(payload);
            Some(frame_len)
        },
        scratch,
        origin,
    );
}

pub(super) fn forward_from_ingress(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    source: InterfaceId,
    target: InterfaceId,
    header: WirePacketHeader,
    payload: &[u8],
    scratch: &mut WireScratch,
) {
    forward_for_wire(
        egress,
        ifacs,
        target,
        header,
        payload,
        scratch,
        EgressOrigin::Ingress(source),
    );
}

pub(super) fn ifac_for(ifacs: &[InterfaceIfac], id: InterfaceId) -> Option<&InterfaceIfac> {
    if ifacs.is_empty() {
        return None;
    }
    ifacs.iter().find(|entry| entry.id == id)
}

/// The one egress choke: a target with an access code never sees clean bytes on its wire, and a frame the mask refuses (oversize) is dropped rather than leaked open.
fn enqueue_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
    masked: &mut [u8],
    origin: EgressOrigin,
) {
    match ifac_for(ifacs, target) {
        Some(entry) => match entry.context.try_mask_outbound(bytes, masked) {
            Ok(masked_len) => {
                egress.enqueue_with_origin(
                    target,
                    &masked[..masked_len],
                    egress_queue(bytes),
                    origin,
                );
            }
            Err(_) => egress.record_ifac_rejection(),
        },
        None => match origin {
            EgressOrigin::Ingress(source) => {
                egress.enqueue_from_ingress(source, target, bytes);
            }
            EgressOrigin::Internal => {
                egress.enqueue(target, bytes);
            }
        },
    }
}

#[cfg(feature = "runtime-metrics")]
pub(super) fn enqueue_announce_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
    masked: &mut [u8],
    origin: AnnounceOrigin,
) -> PacerDelivery {
    let (outcome, wire_bytes) = match ifac_for(ifacs, target) {
        Some(entry) => {
            let Ok(masked_len) = entry.context.try_mask_outbound(bytes, masked) else {
                egress.record_ifac_rejection();
                egress.record_announce(
                    target,
                    bytes.len(),
                    origin,
                    AnnounceEgressOutcome::IfacRejected,
                );
                return PacerDelivery::Discarded;
            };
            (
                egress.try_enqueue(target, &masked[..masked_len]),
                masked_len,
            )
        }
        None => (egress.try_enqueue(target, bytes), bytes.len()),
    };
    match outcome {
        EgressTryEnqueueOutcome::Enqueued => {
            egress.record_announce(target, wire_bytes, origin, AnnounceEgressOutcome::Enqueued);
            PacerDelivery::Admitted
        }
        EgressTryEnqueueOutcome::LaneFull => PacerDelivery::Backpressured,
        EgressTryEnqueueOutcome::LaneMissing => {
            egress.record_generic_enqueue_outcome(EgressEnqueueOutcome::LaneMissing);
            egress.record_announce(
                target,
                wire_bytes,
                origin,
                AnnounceEgressOutcome::LaneMissing,
            );
            PacerDelivery::Discarded
        }
    }
}

#[cfg(not(feature = "runtime-metrics"))]
fn enqueue_announce_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
    masked: &mut [u8],
) -> PacerDelivery {
    let outcome = match ifac_for(ifacs, target) {
        Some(entry) => {
            let Some(masked_len) = entry.context.mask_outbound(bytes, masked) else {
                return PacerDelivery::Discarded;
            };
            egress.try_enqueue(target, &masked[..masked_len])
        }
        None => egress.try_enqueue(target, bytes),
    };
    match outcome {
        EgressTryEnqueueOutcome::Enqueued => PacerDelivery::Admitted,
        EgressTryEnqueueOutcome::LaneFull => PacerDelivery::Backpressured,
        EgressTryEnqueueOutcome::LaneMissing => PacerDelivery::Discarded,
    }
}

#[cfg(feature = "runtime-metrics")]
fn enqueue_pacerless_announce_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
    masked: &mut [u8],
    origin: AnnounceOrigin,
) {
    if enqueue_announce_for_wire(egress, ifacs, target, bytes, masked, origin)
        == PacerDelivery::Backpressured
    {
        egress.record_full_lane_drop();
        egress.record_announce(target, bytes.len(), origin, AnnounceEgressOutcome::LaneFull);
    }
}

#[cfg(not(feature = "runtime-metrics"))]
fn enqueue_pacerless_announce_for_wire(
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    target: InterfaceId,
    bytes: &[u8],
    masked: &mut [u8],
) {
    if enqueue_announce_for_wire(egress, ifacs, target, bytes, masked)
        == PacerDelivery::Backpressured
    {
        egress.record_full_lane_drop();
    }
}

/// A paced announce is broadcast-sized by construction, so its mask scratch fits on the stack — the wire-sized [`WireScratch`] is reserved for the frame paths.
const PACED_MASK_LEN: usize = crate::wire::BROADCAST_MTU + crate::interfaces::IFAC_MAX_SIZE;

pub(super) struct PacedAnnounce<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) hops: u8,
    #[cfg(feature = "runtime-metrics")]
    pub(super) origin: AnnounceOrigin,
}

#[cfg(feature = "runtime-metrics")]
fn record_pacer_events(
    egress: &mut Egress,
    target: InterfaceId,
    events: impl IntoIterator<Item = PacerEvent<AnnounceOrigin>>,
) {
    for event in events {
        match event {
            PacerEvent::Deferred(entry) => egress.record_backpressure(
                target,
                entry.metadata,
                AnnounceBackpressureEvent::Deferred,
            ),
            PacerEvent::Retry(entry) => {
                egress.record_backpressure(target, entry.metadata, AnnounceBackpressureEvent::Retry)
            }
            PacerEvent::Recovered(entry) => egress.record_backpressure(
                target,
                entry.metadata,
                AnnounceBackpressureEvent::Recovered,
            ),
            PacerEvent::Evicted(entry) => {
                record_shed_entry(egress, target, entry, AnnounceEgressOutcome::PacerEvicted)
            }
            PacerEvent::Expired(entry) => {
                record_shed_entry(egress, target, entry, AnnounceEgressOutcome::PacerExpired)
            }
        }
    }
}

#[cfg(feature = "runtime-metrics")]
fn record_shed_entry(
    egress: &mut Egress,
    target: InterfaceId,
    entry: PacerEntry<AnnounceOrigin>,
    outcome: AnnounceEgressOutcome,
) {
    egress.record_announce(target, entry.frame_bytes, entry.metadata, outcome);
}

#[cfg(feature = "runtime-metrics")]
pub(super) fn offer_to_pacer(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    announce: PacedAnnounce<'_>,
    now: InstantMillis,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
) {
    if egress.skip_unavailable(target) {
        egress.record_announce(
            target,
            announce.bytes.len(),
            announce.origin,
            AnnounceEgressOutcome::InterfaceUnavailable,
        );
        return;
    }
    let mut events = std::vec::Vec::new();
    let offer = match pacers.iter_mut().find(|entry| entry.id == target) {
        Some(entry) => entry.pacer.offer_tagged_observed(
            announce.bytes,
            announce.hops,
            now,
            announce.origin,
            |frame, frame_origin| {
                let mut masked = [0u8; PACED_MASK_LEN];
                enqueue_announce_for_wire(egress, ifacs, target, frame, &mut masked, frame_origin)
            },
            |event| events.push(event),
        ),
        None => {
            let mut masked = [0u8; PACED_MASK_LEN];
            enqueue_pacerless_announce_for_wire(
                egress,
                ifacs,
                target,
                announce.bytes,
                &mut masked,
                announce.origin,
            );
            PacerOffer::Admitted
        }
    };
    record_pacer_events(egress, target, events);
    if matches!(offer, PacerOffer::Rejected(_)) {
        egress.record_announce(
            target,
            announce.bytes.len(),
            announce.origin,
            AnnounceEgressOutcome::PacerRejected,
        );
    }
}

#[cfg(not(feature = "runtime-metrics"))]
pub(super) fn offer_to_pacer(
    pacers: &mut [InterfacePacer],
    target: InterfaceId,
    announce: PacedAnnounce<'_>,
    now: InstantMillis,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
) {
    if egress.skip_unavailable(target) {
        return;
    }
    match pacers.iter_mut().find(|entry| entry.id == target) {
        Some(entry) => {
            entry
                .pacer
                .offer(announce.bytes, announce.hops, now, |frame| {
                    let mut masked = [0u8; PACED_MASK_LEN];
                    enqueue_announce_for_wire(egress, ifacs, target, frame, &mut masked)
                });
        }
        None => {
            let mut masked = [0u8; PACED_MASK_LEN];
            enqueue_pacerless_announce_for_wire(egress, ifacs, target, announce.bytes, &mut masked);
        }
    }
}

#[cfg(feature = "runtime-metrics")]
pub(super) fn flush_due_pacers(
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
) {
    for entry in pacers.iter_mut() {
        let target = entry.id;
        let mut events = std::vec::Vec::new();
        entry.pacer.release_due_tagged_observed(
            now,
            |frame, origin| {
                if egress.skip_unavailable(target) {
                    egress.record_announce(
                        target,
                        frame.len(),
                        origin,
                        AnnounceEgressOutcome::InterfaceUnavailable,
                    );
                    return PacerDelivery::Discarded;
                }
                let mut masked = [0u8; PACED_MASK_LEN];
                enqueue_announce_for_wire(egress, ifacs, target, frame, &mut masked, origin)
            },
            |event| events.push(event),
        );
        record_pacer_events(egress, target, events);
    }
}

#[cfg(not(feature = "runtime-metrics"))]
pub(super) fn flush_due_pacers(
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
) {
    for entry in pacers.iter_mut() {
        let target = entry.id;
        entry.pacer.release_due(now, |frame| {
            if egress.skip_unavailable(target) {
                return PacerDelivery::Discarded;
            }
            let mut masked = [0u8; PACED_MASK_LEN];
            enqueue_announce_for_wire(egress, ifacs, target, frame, &mut masked)
        });
    }
}

pub(super) fn soonest_pacer_release(pacers: &[InterfacePacer]) -> Option<InstantMillis> {
    pacers
        .iter()
        .filter_map(|entry| entry.pacer.next_release())
        .min_by_key(|deadline| deadline.0)
}

pub(super) fn clear_announce_queues(pacers: &mut [InterfacePacer]) -> usize {
    pacers.iter_mut().fold(0, |dropped, entry| {
        dropped.saturating_add(entry.pacer.clear_queue())
    })
}

#[cfg(test)]
mod tests;
