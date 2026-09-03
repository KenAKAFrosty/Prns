use tokio::sync::mpsc::UnboundedReceiver;

use crate::engine::{
    ClassifiedInboundPacket, CryptoOwed, EngineReaction, EngineState, IngestIo, InstantMillis,
    Journaled, OwedWork, ProofRequest, WakeSchedules,
};
use crate::interfaces::{
    FrameAccountingEvent, IfacUnmaskError, InboundPacket, InterfaceId, InterfaceIfac,
    PacketPhyStats,
};
use crate::manifold::wake_schedule::merge_wake_schedules_delta;
use crate::manifold::Host;
use crate::routing::dedup::PacketHash;
use crate::routing::links::resources::receive::part_hash::ResourcePartHashPlan;
use crate::routing::links::resources::ResourceOffer;
use crate::routing::links::LinkId;
use crate::runtime::InterfaceStore;
use crate::storage::StorageLayout;

use super::crypto_pool::{run_link_sign_job, CryptoPool, LinkSignCompleted, LinkSignJob};
use super::egress::{
    ifac_for, route_reaction, route_reaction_with_work, Egress, InterfacePacer, WireScratch,
};
use super::interface_topology::InterfaceTopology;
use super::journal_delivery::JournalDispatch;
use super::owed_work::PendingOwedWork;

#[derive(Clone, Copy)]
enum IngressBufferSource {
    GrantSlot,
    UnmaskScratch,
}

#[derive(Clone, Copy)]
struct IngressPacketSpan {
    start: usize,
    len: usize,
}

impl IngressPacketSpan {
    fn of(bytes: &[u8]) -> Self {
        Self {
            start: bytes.as_ptr() as usize,
            len: bytes.len(),
        }
    }

    fn locate(&self, part: &[u8]) -> Option<std::ops::Range<usize>> {
        let start = (part.as_ptr() as usize).checked_sub(self.start)?;
        let end = start.checked_add(part.len())?;
        (end <= self.len).then_some(start..end)
    }
}

struct DeferredResourcePartHash {
    plan: ResourcePartHashPlan,
    part: std::ops::Range<usize>,
}

fn route_ingress_reaction<J>(
    reaction: EngineReaction<'_, OwedWork<'_>>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    wire_scratch: &mut WireScratch,
    journal: &mut JournalDispatch<J>,
    owed_work: &mut PendingOwedWork,
    crypto_pool: Option<&CryptoPool>,
    link_signs: &mut std::vec::Vec<LinkSignJob>,
    link_identity_barriers: &mut std::vec::Vec<(InterfaceId, LinkId)>,
    packet_span: IngressPacketSpan,
    deferred_resource_part_hash: &mut Option<DeferredResourcePartHash>,
    source: InterfaceId,
    now: InstantMillis,
) where
    J: for<'a> FnMut(Journaled<'a>),
{
    route_reaction_with_work(
        reaction,
        egress,
        ifacs,
        pacers,
        wire_scratch,
        now,
        &mut |journaled| journal.route(journaled),
        &mut |work| match work {
            OwedWork::Crypto(CryptoOwed::LinkReceiptSign(owed)) => {
                link_signs.push(LinkSignJob::Receipt(owed));
            }
            OwedWork::Crypto(CryptoOwed::ChannelAckSign(owed)) => {
                link_signs.push(LinkSignJob::ChannelAck(owed));
            }
            OwedWork::Crypto(owed) => {
                if let CryptoOwed::LinkIdentityVerify(owed) = &owed {
                    if !link_identity_barriers
                        .iter()
                        .any(|(_, link_id)| *link_id == owed.link_id)
                    {
                        link_identity_barriers.push((source, owed.link_id));
                    }
                }
                owed_work.push_crypto(owed);
            }
            OwedWork::ResourceBuild(owed) => {
                owed_work.push(OwedWork::ResourceBuild(owed), crypto_pool);
            }
            OwedWork::ResourceSeal(owed) => {
                owed_work.push(OwedWork::ResourceSeal(owed), crypto_pool);
            }
            OwedWork::ResourcePartHash(owed) => {
                let (plan, part) = owed.into_parts();
                if let Some(part) = packet_span.locate(part) {
                    *deferred_resource_part_hash = Some(DeferredResourcePartHash { plan, part });
                } else {
                    owed_work.push_resource_part_hash_copy(plan, part);
                }
            }
            OwedWork::ResourceOpen(owed) => owed_work.push_resource_open(owed, crypto_pool),
            OwedWork::WholeResourceOpen(owed) => {
                owed_work.push(OwedWork::WholeResourceOpen(owed), crypto_pool);
            }
            OwedWork::ResourceDecompression(owed) => {
                owed_work.push(OwedWork::ResourceDecompression(owed), crypto_pool);
            }
        },
    );
}

