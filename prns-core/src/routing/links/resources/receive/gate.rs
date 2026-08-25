//! RNS 1.4.2 `Resource.accept`: the strategy gate runs before a single part moves. The advertisement declares size and kind up front, so refusing is free.

#[cfg(feature = "runtime-metrics")]
use crate::engine::ResourceAdmissionEvent;
use crate::engine::{CommandId, CommandOutcome, SetResourceStrategy, SetResourceStrategyRejection};
use crate::engine::{Directive, EngineReaction, EngineState, InstantMillis};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::ingress::{DataPacket, IgnoreReason, IngestPacketOutcome};
use crate::routing::links::data::{link_data_frame_ceiling, write_link_packet};
use crate::routing::links::resources::advertisement::ResourceAdvertisement;
use crate::routing::links::resources::assembly::SegmentFit;
use crate::routing::links::resources::control::write_cancel_plaintext;
use crate::routing::links::resources::pending::{
    PendingResourceOffer, PendingResourceOfferError, QueuePendingResourceOfferOutcome,
};
use crate::routing::links::resources::table::{
    AcceptIncomingResourceError, AcceptedResource, IncomingResourceAdmission,
};
use crate::routing::links::resources::{
    resource_sdu, ResourceCompression, ResourceCorrelation, ResourceHash, ResourceStrategy,
    MAX_EFFICIENT_SIZE, PART_REQUEST_MAX_RETRIES, RESOURCE_HASH_LEN,
};
use crate::routing::links::table::{ActiveLinkLookup, LinkPhase};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::{DestinationType, PacketType, WireContext};

impl<S: StorageLayout> EngineState<S> {
    pub(crate) fn ingest_set_resource_strategy(
        &mut self,
        id: CommandId,
        set: SetResourceStrategy,
    ) -> CommandOutcome {
        use crate::routing::links::table::LinkActivationError;
        match self.links.set_resource_strategy(&set.link_id, set.strategy) {
            Ok(()) => CommandOutcome::ResourceStrategySet { id },
            Err(LinkActivationError::UnknownLink) => CommandOutcome::SetResourceStrategyRejected {
                id,
                rejection: SetResourceStrategyRejection::NoSuchLink,
            },
            Err(LinkActivationError::WrongPhase) => CommandOutcome::SetResourceStrategyRejected {
                id,
                rejection: SetResourceStrategyRejection::LinkNotActive,
            },
        }
    }

