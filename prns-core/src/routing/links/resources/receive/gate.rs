//! RNS 1.3.5 `Resource.accept`: the strategy gate runs before a single part moves — the advertisement declares size and kind up front, so refusing is free.

use crate::engine::{CommandId, CommandOutcome, SetResourceStrategy, SetResourceStrategyRejection};
use crate::engine::{EngineState, InstantMillis};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::ingress::{DataPacket, IgnoreReason, IngestPacketOutcome};
use crate::routing::links::resources::advertisement::ResourceAdvertisement;
use crate::routing::links::resources::assembly::SegmentFit;
use crate::routing::links::resources::table::{AcceptIncomingResourceError, AcceptedResource};
use crate::routing::links::resources::{
    resource_sdu, ResourceCompression, ResourceCorrelation, ResourceStrategy, MAX_EFFICIENT_SIZE,
    PART_REQUEST_MAX_RETRIES,
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::{DestinationType, PacketType};

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

    /// RNS 1.3.5 `Resource.accept`; refusals are silent, like a reference receiver that never accepts.
    /// Request-correlated and pending-response advertisements bypass the strategy, exactly the reference's `Link.receive` `RESOURCE_ADV` ladder (its strategy arms only ever see unsolicited resources).
    /// Advertisements stay behind the duplicate filter (only `RESOURCE_REQ`/`RESOURCE`/`RESOURCE_PRF` are exempt in the reference).
    pub(crate) fn ingest_resource_advertisement<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(LinkPhase::Active {
            key,
            mtu,
            resource_strategy,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let resource_strategy = *resource_strategy;
        let mtu = *mtu;
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
            (true, false, Some(id)) => ResourceCorrelation::Request(id),
            (false, true, Some(id)) => ResourceCorrelation::Response(id),
            _ => ResourceCorrelation::Unsolicited,
        };
        let bypasses_strategy = advertisement.total_segments == 1
            && match correlation {
                ResourceCorrelation::Response(id) => self.receipts.has_pending_request(id),
                ResourceCorrelation::Request(_) => true,
                ResourceCorrelation::Unsolicited => false,
            };
        let (max_uncompressed_len, accept_compressed) = if bypasses_strategy {
            (MAX_EFFICIENT_SIZE as u64, true)
        } else {
            match resource_strategy {
                ResourceStrategy::Accept {
                    max_uncompressed_len,
                    accept_compressed,
                } => (max_uncompressed_len, accept_compressed),
                ResourceStrategy::AcceptNone => {
                    return IngestPacketOutcome::Ignored(IgnoreReason::StrategyDeclined)
                }
            }
        };
        let compression = ResourceCompression::from_wire_flag(advertisement.flags.compressed);
        if compression == ResourceCompression::Bz2 && !accept_compressed {
            return IngestPacketOutcome::Ignored(IgnoreReason::StrategyDeclined);
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
        if advertisement.data_size > max_uncompressed_len {
            return IngestPacketOutcome::Ignored(IgnoreReason::StrategyDeclined);
        }
        let Ok(sealed_transfer_len) = usize::try_from(advertisement.transfer_size) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        let sdu = resource_sdu(mtu);
        let part_count = sealed_transfer_len.div_ceil(sdu);
        if part_count == 0 {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        let accepted = AcceptedResource {
            hash: advertisement.hash,
            salt_nonce: advertisement.salt_nonce,
            compression,
            has_metadata: advertisement.flags.has_metadata,
            uncompressed_data_len: advertisement.data_size,
            segment_index: advertisement.segment_index,
            total_segment_count: advertisement.total_segments,
            sealed_transfer_len,
            part_count,
            sdu,
            correlation,
            initial_names: advertisement.hashmap,
        };
        let inherited = match self.links.phase_for(&link_id) {
            Some(LinkPhase::Active {
                last_resource_window,
                last_resource_eifr,
                ..
            }) => (*last_resource_window, *last_resource_eifr),
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
                return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate)
            }
            Err(
                AcceptIncomingResourceError::HashmapTooLong
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
        if multi_segment && advertisement.segment_index == 1 {
            self.incoming_assemblies.begin(
                link_id,
                advertisement.original_hash,
                advertisement.total_segments,
            );
        }
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::OwesResourcePull {
            link_id,
            hash: advertisement.hash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{routable_descriptor, TestStorageLayout};
    use crate::engine::{Directive, EngineReaction};
    use crate::engine::{EngineCommand, IssuedCommand, SetResourceStrategyFailure, Settlement};
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::resources::receive::tests_support::*;
    use crate::routing::links::resources::ResourceHash;
    use crate::routing::links::resources::{ResourceBody, ResourceMetadata, ResourceSend};
    use crate::wire::WireContext;

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
            kind: ReceiptKind::SendRequest,
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
                correlation: ResourceCorrelation::Request(request_id),
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

        let mut receiver = engine_with_active_link();
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
                correlation: ResourceCorrelation::Request(request_id),
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
            transfer_size: (part_count * 464) as u64,
            data_size: 1_000,
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
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[0xD1; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
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
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::AcceptNone,
                }),
            },
            &[routable_descriptor(lane())],
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
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_len: 1 << 20,
                        accept_compressed: false,
                    },
                }),
            },
            &[routable_descriptor(lane())],
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
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_len: 100,
                        accept_compressed: true,
                    },
                }),
            },
            &[routable_descriptor(lane())],
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
    fn a_duplicate_advertisement_is_filtered() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let advertisement = advertisement_frame(&four_part_payload(), None);
        let first = feed(&mut receiver, &advertisement, 2_000);
        let second = feed(&mut receiver, &advertisement, 2_100);

        assert_eq!(first.frames.len(), 1);
        assert!(second.frames.is_empty());
        assert_eq!(receiver.incoming_resources.len(), 1);
    }
}