pub(super) struct InboundDispatch {
    ready_lanes: std::vec::Vec<InterfaceId>,
    unmask_scratch: std::boxed::Box<[u8]>,
    link_signs: std::vec::Vec<LinkSignJob>,
    inline_link_signs: std::vec::Vec<LinkSignJob>,
    /// LINKIDENTIFY changes the authority attached to a link. Later frames from its ingress lane
    /// cannot overtake that verdict merely because signature verification ran on a worker.
    link_identity_barriers: std::vec::Vec<(InterfaceId, LinkId)>,
}

impl InboundDispatch {
    pub(super) fn new(frame_capacity: usize) -> Self {
        Self {
            ready_lanes: std::vec::Vec::new(),
            unmask_scratch: std::vec![0u8; frame_capacity].into_boxed_slice(),
            link_signs: std::vec::Vec::new(),
            inline_link_signs: std::vec::Vec::new(),
            link_identity_barriers: std::vec::Vec::new(),
        }
    }

    pub(super) fn has_ready_lanes(&self) -> bool {
        if self.link_identity_barriers.is_empty() {
            return !self.ready_lanes.is_empty();
        }
        self.ready_lanes.iter().any(|source| {
            !self
                .link_identity_barriers
                .iter()
                .any(|(blocked, _)| blocked == source)
        })
    }

    pub(super) fn release_link_identity_barrier(&mut self, link_id: LinkId) {
        self.link_identity_barriers
            .retain(|(_, blocked_link)| *blocked_link != link_id);
    }

    pub(super) fn mark_ready(&mut self, source: InterfaceId) {
        if !self.ready_lanes.contains(&source) {
            self.ready_lanes.push(source);
        }
    }

    pub(super) fn collect_ready(&mut self, notify: &mut UnboundedReceiver<InterfaceId>) {
        while let Ok(source) = notify.try_recv() {
            self.mark_ready(source);
        }
    }

    pub(super) fn grow_frame_capacity(&mut self, frame_capacity: usize) {
        if self.unmask_scratch.len() < frame_capacity {
            self.unmask_scratch = std::vec![0u8; frame_capacity].into_boxed_slice();
        }
    }

