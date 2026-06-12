//! The engine's send path for a resource — RNS 1.3.1 `Resource(data, link)`
//! plus `Resource.advertise`: seal the transfer straight into the outgoing
//! register and owe the advertisement to the link's interface. This is a
//! borrow-taking entry point beside the command queue, not a command: a
//! payload up to a mebibyte never rides an enum. Everything that can refuse
//! settles immediately; success settles later, when the receiver's proof
//! arrives or the transfer times out.

use crate::engine::commands::{CommandId, SendResourceError, SendResourceFailure, Settlement};
use crate::engine::{Directive, EngineReaction, EngineState, Journaled};
use crate::routing::links::data::{write_link_packet, LINK_MDU};
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::advertisement::{ResourceAdvertisement, ResourceFlags};
use crate::routing::links::resources::build_outgoing::build_outgoing_resource;
use crate::routing::links::resources::table::TrackOutgoingResourceError;
use crate::routing::links::resources::{
    resource_sdu, RESOURCE_NONCE_LEN, {HASHMAP_MAX_LEN, MAP_HASH_LEN},
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::routing::storage::EngineStorage;
use crate::wire::WireContext;

impl<S: EngineStorage> EngineState<S> {
    /// Build and advertise one resource over an active link. `data` is the
    /// uncompressed payload; `compressed_candidate` is the host's bz2 attempt
    /// (or `None` — an embedded host never links a compressor) and the
    /// reference's keep-only-if-smaller rule picks between them. The sealed
    /// stream lands in the outgoing register's slot, the advertisement goes
    /// out grant-first, and the command settles now only on refusal —
    /// delivery settles at the receiver's proof.
    #[allow(clippy::too_many_arguments)]
    pub fn ingest_send_resource_into<F>(
        &mut self,
        id: CommandId,
        link_id: LinkId,
        data: &[u8],
        compressed_candidate: Option<&[u8]>,
        request_id: Option<RequestId>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let settle = |sink: &mut dyn FnMut(EngineReaction<'_>), failure| {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id,
                settlement: Settlement::SendResource(Err(failure)),
            }));
        };
        let Some(phase) = self.links.phase_for(&link_id) else {
            settle(
                sink,
                SendResourceFailure::Rejected(SendResourceError::NoSuchLink),
            );
            return;
        };
        let LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            ..
        } = phase
        else {
            settle(
                sink,
                SendResourceFailure::Rejected(SendResourceError::LinkNotActive),
            );
            return;
        };
        let mtu = *mtu;
        let fire_on = *attached_interface;

        let mut seal_iv = [0u8; 16];
        fill_entropy(&mut seal_iv);
        let sdu = resource_sdu(mtu);
        let tracked_result =
            self.outgoing_resources
                .track(link_id, sdu, id, request_id, |transfer, hashmap| {
                    build_outgoing_resource(
                        data,
                        compressed_candidate,
                        key,
                        &seal_iv,
                        || {
                            let mut nonce = [0u8; RESOURCE_NONCE_LEN];
                            fill_entropy(&mut nonce);
                            nonce
                        },
                        sdu,
                        transfer,
                        hashmap,
                    )
                });
        let hash = match tracked_result {
            Ok(hash) => hash,
            Err(error) => {
                let rejection = match error {
                    TrackOutgoingResourceError::TableFull => SendResourceError::TableFull,
                    TrackOutgoingResourceError::LinkBusy => SendResourceError::LinkBusy,
                    TrackOutgoingResourceError::Build(build) => SendResourceError::Build(build),
                };
                settle(sink, SendResourceFailure::Rejected(rejection));
                return;
            }
        };

        let mut adv_iv = [0u8; 16];
        fill_entropy(&mut adv_iv);
        let mut wrote = false;
        let outgoing = &self.outgoing_resources;
        let mut fill = |slot: &mut [u8]| -> Option<usize> {
            let index = outgoing.lookup(&link_id, &hash)?;
            let state = outgoing.state(index);
            let names = outgoing.names_flat(index);
            let first_segment = &names[..names.len().min(HASHMAP_MAX_LEN * MAP_HASH_LEN)];
            let advertisement = ResourceAdvertisement {
                transfer_size: state.sealed_transfer_len as u64,
                data_size: state.uncompressed_data_len,
                part_count: state.part_count as u64,
                hash,
                salt_nonce: state.salt_nonce,
                original_hash: hash,
                segment_index: 1,
                total_segments: 1,
                request_id: state.request_id,
                flags: ResourceFlags {
                    encrypted: true,
                    compressed: state.compression.wire_flag(),
                    split: false,
                    is_request: false,
                    is_response: state.request_id.is_some(),
                    has_metadata: false,
                },
                hashmap: first_segment,
            };
            let mut plaintext = [0u8; LINK_MDU];
            let plaintext_len = advertisement.write(&mut plaintext).ok()?;
            let wire_len = write_link_packet(
                &link_id,
                key,
                mtu,
                WireContext::ResourceAdvertisement,
                &plaintext[..plaintext_len],
                &adv_iv,
                slot,
            )
            .ok()?;
            wrote = true;
            Some(wire_len)
        };
        sink(EngineReaction::Directive(Directive::EmitFrame {
            target: fire_on,
            fill: &mut fill,
        }));
        if !wrote {
            self.outgoing_resources.remove(&link_id, &hash);
            settle(sink, SendResourceFailure::WriteFailed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};
    use crate::crypto::{CryptoError, Ed25519PublicKey};
    use crate::engine::test_support::{filled_frame, Cap};
    use crate::engine::InstantMillis;
    use crate::interfaces::InterfaceId;
    use crate::routing::links::resources::build_outgoing::BuildOutgoingResourceError;
    use crate::routing::links::resources::table::OutgoingResourceStatus;
    use crate::routing::links::table::InitiatedLink;
    use crate::routing::links::LinkKey;
    use crate::wire::{DestinationHash, PacketType, WirePacketHeader, BROADCAST_MTU};

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

    fn sender_with_active_link() -> EngineState<Cap> {
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

    struct SendCapture {
        frames: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
        settlements: std::vec::Vec<(CommandId, Settlement)>,
    }

    fn send(
        engine: &mut EngineState<Cap>,
        id: u64,
        data: &[u8],
        candidate: Option<&[u8]>,
    ) -> SendCapture {
        let mut capture = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        engine.ingest_send_resource_into(
            CommandId(id),
            link_id(),
            data,
            candidate,
            None,
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
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

    fn case1_plaintext() -> std::vec::Vec<u8> {
        b"reticulum resources ride the link ".repeat(40)
    }

    const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";

    #[test]
    fn a_send_resource_seals_registers_and_advertises() {
        let mut engine = sender_with_active_link();
        let plaintext = case1_plaintext();
        let candidate = hx(CASE1_BZ2);
        let capture = send(&mut engine, 7, &plaintext, Some(&candidate));

        assert!(
            capture.settlements.is_empty(),
            "success settles at the proof"
        );
        assert_eq!(capture.frames.len(), 1);
        let (target, frame) = &capture.frames[0];
        assert_eq!(*target, lane());

        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(header.context, WireContext::ResourceAdvertisement);
        assert_eq!(
            header.destination,
            DestinationHash::new(*link_id().as_bytes())
        );

        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();

        let index = engine
            .outgoing_resources
            .lookup(&link_id(), &advertisement.hash)
            .expect("the advertised transfer is registered");
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.status, OutgoingResourceStatus::Advertised);
        assert_eq!(
            advertisement.transfer_size,
            state.sealed_transfer_len as u64
        );
        assert_eq!(advertisement.data_size, 1_360);
        assert_eq!(advertisement.part_count, 1);
        assert_eq!(advertisement.salt_nonce, state.salt_nonce);
        assert_eq!(advertisement.original_hash, advertisement.hash);
        assert_eq!(advertisement.segment_index, 1);
        assert_eq!(advertisement.total_segments, 1);
        assert_eq!(advertisement.request_id, None);
        assert!(advertisement.flags.encrypted);
        assert!(advertisement.flags.compressed);
        assert!(!advertisement.flags.is_response);
        assert_eq!(
            advertisement.hashmap,
            engine.outgoing_resources.names_flat(index),
        );
    }

    #[test]
    fn one_resource_per_link_rejects_the_second_send() {
        let mut engine = sender_with_active_link();
        let plaintext = case1_plaintext();
        send(&mut engine, 7, &plaintext, None);
        let second = send(&mut engine, 8, &plaintext, None);

        assert!(second.frames.is_empty());
        assert_eq!(second.settlements.len(), 1);
        assert!(matches!(
            second.settlements[0],
            (
                CommandId(8),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceError::LinkBusy,
                ))),
            ),
        ));
        assert_eq!(engine.outgoing_resources.len(), 1);
    }

    #[test]
    fn a_missing_or_inactive_link_rejects_by_name() {
        let mut engine = EngineState::<Cap>::default();
        let capture = send(&mut engine, 7, b"data", None);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceError::NoSuchLink,
                ))),
            ),
        ));

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
        let capture = send(&mut engine, 8, b"data", None);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(8),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceError::LinkNotActive,
                ))),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }

    #[test]
    fn a_transfer_the_store_cannot_hold_rejects_and_releases_the_slot() {
        let mut engine = sender_with_active_link();
        let oversized = std::vec![0x42u8; 5_000];
        let capture = send(&mut engine, 7, &oversized, None);

        assert!(capture.frames.is_empty());
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceError::Build(BuildOutgoingResourceError::Seal(
                        CryptoError::BufferTooShort,
                    )),
                ))),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }
}