    /// RNS 1.4.2 `Resource.accept`; strategy refusals are silent, like a reference receiver that never accepts — except an `AcceptIf` decline, which answers with the reference's `Resource.reject`.
    /// Request-correlated and pending-response advertisements bypass the strategy, exactly the reference's `Link.receive` `RESOURCE_ADV` ladder: its strategy arms only ever see unsolicited resources, and a response naming no pending request drops before them.
    /// Accepting a response segment claims the pending request's timeout (the reference's `RECEIVING` flip); the transfer settles the row through every exit from here.
    /// Intentional deviation from reference: a split request advertisement stays behind the strategy — the reference accepts request resources unconditionally, but our inbound request dispatch reads the whole pack at once, which a split does not deliver. Under `AcceptIf` the decider judges it like any unsolicited offer, and an admitted request still faces the route policy at dispatch.
    /// Advertisements stay behind the duplicate filter (only `RESOURCE_REQ`/`RESOURCE`/`RESOURCE_PRF` are exempt in the reference).
    pub(crate) fn ingest_resource_advertisement<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(LinkPhase::Active {
            key,
            mtu,
            role,
            resource_strategy,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let resource_strategy = *resource_strategy;
        let mtu = *mtu;
        let responder_destination = match role {
            crate::routing::links::table::LinkRole::Responder { destination, .. } => {
                Some(*destination)
            }
            crate::routing::links::table::LinkRole::Initiator { .. } => None,
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
        let Ok(advertisement) = ResourceAdvertisement::parse(plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        if !advertisement.flags.encrypted
            || advertisement.hashmap.is_empty()
            || advertisement.total_segments == 0
            || advertisement.segment_index == 0
            || advertisement.segment_index > advertisement.total_segments
            || advertisement.flags.split != (advertisement.total_segments > 1)
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        let correlation = match (
            advertisement.flags.is_request,
            advertisement.flags.is_response,
            advertisement.request_id,
        ) {
            (true, false, Some(id)) => ResourceCorrelation::Request {
                id,
                response_timeout: Default::default(),
                maximum_response_bytes: Default::default(),
            },
            (false, true, Some(id)) => ResourceCorrelation::Response(id),
            _ => ResourceCorrelation::Unsolicited,
        };
        if let ResourceCorrelation::Request { .. } = correlation {
            let maximum_request_bytes = responder_destination
                .and_then(|destination| self.upstream_app_destinations.lookup_single(&destination))
                .map(|registered| registered.maximum_request_bytes)
                .unwrap_or_default();
            if !maximum_request_bytes.allows(advertisement.data_bytes) {
                return IngestPacketOutcome::ResourceTooLarge {
                    link_id,
                    hash: advertisement.hash,
                    settled_request: None,
                };
            }
        }
        if let ResourceCorrelation::Response(id) = correlation {
            let Some(maximum_response_bytes) = self.receipts.pending_request_response_limit(id)
            else {
                return IngestPacketOutcome::Ignored(IgnoreReason::UnmatchedResponse);
            };
            if !maximum_response_bytes.allows(advertisement.data_bytes) {
                let settled_request = self
                    .receipts
                    .settle_by_request_id(id)
                    .map(|proven| proven.command_id);
                return IngestPacketOutcome::ResourceTooLarge {
                    link_id,
                    hash: advertisement.hash,
                    settled_request,
                };
            }
        }
        let bypasses_strategy = match correlation {
            ResourceCorrelation::Response(_) => true,
            ResourceCorrelation::Request { .. } => advertisement.total_segments == 1,
            ResourceCorrelation::Unsolicited => false,
        };
        let policy = if bypasses_strategy {
            GatePolicy::Admit {
                max_uncompressed_bytes: (MAX_EFFICIENT_SIZE as u64)
                    .saturating_mul(advertisement.total_segments),
                accept_compressed: true,
            }
        } else {
            match resource_strategy {
                ResourceStrategy::Accept {
                    max_uncompressed_bytes,
                    accept_compressed,
                } => GatePolicy::Admit {
                    max_uncompressed_bytes,
                    accept_compressed,
                },
                ResourceStrategy::AcceptIf => GatePolicy::OfferToApp,
                ResourceStrategy::AcceptNone => {
                    return IngestPacketOutcome::Ignored(IgnoreReason::StrategyDeclined)
                }
            }
        };
        let compression = ResourceCompression::from_wire_flag(advertisement.flags.compressed);
        if let GatePolicy::Admit {
            accept_compressed: false,
            ..
        } = policy
        {
            if compression == ResourceCompression::Bz2 {
                return IngestPacketOutcome::Ignored(IgnoreReason::StrategyDeclined);
            }
        }
        let multi_segment = advertisement.total_segments > 1;
        if multi_segment
            && advertisement.segment_index > 1
            && self.incoming_assemblies.fit(
                &link_id,
                &advertisement.original_hash,
                advertisement.segment_index,
            ) == SegmentFit::Unexpected
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        if let GatePolicy::Admit {
            max_uncompressed_bytes,
            ..
        } = policy
        {
            if advertisement.data_bytes > max_uncompressed_bytes {
                return IngestPacketOutcome::Ignored(IgnoreReason::StrategyDeclined);
            }
        }
        let Ok(sealed_transfer_bytes) = usize::try_from(advertisement.transfer_bytes) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        let sdu = resource_sdu(mtu);
        let part_count = sealed_transfer_bytes.div_ceil(sdu);
        if part_count == 0 {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        let accepted = AcceptedResource {
            hash: advertisement.hash,
            salt_nonce: advertisement.salt_nonce,
            compression,
            has_metadata: advertisement.flags.has_metadata,
            uncompressed_data_bytes: advertisement.data_bytes,
            segment_index: advertisement.segment_index,
            total_segment_count: advertisement.total_segments,
            sealed_transfer_bytes,
            part_count,
            sdu,
            correlation,
            initial_names: advertisement.hashmap,
        };
        if self
            .pending_resource_offers
            .contains(&link_id, &accepted.hash)
        {
            self.links.note_inbound(&link_id, arrived_at);
            return IngestPacketOutcome::ResourceAdmissionPending;
        }
        if matches!(&policy, GatePolicy::OfferToApp)
            && self.incoming_resources.admission_for(&link_id, accepted)
                == IncomingResourceAdmission::AlreadyReceiving
        {
            self.links.note_inbound(&link_id, arrived_at);
            return IngestPacketOutcome::OwesResourcePull {
                link_id,
                hash: accepted.hash,
            };
        }
        match policy {
            GatePolicy::Admit { .. } => self.admit_or_queue_accepted_resource(
                link_id,
                advertisement.original_hash,
                accepted,
                arrived_at,
            ),
            GatePolicy::OfferToApp => IngestPacketOutcome::ResourceOffered {
                link_id,
                original_hash: advertisement.original_hash,
                accepted,
            },
        }
    }

    pub(crate) fn admit_or_queue_accepted_resource(
        &mut self,
        link_id: LinkId,
        original_hash: ResourceHash,
        accepted: AcceptedResource<'_>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        match self.incoming_resources.admission_for(&link_id, accepted) {
            IncomingResourceAdmission::Available | IncomingResourceAdmission::AlreadyReceiving => {
                self.admit_accepted_resource(link_id, original_hash, accepted, arrived_at)
            }
            IncomingResourceAdmission::TemporarilyFull => {
                let frozen_link_rtt = match self.links.phase_for(&link_id) {
                    Some(LinkPhase::Active { rtt, .. }) => *rtt,
                    _ => return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch),
                };
                let pending = match PendingResourceOffer::try_from_accepted(
                    link_id,
                    original_hash,
                    accepted,
                    arrived_at,
                    frozen_link_rtt,
                ) {
                    Ok(pending) => pending,
                    Err(
                        PendingResourceOfferError::BufferShape(_)
                        | PendingResourceOfferError::PartCountMismatch
                        | PendingResourceOfferError::RequestSettingsNotApplicable
                        | PendingResourceOfferError::HashmapTooLong
                        | PendingResourceOfferError::HashmapRagged
                        | PendingResourceOfferError::HashmapBeyondPartCount,
                    ) => return IngestPacketOutcome::Ignored(IgnoreReason::Malformed),
                };
                let outcome = self.pending_resource_offers.queue(pending);
                self.links.note_inbound(&link_id, arrived_at);
                match outcome {
                    QueuePendingResourceOfferOutcome::Queued => {
                        #[cfg(feature = "runtime-metrics")]
                        self.record_resource_admission_event(ResourceAdmissionEvent::Queued);
                        IngestPacketOutcome::ResourceAdmissionPending
                    }
                    QueuePendingResourceOfferOutcome::RetryCoalesced => {
                        IngestPacketOutcome::ResourceAdmissionPending
                    }
                    QueuePendingResourceOfferOutcome::TableFull => self.resource_capacity_rejected(
                        link_id,
                        accepted.hash,
                        accepted.correlation,
                    ),
                }
            }
            IncomingResourceAdmission::Impossible => {
                self.links.note_inbound(&link_id, arrived_at);
                self.resource_capacity_rejected(link_id, accepted.hash, accepted.correlation)
            }
            IncomingResourceAdmission::Malformed => {
                IngestPacketOutcome::Ignored(IgnoreReason::Malformed)
            }
        }
    }

    fn resource_capacity_rejected(
        &mut self,
        link_id: LinkId,
        hash: ResourceHash,
        correlation: ResourceCorrelation,
    ) -> IngestPacketOutcome<'static> {
        #[cfg(feature = "runtime-metrics")]
        self.record_resource_admission_event(ResourceAdmissionEvent::Rejected);
        let settled_request = match correlation {
            ResourceCorrelation::Response(request_id) => self
                .receipts
                .settle_by_request_id(request_id)
                .map(|receipt| receipt.command_id),
            ResourceCorrelation::Request { .. } | ResourceCorrelation::Unsolicited => None,
        };
        IngestPacketOutcome::ResourceCapacityRejected {
            link_id,
            hash,
            settled_request,
        }
    }

    pub(crate) fn admit_accepted_resource(
        &mut self,
        link_id: LinkId,
        original_hash: ResourceHash,
        accepted: AcceptedResource<'_>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let hash = accepted.hash;
        let correlation = accepted.correlation;
        let segment_index = accepted.segment_index;
        let total_segment_count = accepted.total_segment_count;
        let inherited = match self.links.phase_for(&link_id) {
            Some(LinkPhase::Active {
                last_resource_window,
                last_resource_eifr,
                ..
            }) => (
                last_resource_window.map(core::num::NonZeroUsize::get),
                *last_resource_eifr,
            ),
            _ => (None, None),
        };
        let index = match self.incoming_resources.accept(link_id, accepted) {
            Ok(index) => index,
            Err(
                AcceptIncomingResourceError::TableFull
                | AcceptIncomingResourceError::TransferTooLarge
                | AcceptIncomingResourceError::TooManyParts,
            ) => return IngestPacketOutcome::Ignored(IgnoreReason::CapacityExhausted),
            Err(AcceptIncomingResourceError::AlreadyReceiving) => {
                // RNS 1.4.2 sends an advertisement immediately before registering the outgoing
                // resource, so a fast first pull can reach it in that gap and be discarded. Its
                // watchdog rebuilds each advertisement retry with a fresh IV; packet dedup sees
                // a new frame, while this table recognizes the active resource. Refreshing the
                // pull here closes that race without admitting a second transfer.
                return IngestPacketOutcome::OwesResourcePull { link_id, hash };
            }
            Err(
                AcceptIncomingResourceError::EmptyTransfer
                | AcceptIncomingResourceError::SduTooSmall
                | AcceptIncomingResourceError::PartCountMismatch
                | AcceptIncomingResourceError::HashmapTooLong
                | AcceptIncomingResourceError::HashmapRagged
                | AcceptIncomingResourceError::HashmapBeyondPartCount,
            ) => return IngestPacketOutcome::Ignored(IgnoreReason::Malformed),
        };
        {
            let state = self.incoming_resources.state_mut(index);
            state.retries_left = PART_REQUEST_MAX_RETRIES;
            if let Some(window) = inherited.0 {
                state.window = window;
            }
            state.inherited_eifr = inherited.1;
        }
        if total_segment_count > 1 && segment_index == 1 {
            self.incoming_assemblies
                .begin(link_id, original_hash, total_segment_count);
        }
        if let ResourceCorrelation::Response(id) = correlation {
            self.receipts.claim_request_for_transfer(id);
        }
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::OwesResourcePull { link_id, hash }
    }

    /// RNS 1.4.2 `Resource.reject`: the declined segment's bare hash, sealed under the link key, context `RESOURCE_RCL`.
    pub(crate) fn reject_offered_resource<F>(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let ActiveLinkLookup::Active(link) = self.links.active_view(link_id) else {
            return;
        };
        let key = link.key;
        let mtu = link.mtu;
        let fire_on = link.attached_interface;
        let mut reject_iv = [0u8; 16];
        fill_entropy(&mut reject_iv);
        let mut reject_plaintext = [0u8; RESOURCE_HASH_LEN];
        if write_cancel_plaintext(hash, &mut reject_plaintext).is_ok() {
            let mut wrote = false;
            {
                let mut fill = |slot: &mut [u8]| -> Option<usize> {
                    let wire_bytes = write_link_packet(
                        link_id,
                        key,
                        mtu,
                        WireContext::ResourceReceiverCancel,
                        &reject_plaintext,
                        &reject_iv,
                        slot,
                    )
                    .ok()?;
                    wrote = true;
                    Some(wire_bytes)
                };
                sink(EngineReaction::Directive(Directive::EmitFrame {
                    target: fire_on,
                    size_hint: link_data_frame_ceiling(RESOURCE_HASH_LEN),
                    fill: &mut fill,
                }));
            }
            if wrote {
                self.links.note_outbound(link_id, now);
            }
        }
    }
}

/// What the strategy ladder resolved to: declarative bounds admit inline, `AcceptIf` hands the validated offer up for the host decider's verdict.
enum GatePolicy {
    Admit {
        max_uncompressed_bytes: u64,
        accept_compressed: bool,
    },
    OfferToApp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{routable_descriptor, test_entropy_bytes, TestStorageLayout};
    use crate::engine::{Directive, EngineReaction};
    use crate::engine::{IssuedCommand, PrnsCommand, SetResourceStrategyFailure, Settlement};
    use crate::interfaces::AttachedInterfaces;
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::resources::receive::tests_support::*;
    use crate::routing::links::resources::ResourceHash;
    use crate::routing::links::resources::{ResourceBody, ResourceMetadata, ResourceSend};
    use crate::wire::{WireContext, BROADCAST_MTU};