    pub(super) fn process<S, H, J, P, A>(
        &mut self,
        context: InboundContext<'_, S, H, J, P, A>,
    ) -> usize
    where
        S: StorageLayout,
        H: Host,
        J: for<'a> FnMut(Journaled<'a>),
        P: FnMut(&ProofRequest) -> bool,
        A: FnMut(&ResourceOffer) -> bool,
    {
        let InboundContext {
            engine,
            host,
            topology,
            wire_scratch,
            journal,
            crypto_pool,
            packet_phy_store,
            wake_schedules,
            should_prove,
            should_accept_resource,
            max_frames_per_lane,
            max_frames_total,
            owed_work,
            now,
        } = context;
        let Self {
            ready_lanes,
            unmask_scratch,
            link_signs,
            inline_link_signs,
            link_identity_barriers,
        } = self;
        let mut processed_frames = 0;
        'lanes: for &source in ready_lanes.iter() {
            if processed_frames == max_frames_total {
                break;
            }
            if !link_identity_barriers.is_empty()
                && link_identity_barriers
                    .iter()
                    .any(|(blocked, _)| *blocked == source)
            {
                continue;
            }
            debug_assert!(link_signs.is_empty());
            debug_assert!(inline_link_signs.is_empty());
            let frame_accounting = topology.frame_accounting_recorder(source);
            let Some((_, lane)) = topology
                .inbound_lanes
                .iter_mut()
                .find(|(id, _)| *id == source)
            else {
                continue;
            };
            lane.acknowledge();
            for _ in 0..max_frames_per_lane {
                if processed_frames == max_frames_total {
                    break;
                }
                if crypto_pool.is_some_and(|pool| {
                    !pool.has_queue_capacity(
                        owed_work
                            .pool_jobs_len()
                            .saturating_add(link_signs.len())
                            .saturating_add(2),
                    )
                }) {
                    break;
                }
                let Some(slot) = lane.try_peek() else {
                    break;
                };
                let packet_phy = slot.packet_phy;
                let (bytes, buffer_source) = match ifac_for(&topology.ifacs, source) {
                    Some(entry) => {
                        match entry
                            .context
                            .try_unmask_inbound(slot.frame(), unmask_scratch)
                        {
                            Ok(clean_len) => (
                                &mut unmask_scratch[..clean_len],
                                IngressBufferSource::UnmaskScratch,
                            ),
                            Err(IfacUnmaskError::PacketTooShort) => {
                                if let Some(recorder) = &frame_accounting {
                                    recorder.record(FrameAccountingEvent::ProtocolViolation);
                                }
                                lane.release();
                                processed_frames += 1;
                                continue;
                            }
                            Err(
                                IfacUnmaskError::MissingFlag
                                | IfacUnmaskError::InvalidSignature
                                | IfacUnmaskError::OutputTooSmall { .. },
                            ) => {
                                lane.release();
                                processed_frames += 1;
                                continue;
                            }
                        }
                    }
                    None => (slot.frame_mut(), IngressBufferSource::GrantSlot),
                };
                let packet_span = IngressPacketSpan::of(bytes);
                let packet = ClassifiedInboundPacket::classify(InboundPacket {
                    arrived_at: now,
                    source_interface: source,
                    bytes,
                });
                let packet_hash = packet.packet_hash();
                if let Some(packet_hash) = packet_hash {
                    retain_packet_phy(packet_phy_store, packet_hash, packet_phy);
                }
                let mut deferred_resource_part_hash = None;
                let ingest_report = engine.ingest_classified_into_report(
                    packet,
                    IngestIo {
                        interfaces: topology.interfaces.view(),
                        now,
                        fill_random: &mut |entropy| host.fill_random(entropy),
                        should_prove,
                        should_accept_resource,
                        sink: &mut |reaction| {
                            route_ingress_reaction(
                                reaction,
                                &mut topology.egress,
                                &topology.ifacs,
                                &mut topology.pacers,
                                wire_scratch,
                                journal,
                                owed_work,
                                crypto_pool,
                                link_signs,
                                link_identity_barriers,
                                packet_span,
                                &mut deferred_resource_part_hash,
                                source,
                                now,
                            );
                        },
                    },
                );
                if let (Some(recorder), Some(violation)) =
                    (&frame_accounting, ingest_report.protocol_violation)
                {
                    recorder.record(if violation.is_malformed() {
                        FrameAccountingEvent::Malformed
                    } else {
                        FrameAccountingEvent::ProtocolViolation
                    });
                }
                match (buffer_source, deferred_resource_part_hash) {
                    (
                        IngressBufferSource::GrantSlot,
                        Some(DeferredResourcePartHash { plan, part }),
                    ) => {
                        let frame = lane
                            .take_peeked()
                            .expect("the deferred resource part retains its ingress grant");
                        owed_work.push_resource_part_hash_grant_slot(plan, source, frame, part);
                    }
                    (
                        IngressBufferSource::UnmaskScratch,
                        Some(DeferredResourcePartHash { plan, part }),
                    ) => {
                        owed_work.push_resource_part_hash_copy(plan, &unmask_scratch[part]);
                        lane.release();
                    }
                    (IngressBufferSource::GrantSlot | IngressBufferSource::UnmaskScratch, None) => {
                        lane.release();
                    }
                }
                processed_frames += 1;
                merge_wake_schedules_delta(
                    wake_schedules,
                    ingest_report.wake_schedules,
                    engine,
                    topology.interfaces.view(),
                );
                if !link_identity_barriers.is_empty()
                    && link_identity_barriers
                        .iter()
                        .any(|(blocked, _)| *blocked == source)
                {
                    break;
                }
            }
            {
                let inline_signs = crypto_pool.map_or(usize::MAX, |_| 0);
                for _ in 0..inline_signs {
                    let Some(sign) = link_signs.pop() else {
                        break;
                    };
                    inline_link_signs.push(sign);
                }
                if let Some(pool) = crypto_pool {
                    pool.submit_link_signs(link_signs);
                }
                for job in inline_link_signs.drain(..) {
                    match run_link_sign_job(job) {
                        LinkSignCompleted::ChannelAck(completed) => {
                            engine.resume_channel_ack_sign(completed, now, &mut |reaction| {
                                route_reaction(
                                    reaction,
                                    &mut topology.egress,
                                    &topology.ifacs,
                                    &mut topology.pacers,
                                    wire_scratch,
                                    now,
                                    &mut |journaled| journal.route(journaled),
                                );
                            });
                        }
                        LinkSignCompleted::Receipt(completed) => {
                            engine.resume_link_receipt_sign(completed, now, &mut |reaction| {
                                route_reaction(
                                    reaction,
                                    &mut topology.egress,
                                    &topology.ifacs,
                                    &mut topology.pacers,
                                    wire_scratch,
                                    now,
                                    &mut |journaled| journal.route(journaled),
                                );
                            });
                        }
                        LinkSignCompleted::Identify(completed) => {
                            let changed =
                                engine.resume_identify_sign(completed, now, &mut |reaction| {
                                    route_reaction(
                                        reaction,
                                        &mut topology.egress,
                                        &topology.ifacs,
                                        &mut topology.pacers,
                                        wire_scratch,
                                        now,
                                        &mut |journaled| journal.route(journaled),
                                    );
                                });
                            merge_wake_schedules_delta(
                                wake_schedules,
                                changed,
                                engine,
                                topology.interfaces.view(),
                            );
                        }
                    }
                }
            }
            if processed_frames == max_frames_total {
                break 'lanes;
            }
        }
        ready_lanes.retain(|source| {
            topology
                .inbound_lanes
                .iter_mut()
                .find(|(id, _)| id == source)
                .is_some_and(|(_, lane)| lane.try_peek().is_some())
        });
        if ready_lanes.len() > 1 {
            ready_lanes.rotate_left(1);
        }
        processed_frames
    }
}

