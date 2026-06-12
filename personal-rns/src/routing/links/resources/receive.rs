//! The engine's receive path for a resource — RNS 1.3.1 `Resource.accept`
//! plus the receiver's half of the link dispatch: gate an inbound
//! advertisement on the link's [`ResourceStrategy`] and the store's
//! capacity, register the transfer, and start pulling parts by name. The
//! strategy gate runs before a single part moves: the advertisement declares
//! the decompressed size and compression kind up front, so refusing is free.

use crate::engine::commands::{
    CommandId, CommandOutcome, SetResourceStrategy, SetResourceStrategyError,
};
use crate::engine::{Directive, EngineReaction, EngineState, InstantMillis};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::ingress::{DataPacket, IngestPacketOutcome};
use crate::routing::links::data::write_link_packet;
use crate::routing::links::resources::advertisement::ResourceAdvertisement;
use crate::routing::links::resources::control::write_part_request_plaintext;
use crate::routing::links::resources::table::AcceptedResource;
use crate::routing::links::resources::{
    resource_sdu, ResourceCompression, ResourceHash, ResourceStrategy, MAP_HASH_LEN, WINDOW_MAX,
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::routing::storage::EngineStorage;
use crate::wire::{DestinationType, PacketType, WireContext};

impl<S: EngineStorage> EngineState<S> {
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
                error: SetResourceStrategyError::NoSuchLink,
            },
            Err(LinkActivationError::WrongPhase) => CommandOutcome::SetResourceStrategyRejected {
                id,
                error: SetResourceStrategyError::LinkNotActive,
            },
        }
    }

    /// RNS 1.3.1 `Resource.accept`, behind the strategy gate. Refusals are
    /// silent, like a reference receiver that never accepts: the sender's
    /// advertisement simply goes unanswered. The deferred shapes — split
    /// (multi-segment), resource-as-request, metadata — are refused here
    /// too, named in order in the gate. Advertisements stay behind the
    /// duplicate filter (only `RESOURCE_REQ`/`RESOURCE`/`RESOURCE_PRF` are
    /// exempt in the reference).
    pub(crate) fn classify_resource_advertisement<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        let Some(LinkPhase::Active {
            key,
            mtu,
            resource_strategy,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored;
        };
        let ResourceStrategy::Accept {
            max_uncompressed_len,
            accept_compressed,
        } = *resource_strategy
        else {
            return IngestPacketOutcome::Ignored;
        };
        let mtu = *mtu;
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data.destination,
            data.context,
            data.payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => return IngestPacketOutcome::Ignored,
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        let Ok(advertisement) = ResourceAdvertisement::parse(plaintext) else {
            return IngestPacketOutcome::Ignored;
        };
        if !advertisement.flags.encrypted
            || advertisement.flags.split
            || advertisement.flags.is_request
            || advertisement.flags.has_metadata
            || advertisement.total_segments != 1
            || advertisement.hashmap.is_empty()
        {
            return IngestPacketOutcome::Ignored;
        }
        let compression = ResourceCompression::from_wire_flag(advertisement.flags.compressed);
        if compression == ResourceCompression::Bz2 && !accept_compressed {
            return IngestPacketOutcome::Ignored;
        }
        if advertisement.data_size > max_uncompressed_len {
            return IngestPacketOutcome::Ignored;
        }
        let Ok(sealed_transfer_len) = usize::try_from(advertisement.transfer_size) else {
            return IngestPacketOutcome::Ignored;
        };
        let sdu = resource_sdu(mtu);
        let part_count = sealed_transfer_len.div_ceil(sdu);
        if part_count == 0 {
            return IngestPacketOutcome::Ignored;
        }
        let accepted = AcceptedResource {
            hash: advertisement.hash,
            salt_nonce: advertisement.salt_nonce,
            compression,
            uncompressed_data_len: advertisement.data_size,
            sealed_transfer_len,
            part_count,
            sdu,
            request_id: advertisement.request_id,
            initial_names: advertisement.hashmap,
        };
        if self.incoming_resources.accept(link_id, accepted).is_err() {
            return IngestPacketOutcome::Ignored;
        }
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::OwesResourcePull {
            link_id,
            hash: advertisement.hash,
        }
    }

    /// RNS 1.3.1 `Resource.request_next`: scan one window of part slots from
    /// the consecutive-completed height, request every missing part whose
    /// name is known, and flag the request hashmap-exhausted — carrying the
    /// last known name — when the window runs past the names received so
    /// far. Nothing is emitted when the window holds nothing to ask for.
    pub(crate) fn emit_resource_pull<F>(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let Some(index) = self.incoming_resources.lookup(link_id, hash) else {
            return;
        };
        let state = *self.incoming_resources.state(index);

        let mut requested = [0u8; WINDOW_MAX * MAP_HASH_LEN];
        let mut requested_count = 0;
        let mut exhausted = false;
        {
            let received = self.incoming_resources.received_flags(index);
            let names = self.incoming_resources.names_flat(index);
            let mut position = state.consecutive_completed.map_or(0, |height| height + 1);
            let mut scanned = 0;
            while position < state.part_count && scanned < state.window {
                if !received[position] {
                    if position < state.hashmap_height {
                        requested[requested_count * MAP_HASH_LEN..][..MAP_HASH_LEN]
                            .copy_from_slice(&names[position * MAP_HASH_LEN..][..MAP_HASH_LEN]);
                        requested_count += 1;
                    } else {
                        exhausted = true;
                        break;
                    }
                }
                position += 1;
                scanned += 1;
            }
        }
        if requested_count == 0 && !exhausted {
            return;
        }
        let names = self.incoming_resources.names_flat(index);
        let last = state.hashmap_height.saturating_sub(1);
        let Ok(last_known) =
            <[u8; MAP_HASH_LEN]>::try_from(&names[last * MAP_HASH_LEN..(last + 1) * MAP_HASH_LEN])
        else {
            return;
        };
        {
            let state = self.incoming_resources.state_mut(index);
            state.outstanding_part_count = requested_count;
            state.waiting_for_hmu = exhausted;
        }

        let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            ..
        }) = self.links.phase_for(link_id)
        else {
            return;
        };
        let mtu = *mtu;
        let fire_on = *attached_interface;
        let mut iv = [0u8; 16];
        fill_entropy(&mut iv);
        let mut fill = |slot: &mut [u8]| -> Option<usize> {
            let mut plaintext = [0u8; 1 + MAP_HASH_LEN + 32 + WINDOW_MAX * MAP_HASH_LEN];
            let plaintext_len = write_part_request_plaintext(
                hash,
                exhausted.then_some(&last_known),
                &requested[..requested_count * MAP_HASH_LEN],
                &mut plaintext,
            )
            .ok()?;
            write_link_packet(
                link_id,
                key,
                mtu,
                WireContext::ResourceRequest,
                &plaintext[..plaintext_len],
                &iv,
                slot,
            )
            .ok()
        };
        sink(EngineReaction::Directive(Directive::EmitFrame {
            target: fire_on,
            fill: &mut fill,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{
        x25519_diffie_hellman, Ed25519PublicKey, X25519PublicKey, X25519SecretKey,
    };
    use crate::engine::commands::{
        EngineCommand, IssuedCommand, SetResourceStrategyFailure, Settlement,
    };
    use crate::engine::test_support::{filled_frame, routable_descriptor, Cap, TEST_ENTROPY};
    use crate::engine::Journaled;
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::links::resources::control::parse_part_request_plaintext;
    use crate::routing::links::table::InitiatedLink;
    use crate::routing::links::LinkKey;
    use crate::wire::{DestinationHash, WirePacketHeader, BROADCAST_MTU};

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    const INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    const RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";
    const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";

    fn link_id() -> LinkId {
        LinkId::new(hx(LINK_ID).try_into().unwrap())
    }

    fn link_key() -> LinkKey {
        let scalar: [u8; 32] = hx(INITIATOR_SCALAR).try_into().unwrap();
        let public: [u8; 32] = hx(RESPONDER_PUBLIC).try_into().unwrap();
        let shared = x25519_diffie_hellman(&X25519SecretKey::new(scalar), &X25519PublicKey(public));
        LinkKey::derive(&link_id(), &shared)
    }

    fn lane() -> InterfaceId {
        InterfaceId::new([0xEE; 16])
    }

    fn engine_with_active_link() -> EngineState<Cap> {
        let mut engine = EngineState::<Cap>::default();
        engine
            .links
            .track_initiated(InitiatedLink {
                link_id: link_id(),
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                requested_at: InstantMillis(500),
                timeout_at: InstantMillis(5_000),
                command_id: CommandId(1),
            })
            .unwrap();
        engine
            .links
            .activate_initiated(
                &link_id(),
                link_key(),
                250,
                BROADCAST_MTU,
                lane(),
                InstantMillis(1_000),
                Ed25519PublicKey([0x99; 32]),
            )
            .unwrap();
        engine
    }

    fn advertisement_frame(data: &[u8], candidate: Option<&[u8]>) -> std::vec::Vec<u8> {
        let mut sender = engine_with_active_link();
        let mut frame = None;
        sender.ingest_send_resource_into(
            CommandId(7),
            link_id(),
            data,
            candidate,
            None,
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    frame = filled_frame(fill);
                }
            },
        );
        frame.expect("the sender advertises")
    }

    struct InboundCapture {
        frames: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
        settlements: std::vec::Vec<(CommandId, Settlement)>,
    }

    fn feed(engine: &mut EngineState<Cap>, frame: &[u8], at: u64) -> InboundCapture {
        let mut capture = InboundCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        let mut raw = frame.to_vec();
        engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(at),
                source_interface: lane(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &[routable_descriptor(lane())],
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xC7),
            &mut |_: &crate::engine::ProofRequest| false,
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill }) => {
                    if let Some(frame) = filled_frame(fill) {
                        capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        capture
    }

    fn accept_everything(engine: &mut EngineState<Cap>) {
        let mut settled = std::vec::Vec::new();
        engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_len: 1 << 20,
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
        assert!(matches!(
            settled[0],
            (CommandId(9), Settlement::SetResourceStrategy(Ok(()))),
        ));
    }

    fn four_part_payload() -> std::vec::Vec<u8> {
        b"resource parts ride raw on the wire! ".repeat(41)
    }

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
    fn the_strategy_command_demands_an_active_link() {
        let mut engine = EngineState::<Cap>::default();
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
                    SetResourceStrategyError::NoSuchLink,
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
            Some(&hx(CASE1_BZ2)),
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
