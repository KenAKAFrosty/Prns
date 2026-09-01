use tokio::sync::mpsc::UnboundedReceiver;

use crate::crypto::ed25519_sign;
use crate::engine::{
    ClassifiedInboundPacket, CryptoOwed, EngineReaction, EngineState, IngestIo, InstantMillis,
    Journaled, LinkReceiptSignCompleted, LinkReceiptSignOwed, OwedWork, ProofRequest,
    WakeSchedules,
};
use crate::interfaces::{
    FrameAccountingEvent, IfacUnmaskError, InboundPacket, InterfaceId, InterfaceIfac,
    PacketPhyStats,
};
use crate::manifold::wake_schedule::merge_wake_schedules_delta;
use crate::manifold::Host;
use crate::routing::dedup::PacketHash;
use crate::routing::links::resources::ResourceOffer;
use crate::runtime::InterfaceStore;
use crate::storage::StorageLayout;

use super::crypto_pool::CryptoPool;
use super::egress::{
    ifac_for, route_reaction, route_reaction_with_work, Egress, InterfacePacer, WireScratch,
};
use super::interface_topology::InterfaceTopology;
use super::journal_delivery::JournalDispatch;
use super::owed_work::PendingOwedWork;

fn route_ingress_reaction<J>(
    reaction: EngineReaction<'_, OwedWork<'_>>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    wire_scratch: &mut WireScratch,
    journal: &mut JournalDispatch<J>,
    owed_work: &mut PendingOwedWork,
    crypto_pool: Option<&CryptoPool>,
    link_receipt_signs: &mut std::vec::Vec<LinkReceiptSignOwed>,
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
                link_receipt_signs.push(owed);
            }
            OwedWork::Crypto(owed) => owed_work.push_crypto(owed),
            OwedWork::ResourceBuild(owed) => {
                owed_work.push(OwedWork::ResourceBuild(owed), crypto_pool);
            }
            OwedWork::ResourceOpen(owed) => owed_work.push_resource_open(owed, crypto_pool),
            OwedWork::ResourceDecompression(owed) => {
                owed_work.push(OwedWork::ResourceDecompression(owed), crypto_pool);
            }
        },
    );
}

pub(super) struct InboundDispatch {
    ready_lanes: std::vec::Vec<InterfaceId>,
    unmask_scratch: std::boxed::Box<[u8]>,
    link_receipt_signs: std::vec::Vec<LinkReceiptSignOwed>,
    inline_link_receipt_signs: std::vec::Vec<LinkReceiptSignOwed>,
}

// The minimum sixteen-job admission depth exposes at most fifteen receipt signs while retaining
// room for the next packet's possible second crypto job. Split that common backlog across the
// manifold (seven immediate proofs) and the pool (up to eight jobs). Seven remains a fixed latency
// bound on larger hosts; their additional backlog goes to their larger pool instead of blocking
// the manifold. Neither side waits for work, and a lone receipt stays entirely inline.
const INLINE_LINK_RECEIPT_TRANCHE: usize = 7;

impl InboundDispatch {
    pub(super) fn new(frame_capacity: usize) -> Self {
        Self {
            ready_lanes: std::vec::Vec::new(),
            unmask_scratch: std::vec![0u8; frame_capacity].into_boxed_slice(),
            link_receipt_signs: std::vec::Vec::new(),
            inline_link_receipt_signs: std::vec::Vec::new(),
        }
    }

    pub(super) fn has_ready_lanes(&self) -> bool {
        !self.ready_lanes.is_empty()
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

    pub(super) fn process<S, H, J, P, A>(&mut self, context: InboundContext<'_, S, H, J, P, A>)
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
            owed_work,
            now,
        } = context;
        let Self {
            ready_lanes,
            unmask_scratch,
            link_receipt_signs,
            inline_link_receipt_signs,
        } = self;
        for &source in ready_lanes.iter() {
            debug_assert!(link_receipt_signs.is_empty());
            debug_assert!(inline_link_receipt_signs.is_empty());
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
                if crypto_pool.is_some_and(|pool| {
                    !pool.has_queue_capacity(
                        owed_work
                            .len()
                            .saturating_add(link_receipt_signs.len())
                            .saturating_add(2),
                    )
                }) {
                    break;
                }
                let Some(slot) = lane.try_peek() else {
                    break;
                };
                let packet_phy = slot.packet_phy;
                let bytes = match ifac_for(&topology.ifacs, source) {
                    Some(entry) => {
                        match entry
                            .context
                            .try_unmask_inbound(slot.frame(), unmask_scratch)
                        {
                            Ok(clean_len) => &mut unmask_scratch[..clean_len],
                            Err(IfacUnmaskError::PacketTooShort) => {
                                if let Some(recorder) = &frame_accounting {
                                    recorder.record(FrameAccountingEvent::ProtocolViolation);
                                }
                                lane.release();
                                continue;
                            }
                            Err(
                                IfacUnmaskError::MissingFlag
                                | IfacUnmaskError::InvalidSignature
                                | IfacUnmaskError::OutputTooSmall { .. },
                            ) => {
                                lane.release();
                                continue;
                            }
                        }
                    }
                    None => slot.frame_mut(),
                };
                let packet = ClassifiedInboundPacket::classify(InboundPacket {
                    arrived_at: now,
                    source_interface: source,
                    bytes,
                });
                let packet_hash = packet.packet_hash();
                if let Some(packet_hash) = packet_hash {
                    retain_packet_phy(packet_phy_store, packet_hash, packet_phy);
                }
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
                                link_receipt_signs,
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
                lane.release();
                merge_wake_schedules_delta(
                    wake_schedules,
                    ingest_report.wake_schedules,
                    engine,
                    topology.interfaces.view(),
                );
            }
            {
                let inline_receipts = if crypto_pool.is_some() {
                    INLINE_LINK_RECEIPT_TRANCHE
                } else {
                    usize::MAX
                };
                for _ in 0..inline_receipts {
                    let Some(receipt) = link_receipt_signs.pop() else {
                        break;
                    };
                    inline_link_receipt_signs.push(receipt);
                }
                if let Some(pool) = crypto_pool {
                    pool.submit_link_receipts(link_receipt_signs);
                }
                for owed in inline_link_receipt_signs.drain(..) {
                    let signature = ed25519_sign(&owed.signing_secret, owed.packet_hash.as_bytes());
                    engine.resume_link_receipt_sign(
                        LinkReceiptSignCompleted {
                            target: owed.target,
                            link_id: owed.link_id,
                            packet_hash: owed.packet_hash,
                            signature,
                        },
                        now,
                        &mut |reaction| {
                            route_reaction(
                                reaction,
                                &mut topology.egress,
                                &topology.ifacs,
                                &mut topology.pacers,
                                wire_scratch,
                                now,
                                &mut |journaled| journal.route(journaled),
                            );
                        },
                    );
                }
            }
        }
        ready_lanes.retain(|source| {
            topology
                .inbound_lanes
                .iter_mut()
                .find(|(id, _)| id == source)
                .is_some_and(|(_, lane)| lane.try_peek().is_some())
        });
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