pub(super) struct InboundContext<'a, S, H, J, P, A>
where
    S: StorageLayout,
    H: Host,
    J: for<'b> FnMut(Journaled<'b>),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    pub(super) engine: &'a mut EngineState<S>,
    pub(super) host: &'a mut H,
    pub(super) topology: &'a mut InterfaceTopology,
    pub(super) wire_scratch: &'a mut WireScratch,
    pub(super) journal: &'a mut JournalDispatch<J>,
    pub(super) crypto_pool: Option<&'a CryptoPool>,
    pub(super) packet_phy_store: Option<&'a InterfaceStore>,
    pub(super) wake_schedules: &'a mut WakeSchedules,
    pub(super) should_prove: &'a mut P,
    pub(super) should_accept_resource: &'a mut A,
    pub(super) max_frames_per_lane: usize,
    pub(super) max_frames_total: usize,
    pub(super) owed_work: &'a mut PendingOwedWork,
    pub(super) now: InstantMillis,
}

fn retain_packet_phy(
    store: Option<&InterfaceStore>,
    packet_hash: PacketHash,
    packet_phy: PacketPhyStats,
) {
    if packet_phy.is_empty() {
        return;
    }
    let Some(store) = store else {
        return;
    };
    store.remember_packet_phy(packet_hash, packet_phy);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{bytes_from_hex, RNS_1_4_2_ANNOUNCE};
    use crate::interfaces::{RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb};

    #[test]
    fn link_identity_verdict_blocks_only_its_ingress_lane_until_completion() {
        let blocked = InterfaceId::new([0xB1; 8]);
        let independent = InterfaceId::new([0xB2; 8]);
        let link_id = LinkId::new([0x51; 16]);
        let mut inbound = InboundDispatch::new(64);
        inbound.mark_ready(blocked);
        inbound.link_identity_barriers.push((blocked, link_id));

        assert!(!inbound.has_ready_lanes());

        inbound.mark_ready(independent);
        assert!(inbound.has_ready_lanes());

        inbound.ready_lanes.retain(|source| *source == blocked);
        inbound.release_link_identity_barrier(LinkId::new([0x52; 16]));
        assert!(!inbound.has_ready_lanes());

        inbound.release_link_identity_barrier(link_id);
        assert!(inbound.has_ready_lanes());
    }

    #[test]
    fn packet_phy_reuses_the_classified_wire_stable_packet_hash() {
        let store = InterfaceStore::new();
        let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let expected = PacketHash::of_wire_packet(&raw).expect("the fixture is a wire packet");
        let packet = ClassifiedInboundPacket::classify(InboundPacket {
            arrived_at: crate::engine::InstantMillis(7),
            source_interface: InterfaceId::new([0xC7; 8]),
            bytes: &mut raw,
        });
        let packet_hash = packet.packet_hash().expect("the packet was classified");
        let packet_phy = PacketPhyStats {
            rssi: Some(RssiDbm::new(-103)),
            snr: Some(SnrQuarterDb::new(-11)),
            quality: SignalQualityTenthsPercent::new(731),
        };

        retain_packet_phy(Some(&store), packet_hash, packet_phy);

        assert_eq!(packet_hash, expected);
        assert_eq!(store.packet_phy(packet_hash), Some(packet_phy));
    }
}