    use crate::engine::Journaled;
    use crate::routing::links::resources::control::parse_part_request_plaintext;
    use crate::wire::WirePacketHeader;

    #[test]
    fn the_default_strategy_ignores_advertisements() {
        let mut receiver = engine_with_active_link();
        let capture = feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        assert!(capture.frames.is_empty());
        assert!(receiver.incoming_resources.is_empty());
        assert!(receiver.pending_resource_offers.is_empty());
    }

    #[test]
    fn a_response_resource_settles_its_request_despite_the_default_strategy() {
        use crate::crypto::Ed25519PublicKey;
        use crate::engine::test_support::filled_frame;
        use crate::identity::IdentitySigningPublicKey;
        use crate::routing::dedup::PacketHash;
        use crate::routing::delivery::receipts::{OutstandingReceipt, ReceiptKind};
        use crate::routing::links::request::RequestId;
        use crate::wire::{DestinationType, PacketType, WireContext};

        let mut receiver = engine_with_active_link();
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &link_id().to_address(),
            WireContext::Request,
            &b"the request we sent"[..],
        );
        let request_id = RequestId::of_packet(&packet_hash);
        receiver.receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: CommandId(42),
            kind: ReceiptKind::SendRequest {
                maximum_response_bytes: crate::units::ByteLimit::Unlimited,
            },
            peer_signing_key: IdentitySigningPublicKey::new(Ed25519PublicKey([0x99; 32])),
            sent_at: InstantMillis(1_800),
            timeout_at: InstantMillis(20_000),
        });

        let data = four_part_payload();
        let mut sender = engine_with_active_link();
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
                correlation: crate::routing::links::resources::ResourceCorrelation::Response(
                    request_id,
                ),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let advertisement = advertisement.expect("the responder advertises its response resource");

        let pull = feed(&mut receiver, &advertisement, 2_000);
        assert_eq!(
            pull.frames.len(),
            1,
            "a response to a request we sent is pulled, default strategy notwithstanding",
        );
        assert!(!receiver.incoming_resources.is_empty());

        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(serve.frames.len(), 4, "the peer streams every part");

        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.settlements.is_empty() || !capture.received.is_empty() {
                conclusion = Some(capture);
            }
        }
        let conclusion = conclusion.expect("the last part concludes the response");
        assert!(
            conclusion.received.is_empty(),
            "a response settles its request, not a bare ResourceReceived",
        );
        assert!(matches!(
            conclusion.settlements[0],
            (CommandId(42), Settlement::SendRequest(Ok(_))),
        ));
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn an_oversized_response_resource_is_rejected_before_allocation() {
        use crate::engine::test_support::filled_frame;
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::units::ByteLimit;

        let data = four_part_payload();
        let mut receiver = engine_with_active_link();
        let request_id = track_pending_request_with_limit(
            &mut receiver,
            CommandId(42),
            1_800,
            20_000,
            ByteLimit::Maximum(data.len() as u64 - 1),
        );
        let mut sender = engine_with_active_link();
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
                correlation: ResourceCorrelation::Response(request_id),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let capture = feed(
            &mut receiver,
            &advertisement.expect("the responder advertises"),
            2_000,
        );
        assert_eq!(capture.frames.len(), 1);
        let (header, _) = WirePacketHeader::parse(&capture.frames[0].1).unwrap();
        assert_eq!(header.context, WireContext::ResourceReceiverCancel);
        assert_eq!(
            capture.settlements,
            std::vec![(
                CommandId(42),
                Settlement::SendRequest(Err(crate::engine::SendRequestFailure::ResponseTooLarge)),
            )],
        );
        assert!(receiver.incoming_resources.is_empty());
        assert!(!receiver.receipts.has_pending_request(request_id));
    }

    #[test]
    fn a_request_resource_dispatches_a_request_despite_the_default_strategy() {
        use crate::engine::test_support::filled_frame;
        use crate::routing::links::request::{write_request_plaintext, RequestId};
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::routing::request_handlers::RequestPathHash;

        let path_hash = RequestPathHash::new([0x44; 16]);
        let request_data = b"a request too fat for one packet, ".repeat(40);
        let mut packed = std::vec![0u8; request_data.len() + 64];
        let plain_len =
            write_request_plaintext(InstantMillis(1_400), &path_hash, &request_data, &mut packed)
                .unwrap();
        let packed_request = &packed[..plain_len];
        let request_id = RequestId::of_request_data(packed_request);

        let mut sender = engine_with_active_link();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: packed_request,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Request {
                    id: request_id,
                    response_timeout: Default::default(),
                    maximum_response_bytes: Default::default(),
                },
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let advertisement = advertisement.expect("the peer advertises its request resource");

        let mut receiver = engine_with_responding_link();
        receiver
            .request_handlers
            .register(
                RESPONDER_DESTINATION,
                path_hash,
                crate::routing::request_handlers::RequestPolicy::AllowAll,
            )
            .unwrap();
        let pull = feed(&mut receiver, &advertisement, 2_000);
        assert_eq!(
            pull.frames.len(),
            1,
            "an inbound request is accepted and pulled, default strategy notwithstanding",
        );

        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.requests.is_empty() || !capture.received.is_empty() {
                conclusion = Some(capture);
            }
        }
        let conclusion = conclusion.expect("the last part concludes the request");
        assert!(
            conclusion.received.is_empty(),
            "a request resource dispatches a RequestReceived, not a bare ResourceReceived",
        );
        assert_eq!(conclusion.requests.len(), 1);
        assert_eq!(conclusion.requests[0].0, request_id);
        assert_eq!(conclusion.requests[0].1, request_data);
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn an_oversized_request_resource_is_rejected_before_allocation() {
        use crate::crypto::ratchets::RatchetPolicy;
        use crate::engine::test_support::filled_frame;
        use crate::identity::IdentityHash;
        use crate::routing::links::request::{write_request_plaintext, RequestId};
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::routing::links::table::RespondingLink;
        use crate::routing::request_handlers::RequestPathHash;
        use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
        use crate::units::{ByteLimit, RttMillis};

        let path_hash = RequestPathHash::new([0x44; 16]);
        let request_data = b"a request too fat for one packet, ".repeat(40);
        let mut packed = std::vec![0u8; request_data.len() + 64];
        let plain_len =
            write_request_plaintext(InstantMillis(1_400), &path_hash, &request_data, &mut packed)
                .unwrap();
        let packed_request = &packed[..plain_len];
        let request_id = RequestId::of_request_data(packed_request);

        let mut sender = engine_with_active_link();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: packed_request,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Request {
                    id: request_id,
                    response_timeout: Default::default(),
                    maximum_response_bytes: Default::default(),
                },
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let mut receiver = EngineState::<TestStorageLayout>::default();
        let identity = IdentityHash::new([0x77; 16]);
        let destination = receiver
            .upstream_app_destinations
            .register_single(
                &identity,
                "limits",
                &["request"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        assert!(receiver.set_maximum_request_bytes(
            &destination,
            ByteLimit::Maximum(packed_request.len() as u64 - 1),
        ));
        receiver
            .links
            .track_responding(RespondingLink {
                link_id: link_id(),
                key: link_key(),
                requested_at: InstantMillis(500),
                timeout_at: InstantMillis(5_000),
                mtu: BROADCAST_MTU,
                initiator_signing: crate::crypto::Ed25519PublicKey([0x99; 32]),
                destination,
                identity,
                proof_strategy: ProofStrategy::ProveNone,
            })
            .unwrap();
        receiver
            .links
            .activate_responding(
                &link_id(),
                RttMillis::new(250),
                lane(),
                InstantMillis(1_000),
            )
            .unwrap();

        let capture = feed(
            &mut receiver,
            &advertisement.expect("the requester advertises"),
            2_000,
        );
        assert_eq!(capture.frames.len(), 1);
        let (header, _) = WirePacketHeader::parse(&capture.frames[0].1).unwrap();
        assert_eq!(header.context, WireContext::ResourceReceiverCancel);
        assert!(capture.settlements.is_empty());
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn a_big_request_rides_a_resource_that_books_its_pending_row() {
        use crate::engine::test_support::filled_frame;
        use crate::routing::links::request::{write_request_plaintext, RequestId};
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::routing::request_handlers::RequestPathHash;

        let path_hash = RequestPathHash::new([0x55; 16]);
        let request_data = b"a request too fat for a packet, ".repeat(40);
        let mut packed = std::vec![0u8; request_data.len() + 64];
        let plain_len =
            write_request_plaintext(InstantMillis(1_400), &path_hash, &request_data, &mut packed)
                .unwrap();
        let packed_request = &packed[..plain_len];
        let request_id = RequestId::of_request_data(packed_request);

        let mut requester = engine_with_active_link();
        assert!(
            requester.request_fits_packet(&link_id(), b"small enough"),
            "a tiny request rides a packet",
        );
        assert!(
            !requester.request_fits_packet(&link_id(), packed_request),
            "a >MDU request does not fit a packet — it must ride a resource",
        );

        requester.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(55),
                link_id: link_id(),
                body: ResourceBody {
                    data: packed_request,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Request {
                    id: request_id,
                    response_timeout: Default::default(),
                    maximum_response_bytes: Default::default(),
                },
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    let _ = filled_frame(fill);
                }
            },
        );

        assert!(
            requester.receipts.has_pending_request(request_id),
            "the request resource books the pending row its response will settle",
        );
    }

    fn crafted_split_advertisement(segment_index: u64, total_segments: u64) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::advertisement::{
            ResourceAdvertisement, ResourceFlags,
        };
        use crate::routing::links::resources::SaltNonce;
        use crate::wire::BROADCAST_MTU;
        let part_count = 4usize;
        let names = [0xCDu8; 16];
        let advertisement = ResourceAdvertisement {
            transfer_bytes: (part_count * 464) as u64,
            data_bytes: 1_000,
            part_count: part_count as u64,
            hash: ResourceHash::new([0xAB; 32]),
            salt_nonce: SaltNonce::new([0x61; 4]),
            original_hash: ResourceHash::new([0xAB; 32]),
            segment_index,
            total_segments,
            request_id: None,
            flags: ResourceFlags {
                encrypted: true,
                compressed: false,
                split: total_segments > 1,
                is_request: false,
                is_response: false,
                has_metadata: false,
            },
            hashmap: &names,
        };
        let mut plaintext = [0u8; 431];
        let plaintext_len = advertisement.write(&mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[0xD1; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_bytes].to_vec()
    }

    #[test]
    fn an_unmatched_response_advertisement_drops_before_the_strategy() {
        use crate::engine::test_support::filled_frame;
        use crate::routing::dedup::PacketHash;
        use crate::routing::links::request::RequestId;

        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let never_sent = RequestId::of_packet(&PacketHash::new([0x5C; 32]));

        let mut sender = engine_with_active_link();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &four_part_payload(),
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Response(
                    never_sent,
                ),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let capture = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        assert!(capture.frames.is_empty());
        assert!(
            receiver.incoming_resources.is_empty(),
            "a response naming no pending request drops before the strategy arms, like the reference's ladder",
        );
    }

    fn crafted_split_response_advertisement(
        request_id: crate::routing::links::request::RequestId,
        data_bytes: u64,
        total_segments: u64,
    ) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::advertisement::{
            ResourceAdvertisement, ResourceFlags,
        };
        use crate::routing::links::resources::SaltNonce;
        use crate::wire::BROADCAST_MTU;
        let part_count = 4usize;
        let names = [0xCDu8; 16];
        let advertisement = ResourceAdvertisement {
            transfer_bytes: (part_count * 464) as u64,
            data_bytes,
            part_count: part_count as u64,
            hash: ResourceHash::new([0xAB; 32]),
            salt_nonce: SaltNonce::new([0x61; 4]),
            original_hash: ResourceHash::new([0xAB; 32]),
            segment_index: 1,
            total_segments,
            request_id: Some(request_id),
            flags: ResourceFlags {
                encrypted: true,
                compressed: false,
                split: total_segments > 1,
                is_request: false,
                is_response: true,
                has_metadata: false,
            },
            hashmap: &names,
        };
        let mut plaintext = [0u8; 431];
        let plaintext_len = advertisement.write(&mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[0xD2; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_bytes].to_vec()
    }

    #[test]
    fn the_bypass_cap_scales_to_the_chains_declared_segments() {
        let mut receiver = engine_with_active_link();
        let request_id = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);

        let over_one_segment = (crate::routing::links::resources::MAX_EFFICIENT_SIZE as u64) + 1;
        let refused = feed(
            &mut receiver,
            &crafted_split_response_advertisement(request_id, over_one_segment, 1),
            2_000,
        );
        assert!(
            refused.frames.is_empty() && receiver.incoming_resources.is_empty(),
            "a single segment declaring more than a conforming segment carries stays refused",
        );

        let accepted = feed(
            &mut receiver,
            &crafted_split_response_advertisement(request_id, over_one_segment, 2),
            2_100,
        );
        assert_eq!(
            accepted.frames.len(),
            1,
            "a two-segment response declaring the whole transfer's length is admitted",
        );
    }

    #[test]
    fn a_split_advertisement_opens_a_chain_keyed_by_original_hash() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        feed(&mut receiver, &crafted_split_advertisement(1, 3), 2_000);
        assert!(!receiver.incoming_resources.is_empty());
        assert_eq!(
            receiver.incoming_assemblies.original_hash(&link_id()),
            Some(ResourceHash::new([0xAB; 32])),
        );
    }

    #[test]
    fn a_segment_index_past_the_chain_length_is_refused() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        feed(&mut receiver, &crafted_split_advertisement(3, 2), 2_000);
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(receiver.incoming_assemblies.original_hash(&link_id()), None);
    }

    #[test]
    fn the_strategy_command_demands_an_active_link() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut settled = std::vec::Vec::new();
        engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: PrnsCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::AcceptNone,
                }),
            },
            AttachedInterfaces::new(&[routable_descriptor(lane())]),
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        assert!(matches!(
            settled[0],
            (
                CommandId(9),
                Settlement::SetResourceStrategy(Err(SetResourceStrategyFailure::Rejected(
                    SetResourceStrategyRejection::NoSuchLink,
                ))),
            ),
        ));
    }

    #[test]
    fn an_accepted_advertisement_registers_and_pulls_the_first_window() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let capture = feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );

        assert_eq!(capture.frames.len(), 1);
        let (target, frame) = &capture.frames[0];
        assert_eq!(*target, lane());
        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.context, WireContext::ResourceRequest);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let request = parse_part_request_plaintext(opened).unwrap();
        assert_eq!(request.last_known_map_hash, None);

        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &request.hash)
            .expect("the transfer is registered");
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.part_count, 4);
        assert_eq!(state.outstanding_part_count, 4);
        assert!(!state.waiting_for_hmu);
        assert_eq!(
            request.requested,
            receiver.incoming_resources.names_flat(index),
            "the first window asks for every part it can name",
        );
    }

    #[test]
    fn policy_refusals_are_silent() {
        let compressed = advertisement_frame(
            &b"reticulum resources ride the link ".repeat(40),
            Some(&bytes_from_hex(CASE1_BZ2)),
        );

        let mut no_compression = engine_with_active_link();
        let mut settled = std::vec::Vec::new();
        no_compression.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: PrnsCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_bytes: 1 << 20,
                        accept_compressed: false,
                    },
                }),
            },
            AttachedInterfaces::new(&[routable_descriptor(lane())]),
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        let capture = feed(&mut no_compression, &compressed, 2_000);
        assert!(capture.frames.is_empty());
        assert!(no_compression.incoming_resources.is_empty());

        let mut tiny_cap = engine_with_active_link();
        let mut settled = std::vec::Vec::new();
        tiny_cap.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: PrnsCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_bytes: 100,
                        accept_compressed: true,
                    },
                }),
            },
            AttachedInterfaces::new(&[routable_descriptor(lane())]),
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        let capture = feed(
            &mut tiny_cap,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        assert!(capture.frames.is_empty());
        assert!(tiny_cap.incoming_resources.is_empty());
    }

    #[test]
    fn an_accept_if_verdict_admits_and_pulls_the_first_window() {
        use crate::routing::links::resources::ResourceOffer;

        let mut receiver = engine_with_active_link();
        let remote_identity = crate::identity::IdentityHash::new([0xA7; 16]);
        receiver.links.note_identified(&link_id(), remote_identity);
        set_strategy(&mut receiver, ResourceStrategy::AcceptIf);
        let payload = four_part_payload();
        let mut offers = std::vec::Vec::new();
        let capture = feed_judged(
            &mut receiver,
            &advertisement_frame(&payload, None),
            2_000,
            &mut |offer: &ResourceOffer| {
                offers.push(*offer);
                true
            },
        );

        assert_eq!(
            capture.frames.len(),
            1,
            "the accepted offer pulls its first window",
        );
        let (header, _) = WirePacketHeader::parse(&capture.frames[0].1).unwrap();
        assert_eq!(header.context, WireContext::ResourceRequest);
        assert!(!receiver.incoming_resources.is_empty());

        let judged_hash = offers.first().expect("the decider saw the offer").hash;
        let judged_sealed_len = offers[0].sealed_transfer_bytes;
        assert!(judged_sealed_len >= payload.len());
        assert_eq!(
            offers,
            [ResourceOffer {
                link_id: link_id(),
                remote_identity: Some(remote_identity),
                hash: judged_hash,
                uncompressed_data_bytes: payload.len() as u64,
                sealed_transfer_bytes: judged_sealed_len,
                part_count: 4,
                segment_index: 1,
                total_segment_count: 1,
                compression: ResourceCompression::Uncompressed,
                has_metadata: false,
            }],
        );
    }

    #[test]
    fn an_accept_if_decline_answers_with_the_references_reject() {
        use crate::routing::links::resources::control::parse_cancel_plaintext;
        use crate::routing::links::resources::ResourceOffer;

        let mut receiver = engine_with_active_link();
        set_strategy(&mut receiver, ResourceStrategy::AcceptIf);
        let mut judged_hash = None;
        let capture = feed_judged(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
            &mut |offer: &ResourceOffer| {
                judged_hash = Some(offer.hash);
                false
            },
        );

        assert_eq!(capture.frames.len(), 1, "the decline answers on the wire");
        let (header, payload) = WirePacketHeader::parse(&capture.frames[0].1).unwrap();
        assert_eq!(header.context, WireContext::ResourceReceiverCancel);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let rejected_hash = parse_cancel_plaintext(opened).unwrap();
        assert_eq!(Some(rejected_hash), judged_hash);
        assert!(receiver.incoming_resources.is_empty());
        assert!(receiver.pending_resource_offers.is_empty());
    }

    #[test]
    fn a_declined_offer_settles_the_senders_transfer_as_rejected() {
        use crate::engine::SendResourceFailure;
        use crate::routing::links::resources::ResourceOffer;

        let mut sender = engine_with_active_link();
        let advertisement = advertise_from(&mut sender, &four_part_payload(), None);

        let mut receiver = engine_with_active_link();
        set_strategy(&mut receiver, ResourceStrategy::AcceptIf);
        let decline = feed_judged(
            &mut receiver,
            &advertisement,
            2_000,
            &mut |_: &ResourceOffer| false,
        );
        assert_eq!(decline.frames.len(), 1);

        let settle = feed(&mut sender, &decline.frames[0].1, 2_100);
        assert!(matches!(
            settle.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::RejectedByPeer)),
            ),
        ));
    }

    #[test]
    fn a_bypassing_response_never_consults_the_decider() {
        use crate::engine::test_support::filled_frame;
        use crate::routing::links::resources::ResourceOffer;

        let mut receiver = engine_with_active_link();
        set_strategy(&mut receiver, ResourceStrategy::AcceptIf);
        let request_id = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);

        let mut sender = engine_with_active_link();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &four_part_payload(),
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Response(
                    request_id,
                ),
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let pull = feed_judged(
            &mut receiver,
            &advertisement.unwrap(),
            2_000,
            &mut |_: &ResourceOffer| panic!("the ladder bypasses the strategy for responses"),
        );
        assert_eq!(pull.frames.len(), 1);
        assert!(!receiver.incoming_resources.is_empty());
    }

    #[test]
    fn a_reencrypted_advertisement_retry_refreshes_only_the_active_pull() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let advertisement = advertisement_frame(&four_part_payload(), None);
        let first = feed(&mut receiver, &advertisement, 2_000);
        let (header, payload) = WirePacketHeader::parse(&advertisement).unwrap();
        let mut sealed = payload.to_vec();
        let plaintext = link_key().open_in_place(&mut sealed).unwrap();
        let mut refreshed = std::vec![0u8; BROADCAST_MTU];
        let refreshed_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            header.context,
            plaintext,
            &[0xE3; 16],
            &mut refreshed,
        )
        .unwrap();
        refreshed.truncate(refreshed_len);
        let reencrypted_retry = feed(&mut receiver, &refreshed, 2_100);
        let identical_retry = feed(&mut receiver, &refreshed, 2_200);

        assert_eq!(first.frames.len(), 1);
        assert_eq!(reencrypted_retry.frames.len(), 1);
        assert!(identical_retry.frames.is_empty());
        assert_eq!(receiver.incoming_resources.len(), 1);

        let hash = *receiver.incoming_resources.hash_at(0);
        receiver.retire_incoming_resource(&link_id(), &hash);
        let after_retirement = feed(&mut receiver, &refreshed, 2_300);
        assert!(after_retirement.frames.is_empty());
        assert!(receiver.incoming_resources.is_empty());
    }

    fn reencrypt_advertisement(frame: &[u8], iv: u8) -> std::vec::Vec<u8> {
        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        let mut sealed = payload.to_vec();
        let plaintext = link_key().open_in_place(&mut sealed).unwrap();
        let mut refreshed = std::vec![0u8; BROADCAST_MTU];
        let refreshed_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            header.context,
            plaintext,
            &test_entropy_bytes::<16>(iv),
            &mut refreshed,
        )
        .unwrap();
        refreshed.truncate(refreshed_len);
        refreshed
    }

    fn fill_incoming_capacity(receiver: &mut EngineState<TestStorageLayout>) {
        accept_everything(receiver);
        for changed_byte in [0x00, 0x11] {
            let mut payload = four_part_payload();
            payload[0] ^= changed_byte;
            assert_eq!(
                feed(receiver, &advertisement_frame(&payload, None), 2_000)
                    .frames
                    .len(),
                1,
            );
        }
        assert_eq!(receiver.incoming_resources.len(), 2);
    }

    #[test]
    fn a_burst_waits_once_then_promotes_immediately_when_capacity_returns() {
        use crate::engine::WakeSchedule;
        use crate::routing::links::resources::ResourceOffer;

        let mut receiver = engine_with_active_link();
        fill_incoming_capacity(&mut receiver);
        set_strategy(&mut receiver, ResourceStrategy::AcceptIf);

        let mut second_payload = four_part_payload();
        second_payload[0] ^= 0x5A;
        let second_advertisement = advertisement_frame(&second_payload, None);
        let mut decisions = 0;
        let queued = feed_judged(
            &mut receiver,
            &second_advertisement,
            2_100,
            &mut |_: &ResourceOffer| {
                decisions += 1;
                true
            },
        );
        assert!(
            queued.frames.is_empty(),
            "waiting sends neither a pull nor a rejection"
        );
        assert_eq!(receiver.pending_resource_offers.len(), 1);

        let retry = reencrypt_advertisement(&second_advertisement, 0xE4);
        let coalesced = feed_judged(&mut receiver, &retry, 2_200, &mut |_: &ResourceOffer| {
            panic!("a validated retry must not rerun AcceptIf")
        });
        assert!(coalesced.frames.is_empty());
        assert_eq!(decisions, 1);
        assert_eq!(receiver.pending_resource_offers.len(), 1);
        assert_eq!(
            receiver.pending_resource_offers.offers()[0].first_arrived_at(),
            InstantMillis(2_100),
        );

        let first_hash = *receiver.incoming_resources.hash_at(0);
        receiver.retire_incoming_resource(&link_id(), &first_hash);
        assert_eq!(
            receiver.resource_deadlines_wake(),
            WakeSchedule::At(InstantMillis(0)),
            "capacity release makes Resource maintenance immediately due",
        );

        let mut promoted_frames = std::vec::Vec::new();
        receiver.fire_due_resource_deadlines(
            InstantMillis(2_300),
            &mut |bytes: &mut [u8]| bytes.fill(0xC5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    if let Some(frame) = crate::engine::test_support::filled_frame(fill) {
                        promoted_frames.push(frame);
                    }
                }
            },
        );
        assert_eq!(promoted_frames.len(), 1);
        assert_eq!(
            WirePacketHeader::parse(&promoted_frames[0])
                .unwrap()
                .0
                .context,
            WireContext::ResourceRequest,
        );
        assert!(receiver.pending_resource_offers.is_empty());
        assert_eq!(receiver.incoming_resources.len(), 2);
        let Some(LinkPhase::Active { last_inbound, .. }) = receiver.links.phase_for(&link_id())
        else {
            panic!("the test Link remains active")
        };
        assert_eq!(
            *last_inbound,
            InstantMillis(2_200),
            "promotion does not forge inbound traffic at its later maintenance time",
        );
        #[cfg(feature = "runtime-metrics")]
        {
            let events = receiver.metrics_snapshot().resources.admission_events;
            assert_eq!(events.get(ResourceAdmissionEvent::Queued), 1);
            assert_eq!(events.get(ResourceAdmissionEvent::Promoted), 1);
            assert_eq!(events.get(ResourceAdmissionEvent::Expired), 0);
            assert_eq!(events.get(ResourceAdmissionEvent::Rejected), 0);
        }
    }

    #[test]
    fn an_offer_that_can_never_fit_is_rejected_without_waiting() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let mut sender = active_engine::<crate::storage::GrowableHeap>();
        let oversized = std::vec![0xA5; 5_000];
        let advertisement = advertise_from(&mut sender, &oversized, None);

        let rejected = feed(&mut receiver, &advertisement, 2_000);
        assert_eq!(rejected.frames.len(), 1);
        assert_eq!(
            WirePacketHeader::parse(&rejected.frames[0].1)
                .unwrap()
                .0
                .context,
            WireContext::ResourceReceiverCancel,
        );
        assert!(receiver.incoming_resources.is_empty());
        assert!(receiver.pending_resource_offers.is_empty());
    }

    #[test]
    fn an_expired_response_wait_rejects_and_settles_as_resource_capacity() {
        use crate::routing::links::resources::ResourceCorrelation;

        let mut receiver = engine_with_active_link();
        fill_incoming_capacity(&mut receiver);
        let request_id = track_pending_request(&mut receiver, CommandId(42), 1_800, 20_000);

        let mut response_payload = four_part_payload();
        response_payload[0] ^= 0x33;
        let mut sender = engine_with_active_link();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &response_payload,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Response(request_id),
            },
            InstantMillis(2_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xA6),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = crate::engine::test_support::filled_frame(fill);
                }
            },
        );
        let queued = feed(&mut receiver, &advertisement.unwrap(), 2_100);
        assert!(queued.frames.is_empty());
        assert_eq!(receiver.pending_resource_offers.len(), 1);
        assert_eq!(
            receiver.pending_resource_offers.offers()[0].wait_deadline(),
            InstantMillis(7_100),
        );

        let mut frames = std::vec::Vec::new();
        let mut settlements = std::vec::Vec::new();
        receiver.fire_due_resource_deadlines(
            InstantMillis(7_100),
            &mut |bytes: &mut [u8]| bytes.fill(0xC6),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(frame) = crate::engine::test_support::filled_frame(fill) {
                        frames.push(frame);
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        assert!(frames.iter().any(|frame| {
            WirePacketHeader::parse(frame)
                .is_ok_and(|(header, _)| header.context == WireContext::ResourceReceiverCancel)
        }));
        assert_eq!(
            settlements,
            [(
                CommandId(42),
                Settlement::SendRequest(Err(crate::engine::SendRequestFailure::ResourceCapacity,)),
            )],
        );
        assert!(receiver.pending_resource_offers.is_empty());
        assert!(!receiver.receipts.has_pending_request(request_id));
        #[cfg(feature = "runtime-metrics")]
        {
            let events = receiver.metrics_snapshot().resources.admission_events;
            assert_eq!(events.get(ResourceAdmissionEvent::Queued), 1);
            assert_eq!(events.get(ResourceAdmissionEvent::Promoted), 0);
            assert_eq!(events.get(ResourceAdmissionEvent::Expired), 1);
            assert_eq!(events.get(ResourceAdmissionEvent::Rejected), 1);
        }
    }

    #[test]
    fn a_response_wait_never_outlives_its_request_deadline() {
        use crate::routing::links::resources::ResourceCorrelation;

        let mut receiver = engine_with_active_link();
        fill_incoming_capacity(&mut receiver);
        let request_id = track_pending_request(&mut receiver, CommandId(42), 1_800, 4_000);

        let mut response_payload = four_part_payload();
        response_payload[0] ^= 0x77;
        let mut sender = engine_with_active_link();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &response_payload,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Response(request_id),
            },
            InstantMillis(2_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xA7),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = crate::engine::test_support::filled_frame(fill);
                }
            },
        );
        assert!(feed(&mut receiver, &advertisement.unwrap(), 2_100)
            .frames
            .is_empty());
        assert_eq!(
            receiver.resource_deadlines_wake(),
            crate::engine::WakeSchedule::At(InstantMillis(4_000)),
        );

        let mut settlements = std::vec::Vec::new();
        let wake = receiver.settle_timed_out_receipts(InstantMillis(4_000), &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settlements.push((id, settlement));
            }
        });
        assert_eq!(
            settlements,
            [(
                CommandId(42),
                Settlement::SendRequest(Err(crate::engine::SendRequestFailure::Timeout)),
            )],
        );
        assert_eq!(
            wake.resource_deadlines,
            crate::engine::WakeSchedule::At(InstantMillis(0)),
        );

        let mut rejected = false;
        receiver.fire_due_resource_deadlines(
            InstantMillis(4_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xC8),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    if let Some(frame) = crate::engine::test_support::filled_frame(fill) {
                        rejected |= WirePacketHeader::parse(&frame).is_ok_and(|(header, _)| {
                            header.context == WireContext::ResourceReceiverCancel
                        });
                    }
                }
            },
        );
        assert!(rejected);
        assert!(receiver.pending_resource_offers.is_empty());
    }

    #[test]
    fn a_full_pending_queue_rejects_the_next_valid_offer() {
        let mut receiver = engine_with_active_link();
        fill_incoming_capacity(&mut receiver);

        for changed_byte in [0x21, 0x22, 0x23, 0x24] {
            let mut payload = four_part_payload();
            payload[0] ^= changed_byte;
            let waiting = feed(
                &mut receiver,
                &advertisement_frame(&payload, None),
                2_100 + changed_byte as u64,
            );
            assert!(waiting.frames.is_empty());
        }
        assert_eq!(receiver.pending_resource_offers.len(), 4);

        let mut overflow_payload = four_part_payload();
        overflow_payload[0] ^= 0x25;
        let rejected = feed(
            &mut receiver,
            &advertisement_frame(&overflow_payload, None),
            2_200,
        );
        assert_eq!(rejected.frames.len(), 1);
        assert_eq!(
            WirePacketHeader::parse(&rejected.frames[0].1)
                .unwrap()
                .0
                .context,
            WireContext::ResourceReceiverCancel,
        );
        assert_eq!(receiver.pending_resource_offers.len(), 4);
        #[cfg(feature = "runtime-metrics")]
        {
            let events = receiver.metrics_snapshot().resources.admission_events;
            assert_eq!(events.get(ResourceAdmissionEvent::Queued), 4);
            assert_eq!(events.get(ResourceAdmissionEvent::Rejected), 1);
        }
    }

    #[test]
    fn link_teardown_drops_its_waiting_offers_without_payload_state() {
        let mut receiver = engine_with_active_link();
        fill_incoming_capacity(&mut receiver);
        let mut payload = four_part_payload();
        payload[0] ^= 0x42;
        assert!(
            feed(&mut receiver, &advertisement_frame(&payload, None), 2_100,)
                .frames
                .is_empty()
        );
        assert_eq!(receiver.pending_resource_offers.len(), 1);

        let mut close = [0u8; BROADCAST_MTU];
        receiver
            .write_owed_link_close(&link_id(), &test_entropy_bytes::<16>(0x91), &mut close)
            .unwrap();
        assert!(receiver.pending_resource_offers.is_empty());
    }

    #[test]
    fn unauthenticated_advertisements_never_enter_the_wait_queue() {
        let mut receiver = engine_with_active_link();
        fill_incoming_capacity(&mut receiver);
        let mut tampered = advertisement_frame(&four_part_payload(), None);
        *tampered.last_mut().unwrap() ^= 0x80;

        let ignored = feed(&mut receiver, &tampered, 2_100);
        assert!(ignored.frames.is_empty());
        assert!(receiver.pending_resource_offers.is_empty());
    }
}
