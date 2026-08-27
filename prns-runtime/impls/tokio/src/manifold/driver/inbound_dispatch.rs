use tokio::sync::mpsc::UnboundedReceiver;

use crate::engine::{
    ClassifiedInboundPacket, DeferredCrypto, EngineState, IngestIo, Journaled, ProofIngest,
    ProofRequest, Settlement, WakeSchedules,
};
use crate::interfaces::{
    FrameAccountingEvent, IfacUnmaskError, InboundPacket, InterfaceId, PacketPhyStats,
};
use crate::manifold::kernel::merge_wake_schedules_delta;
use crate::manifold::Host;
use crate::routing::dedup::PacketHash;
use crate::routing::links::resources::ResourceOffer;
use crate::runtime::InterfaceStore;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::crypto_dispatch::dispatch_open_spans;
use super::crypto_pool::{CryptoJob, CryptoPool, EngineVerifyJob};
use super::egress::{ifac_for, route_reaction, WireScratch};
use super::interface_topology::InterfaceTopology;
use super::journal_delivery::JournalDispatch;

pub(super) struct InboundDispatch {
    ready_lanes: std::vec::Vec<InterfaceId>,
    unmask_scratch: std::boxed::Box<[u8]>,
}

impl InboundDispatch {
    pub(super) fn new(frame_capacity: usize) -> Self {
        Self {
            ready_lanes: std::vec::Vec::new(),
            unmask_scratch: std::vec![0u8; frame_capacity].into_boxed_slice(),
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
        } = context;
        let now = host.now();
        let Self {
            ready_lanes,
            unmask_scratch,
        } = self;
        macro_rules! journaled_sink {
            () => {
                |journaled| journal.route(journaled)
            };
        }
        macro_rules! reaction_sink {
            () => {
                |reaction| {
                    route_reaction(
                        reaction,
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        now,
                        &mut journaled_sink!(),
                    )
                }
            };
        }

        for &source in ready_lanes.iter() {
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
                if crypto_pool.is_some_and(|pool| !pool.has_queue_capacity(2)) {
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
                if let Some(pool) = crypto_pool {
                    if let (Some(proof_packet_hash), Some((address, payload))) =
                        (packet_hash, packet.proof())
                    {
                        if let Some(deferred) = engine.settle_receipt_proof_deferred(
                            payload,
                            &DestinationHash::from_address(address),
                            proof_packet_hash,
                            now,
                        ) {
                            let settlement = match deferred.ingest {
                                ProofIngest::SendSinglePacketDelivered { id, delivered } => {
                                    Some((id, Settlement::SendSinglePacket(Ok(delivered))))
                                }
                                ProofIngest::SendToLinkDelivered { id, delivered } => {
                                    Some((id, Settlement::SendToLink(Ok(delivered))))
                                }
                                ProofIngest::SendToChannelDelivered { .. }
                                | ProofIngest::Ignored => None,
                            };
                            if let Some((id, settlement)) = settlement {
                                pool.submit(CryptoJob::Verify(EngineVerifyJob {
                                    packet_hash: deferred.packet_hash,
                                    signing_key: deferred.signing_key,
                                    signature: deferred.signature,
                                    id,
                                    settlement,
                                    arrived_at: deferred.arrived_at,
                                }));
                            }
                            lane.release();
                            continue;
                        }
                    }
                }
                let ingest_report = match crypto_pool {
                    Some(pool) => {
                        let mut deferred_sign = None;
                        let mut deferred = DeferredCrypto::default();
                        let report = engine.ingest_classified_into_deferring_report(
                            packet,
                            IngestIo {
                                interfaces: topology.interfaces.view(),
                                now,
                                fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                                should_prove,
                                should_accept_resource,
                                sink: &mut reaction_sink!(),
                            },
                            &mut deferred_sign,
                            Some(&mut deferred),
                        );
                        if let Some(owed) = deferred_sign {
                            pool.submit(CryptoJob::Sign(owed));
                        }
                        match deferred {
                            DeferredCrypto::Empty => {}
                            DeferredCrypto::Decrypt(owed) => {
                                pool.submit(CryptoJob::Decrypt(owed));
                            }
                            DeferredCrypto::RatchetDecrypt(owed) => {
                                pool.submit(CryptoJob::DecryptWithRatchets(Box::new(owed)));
                            }
                            DeferredCrypto::LinkProofVerify(owed) => {
                                pool.submit(CryptoJob::VerifyLinkProof(owed));
                            }
                            DeferredCrypto::LinkProofSign(owed) => {
                                pool.submit(CryptoJob::SignLinkProof(owed));
                            }
                            DeferredCrypto::AnnounceVerify(owed) => {
                                pool.submit(CryptoJob::VerifyAnnounce(owed));
                            }
                        }
                        report
                    }
                    None => engine.ingest_classified_into_report(
                        packet,
                        IngestIo {
                            interfaces: topology.interfaces.view(),
                            now,
                            fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                            should_prove,
                            should_accept_resource,
                            sink: &mut reaction_sink!(),
                        },
                    ),
                };
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
                dispatch_open_spans(engine, crypto_pool);
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
