use super::*;
use crate::engine::{RespondFailure, SendResourceFailure};
use crate::routing::links::resources::{ResourceBody, ResourceMetadata};
use crate::units::ByteLimit;
use crate::wire::WirePacketHeader;

enum Framing {
    Raw,
    Envelope,
    File,
}

fn check_value_limit(opening: &Opening, framing: &Framing, value: &[u8], limit: ByteLimit) {
    let mut receiver = engine_with_active_link();
    let mut sender = engine_with_active_link();
    let request =
        track_pending_request_with_limit(&mut receiver, CommandId(42), 1_800, 20_000, limit);
    let split = value.len() / 2;
    let (head, tail) = value.split_at(split);
    let first = match framing {
        Framing::Envelope => {
            let mut bytes = std::vec![0; RESPONSE_WIRE_OVERHEAD + head.len()];
            write_response_plaintext(&request, head, &mut bytes).unwrap();
            bytes
        }
        Framing::Raw | Framing::File => head.to_vec(),
    };
    let packed = b"file metadata";
    let metadata = match framing {
        Framing::File => ResourceMetadata::Packed(packed),
        Framing::Raw | Framing::Envelope => ResourceMetadata::None,
    };
    let stream_total = first.len() + tail.len() + metadata.block_len();
    let mut delivered = Delivery::default();
    for (index, data) in [(1, first.as_slice()), (2, tail)] {
        let metadata = match (index, metadata) {
            (1, metadata) => metadata,
            (_, ResourceMetadata::Packed(packed)) => ResourceMetadata::SentInFirstSegment {
                packed_len: packed.len() as u32,
            },
            (_, ResourceMetadata::None | ResourceMetadata::SentInFirstSegment { .. }) => {
                ResourceMetadata::None
            }
        };
        let mut plaintext = match metadata {
            ResourceMetadata::Packed(packed) => metadata_block(packed),
            ResourceMetadata::None | ResourceMetadata::SentInFirstSegment { .. } => std::vec![],
        };
        plaintext.extend_from_slice(data);
        let at = 2_000 + (index - 1) * 1_000;
        let ad = advertise_response_body_from(
            &mut sender,
            CommandId(20 + index),
            request,
            ResourceBody {
                data,
                compressed_candidate: opening.candidate(),
                metadata,
            },
            ResourceSegment {
                index,
                total_segments: 2,
                total_data_bytes: (stream_total - metadata.block_len()) as u64,
            },
            at,
        );
        let pull = feed(&mut receiver, &ad, at + 100);
        assert_eq!(pull.frames.len(), 1);
        assert_eq!(pull.settlements, std::vec![]);
        let served = feed(&mut sender, &pull.frames[0].1, at + 200);
        assert!(!served.frames.is_empty());
        let replies = finish_segment(
            &mut receiver,
            &served.frames,
            &plaintext,
            opening,
            at + 300,
            &mut delivered,
        );
        let cumulative = if index == 1 { head.len() } else { value.len() };
        if !limit.allows(cumulative as u64) {
            assert_eq!(replies.len(), 1);
            assert_eq!(
                WirePacketHeader::parse(&replies[0].1).unwrap().0.context,
                WireContext::ResourceReceiverCancel
            );
            let rejected = feed(&mut sender, &replies[0].1, at + 400);
            assert_eq!(
                rejected,
                InboundCapture {
                    settlements: std::vec![(
                        CommandId(20 + index),
                        Settlement::Respond(Err(RespondFailure::Resource(
                            SendResourceFailure::RejectedByPeer
                        )))
                    )],
                    ..InboundCapture::default()
                }
            );
            assert!(sender.outgoing_resources.is_empty());
            assert_eq!(sender.outgoing_assemblies.original_hash(&link_id()), None);
            assert_eq!(
                delivered,
                Delivery {
                    chunks: if index == 1 {
                        std::vec![]
                    } else {
                        std::vec![(CommandId(42), request, 1, head.to_vec())]
                    },
                    settlements: std::vec![(
                        CommandId(42),
                        Settlement::SendRequest(Err(SendRequestFailure::ResponseTooLarge))
                    )],
                    failures: std::vec![],
                }
            );
            assert_retired(&receiver, request);
            for (_, frame) in served.frames {
                assert_eq!(
                    feed(&mut receiver, &frame, at + 500),
                    InboundCapture::default()
                );
            }
            return;
        }
        assert_eq!(replies.len(), 1);
        for (_, proof) in replies {
            feed(&mut sender, &proof, at + 400);
        }
    }
    let finished = match opening {
        Opening::Uncompressed => 3_300,
        Opening::Inflated => 3_310,
    };
    assert_eq!(
        delivered,
        Delivery {
            chunks: std::vec![
                (CommandId(42), request, 1, head.to_vec()),
                (CommandId(42), request, 2, tail.to_vec())
            ],
            settlements: std::vec![(
                CommandId(42),
                Settlement::SendRequest(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(finished - 1_800),
                    evidence: DeliveryEvidence::Response
                }))
            )],
            failures: std::vec![],
        }
    );
    assert_retired(&receiver, request);
}

#[test]
fn split_response_value_limits_exclude_only_verified_framing() {
    for opening in [Opening::Uncompressed, Opening::Inflated] {
        for framing in [Framing::Raw, Framing::Envelope, Framing::File] {
            for limit in [
                ByteLimit::Maximum(0),
                ByteLimit::Maximum(128),
                ByteLimit::Maximum(255),
                ByteLimit::Maximum(256),
                ByteLimit::Maximum(u64::MAX),
                ByteLimit::Unlimited,
            ] {
                for prefix in [
                    &[0xA7][..],
                    &[0xC0],
                    &[0xC4, 0],
                    &[0xC5, 0, 1],
                    &[0xC6, 0, 0, 0, 1],
                    b"\x92malformed",
                ] {
                    let mut value = [0xA7; 256];
                    value[..prefix.len()].copy_from_slice(prefix);
                    check_value_limit(&opening, &framing, &value, limit);
                }
            }
        }
    }
}

#[test]
fn split_file_value_budget_counts_envelope_shaped_bytes_literally() {
    let mut file = [0; 256];
    write_response_plaintext(
        &RequestId([0x22; 16]),
        &[0xA7; 256 - RESPONSE_WIRE_OVERHEAD],
        &mut file,
    )
    .unwrap();
    for opening in [Opening::Uncompressed, Opening::Inflated] {
        for limit in [ByteLimit::Maximum(255), ByteLimit::Maximum(256)] {
            check_value_limit(&opening, &Framing::File, &file, limit);
        }
    }
}
