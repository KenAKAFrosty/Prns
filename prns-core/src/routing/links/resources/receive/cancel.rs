//! The sender's mid-flight abort: `RESOURCE_ICL` concludes the transfer as failed by its hash.

use crate::engine::{EngineState, InstantMillis};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::ingress::{DataPacket, IgnoreReason, IngestPacketOutcome};
use crate::routing::links::resources::control::parse_cancel_plaintext;
use crate::routing::links::resources::ResourceFailureCause;
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::{DestinationType, PacketType};

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.4.0's link dispatch for `RESOURCE_ICL`. Sealed, and behind the duplicate filter like the advertisement.
    pub(crate) fn ingest_resource_cancel<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data.header.address,
            data.header.context,
            data.payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => {
                return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate)
            }
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::DecryptFailed);
        };
        let Ok(hash) = parse_cancel_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        if self.incoming_resources.lookup(&link_id, &hash).is_none() {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        }
        let settled_request = self.settle_response_claim(&link_id, &hash);
        self.retire_incoming_resource(&link_id, &hash);
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::IncomingResourceFailed {
            link_id,
            hash,
            cause: ResourceFailureCause::CancelledBySender,
            settled_request,
        }
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::{CommandId, Directive, EngineReaction};
    use crate::engine::{SendResourceFailure, Settlement};
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::resources::control::write_cancel_plaintext;
    use crate::routing::links::resources::receive::tests_support::*;
    use crate::routing::links::resources::ResourceHash;
    use crate::routing::links::resources::RESOURCE_HASH_LEN;
    use crate::routing::links::resources::{ResourceBody, ResourceMetadata, ResourceSend};
    use crate::wire::WireContext;
    use crate::wire::BROADCAST_MTU;

    fn four_part_setup() -> (
        EngineState<crate::engine::test_support::TestStorageLayout>,
        EngineState<crate::engine::test_support::TestStorageLayout>,
        ResourceHash,
    ) {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &data,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let hash = *receiver.incoming_resources.hash_at(0);
        (sender, receiver, hash)
    }

    fn sealed_cancel(hash: &ResourceHash, context: WireContext, iv: u8) -> std::vec::Vec<u8> {
        let mut plaintext = [0u8; RESOURCE_HASH_LEN];
        write_cancel_plaintext(hash, &mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            context,
            &plaintext,
            &[iv; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    #[test]
    fn the_senders_cancel_drops_the_receivers_transfer() {
        let (_, mut receiver, hash) = four_part_setup();
        let cancelled = feed(
            &mut receiver,
            &sealed_cancel(&hash, WireContext::ResourceInitiatorCancel, 0xE1),
            2_500,
        );
        assert_eq!(
            cancelled.failed,
            [(hash, ResourceFailureCause::CancelledBySender)],
        );
        assert!(receiver.incoming_resources.is_empty());

        let again = feed(
            &mut receiver,
            &sealed_cancel(&hash, WireContext::ResourceInitiatorCancel, 0xE2),
            2_600,
        );
        assert!(
            again.failed.is_empty(),
            "a cancel for nothing journals nothing"
        );
    }

    #[test]
    fn the_receivers_reject_settles_the_send_by_its_name() {
        let (mut sender, _, hash) = four_part_setup();
        let rejected = feed(
            &mut sender,
            &sealed_cancel(&hash, WireContext::ResourceReceiverCancel, 0xE3),
            2_500,
        );
        assert!(matches!(
            rejected.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::RejectedByPeer)),
            ),
        ));
        assert!(sender.outgoing_resources.is_empty());

        let unknown = feed(
            &mut sender,
            &sealed_cancel(
                &ResourceHash::new([0x5A; 32]),
                WireContext::ResourceReceiverCancel,
                0xE4,
            ),
            2_600,
        );
        assert!(unknown.settlements.is_empty());
    }
}
