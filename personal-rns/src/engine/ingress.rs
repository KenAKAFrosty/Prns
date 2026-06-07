use crate::engine::InstantMillis;
use crate::interfaces::{InboundPacket, InterfaceDescriptor, InterfaceId};
use crate::routing::announce::Announce;
use crate::routing::NextHop;
use crate::wire::{
    DestinationHash, DestinationType, PacketType, TransportId, WireContext, WirePacketHeader, MTU,
};

#[derive(Debug, PartialEq, Eq)]
pub struct DataPacket<'a> {
    pub destination_type: DestinationType,
    pub destination: DestinationHash,
    pub context: WireContext,
    pub maybe_transport_id: Option<TransportId>,
    pub payload: &'a mut [u8],
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Ingress<'a> {
    Announce {
        announce: Announce<'a>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        next_hop: NextHop,
    },

    Data {
        data: DataPacket<'a>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    LinkRequest,

    Proof {
        payload: &'a [u8],
        arrived_at: InstantMillis,
    },

    Unparseable,
}

impl<'a> Ingress<'a> {
    pub fn classify(packet: InboundPacket<'a>) -> Self {
        let InboundPacket {
            arrived_at,
            source_interface,
            bytes,
        } = packet;
        let (header, payload_offset) = match WirePacketHeader::parse(bytes) {
            Ok((header, payload)) => (header, bytes.len() - payload.len()),
            Err(_) => return Self::Unparseable,
        };
        let (_, payload) = bytes.split_at_mut(payload_offset);

        let received_hops = header.hops.saturating_add(1);

        match header.packet_type {
            PacketType::Announce => {
                if header.destination_type != DestinationType::Single {
                    return Self::Unparseable;
                }

                //erase mutable since it's not needed in this arm
                let payload: &'a [u8] = payload;
                let Ok(announce) = Announce::from_wire(&header, payload) else {
                    return Self::Unparseable;
                };

                // Debug self-check: parse↔serialize round-trip on every
                // accepted announce. If `to_wire` ever drifts from
                // `from_wire`, the engine would silently re-emit a
                // signature-broken packet on rebroadcast. Cheap in
                // debug (one MTU-sized scratch + compare), zero in
                // release.
                debug_assert!(
                    {
                        let mut scratch = [0u8; MTU];
                        announce
                            .to_wire(&mut scratch)
                            .map(|n| &scratch[..n] == payload)
                            .unwrap_or(false)
                    },
                    "Announce::to_wire(from_wire(payload)) must equal payload"
                );

                Self::Announce {
                    announce,
                    received_hops,
                    source_interface,
                    arrived_at,
                    next_hop: header.transport_id.map_or(NextHop::Direct, NextHop::Via),
                }
            }
            PacketType::Data => Self::Data {
                data: DataPacket {
                    destination_type: header.destination_type,
                    destination: header.destination,
                    context: header.context,
                    maybe_transport_id: header.transport_id,
                    payload,
                },
                received_hops,
                source_interface,
                arrived_at,
            },
            PacketType::LinkRequest => Self::LinkRequest,
            PacketType::Proof => Self::Proof {
                payload,
                arrived_at,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{
        ContextFlag, DestinationHash, IfacFlag, PropagationType, TransportId, WireContext,
        WirePacketHeader, HEADER_MIN_LEN,
    };

    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    fn header_bytes(packet_type: PacketType) -> [u8; HEADER_MIN_LEN] {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type,
            hops: 0,
            transport_id: None,
            destination: DestinationHash::new([0xA5; 16]),
            context: WireContext::None,
        };
        let mut bytes = [0u8; HEADER_MIN_LEN];
        assert_eq!(header.write(&mut bytes).unwrap(), HEADER_MIN_LEN);
        bytes
    }

    #[test]
    fn malformed_headers_are_unparseable() {
        let packet = InboundPacket {
            arrived_at: InstantMillis(7),
            source_interface: iface(0x01),
            bytes: &mut [0x01],
        };

        assert!(matches!(Ingress::classify(packet), Ingress::Unparseable));
    }

    #[test]
    fn recognized_non_announce_packets_classify_from_the_header() {
        for packet_type in [PacketType::Data, PacketType::LinkRequest, PacketType::Proof] {
            let mut bytes = header_bytes(packet_type);
            let packet = InboundPacket {
                arrived_at: InstantMillis(9),
                source_interface: iface(0x02),
                bytes: &mut bytes,
            };

            let classified = Ingress::classify(packet);
            match packet_type {
                PacketType::Data => assert!(matches!(classified, Ingress::Data { .. })),
                PacketType::LinkRequest => assert!(matches!(classified, Ingress::LinkRequest)),
                PacketType::Proof => assert!(matches!(classified, Ingress::Proof { .. })),
                PacketType::Announce => unreachable!(),
            }
        }
    }

    #[test]
    fn data_packets_carry_their_typed_fields_through_classification() {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Transport,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 5,
            transport_id: Some(TransportId::new([0x11; 16])),
            destination: DestinationHash::new([0xA5; 16]),
            context: WireContext::Resource,
        };
        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        let mut expected_payload = payload;
        let mut bytes = [0u8; MTU];
        let header_len = header.write(&mut bytes).unwrap();
        bytes[header_len..header_len + payload.len()].copy_from_slice(&payload);

        let packet = InboundPacket {
            arrived_at: InstantMillis(21),
            source_interface: iface(0x05),
            bytes: &mut bytes[..header_len + payload.len()],
        };

        let Ingress::Data {
            data,
            received_hops,
            source_interface,
            arrived_at,
        } = Ingress::classify(packet)
        else {
            panic!("a data packet should classify as data");
        };
        assert_eq!(
            data,
            DataPacket {
                destination_type: DestinationType::Plain,
                destination: DestinationHash::new([0xA5; 16]),
                context: WireContext::Resource,
                maybe_transport_id: Some(TransportId::new([0x11; 16])),
                payload: &mut expected_payload,
            }
        );
        assert_eq!(received_hops, 6);
        assert_eq!(source_interface, iface(0x05));
        assert_eq!(arrived_at, InstantMillis(21));
    }

    #[test]
    fn data_packets_classify_for_every_destination_type() {
        for destination_type in [
            DestinationType::Single,
            DestinationType::Group,
            DestinationType::Plain,
            DestinationType::Link,
        ] {
            let header = WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type,
                packet_type: PacketType::Data,
                hops: 0,
                transport_id: None,
                destination: DestinationHash::new([0xA5; 16]),
                context: WireContext::None,
            };
            let mut bytes = [0u8; HEADER_MIN_LEN];
            assert_eq!(header.write(&mut bytes).unwrap(), HEADER_MIN_LEN);
            let packet = InboundPacket {
                arrived_at: InstantMillis(23),
                source_interface: iface(0x06),
                bytes: &mut bytes,
            };

            let Ingress::Data { data, .. } = Ingress::classify(packet) else {
                panic!("data packets to any destination type classify as data");
            };
            assert_eq!(data.destination_type, destination_type);
            assert!(data.payload.is_empty());
        }
    }

    #[test]
    fn announce_packets_must_target_a_single_destination() {
        let mut raw = hx(RAW_ANNOUNCE);
        raw[0] |= (DestinationType::Group as u8) << 2;
        let packet = InboundPacket {
            arrived_at: InstantMillis(11),
            source_interface: iface(0x03),
            bytes: &mut raw,
        };

        assert!(matches!(Ingress::classify(packet), Ingress::Unparseable));
    }

    #[test]
    fn announce_received_hops_saturates_at_wire_max() {
        let mut raw = hx(RAW_ANNOUNCE);
        raw[1] = u8::MAX;
        let source_interface = iface(0x04);
        let arrived_at = InstantMillis(13);
        let packet = InboundPacket {
            arrived_at,
            source_interface,
            bytes: &mut raw,
        };

        let classified = Ingress::classify(packet);
        let Ingress::Announce {
            received_hops,
            source_interface: classified_source,
            arrived_at: classified_arrival,
            ..
        } = classified
        else {
            panic!("valid announce should classify as announce");
        };
        assert_eq!(received_hops, u8::MAX);
        assert_eq!(classified_source, source_interface);
        assert_eq!(classified_arrival, arrived_at);
    }

    use crate::engine::test_support::*;

    use crate::engine::{
        AnnounceIngest, EngineState, IngestPacketOutcome, RatchetEntropy, RatchetPolicy,
        ReannounceSchedule,
    };
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::IdentitySigner;
    use crate::routing::announce::derive_destination_hash;
    use crate::routing::delivery::{Delivery, PlainDelivery, SingleDelivery};
    use crate::routing::storage::FixedInline;
    use crate::routing::upstream_app_destinations::ProofStrategy;

    #[test]
    fn ingest_counts_each_packet_without_a_clock() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();

        let mut first_bytes = [1, 2, 3];
        let first = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(10),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut first_bytes,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let mut second_bytes = [4];
        let second = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(20),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut second_bytes,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        assert_eq!(first, IngestPacketOutcome::Ignored);
        assert_eq!(second, IngestPacketOutcome::Ignored);
        assert_eq!(state.ingested_packet_count(), 2);
    }
    #[test]
    fn a_single_sealed_for_the_announced_destination_is_delivered() {
        let mut state = personal_node_announcer();
        let destination = state.self_announced_destinations()[0];
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"hello-announced");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-announced",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );
    }

    #[test]
    fn a_single_sealed_to_the_announced_ratchet_is_delivered() {
        let mut state = ratcheted_personal_node_announcer();
        let destination = state.self_announced_destinations()[0];
        let mut raw = hx(RAW_SEALED_TO_RATCHET);

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"ratchet-parity",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );
    }

    #[test]
    fn an_earlier_announced_ratchet_still_opens_after_rotation() {
        let mut state = ratcheted_personal_node_announcer();
        let interval = ReannounceSchedule::default().interval_millis();
        let mut buf = [0u8; MTU];
        let _ = state
            .write_due_self_announce(
                InstantMillis(1_000 + interval),
                TEST_NONCE,
                RatchetEntropy::new([0x77; RatchetEntropy::LEN]),
                &mut buf,
            )
            .written_len();

        let destination = state.self_announced_destinations()[0];
        let mut raw = hx(RAW_SEALED_TO_RATCHET);
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"ratchet-parity",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );
    }

    #[test]
    fn a_ratcheted_destination_still_opens_identity_keyed_traffic() {
        let mut state = ratcheted_personal_node_announcer();
        let destination = state.self_announced_destinations()[0];
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"identity-keyed");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"identity-keyed",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );
    }

    const RAW_PLAIN_DATA: &str = "080012f815e3e65add6ceb2fda0e7be338680068656c6c6f2d706c61696e";

    #[test]
    fn neighbor_plain_data_for_a_registered_destination_delivers_the_rns_1_3_1_payload() {
        let mut raw = hx(RAW_PLAIN_DATA);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let destination = state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Plain(PlainDelivery {
                    destination,
                    context: WireContext::None,
                    payload: b"hello-plain",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );
    }

    #[test]
    fn relayed_plain_data_is_dropped_at_the_packet_filter() {
        let mut raw = hx(RAW_PLAIN_DATA);
        raw[1] = 1;
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn plain_data_for_an_unregistered_destination_is_not_delivered() {
        let mut raw = hx(RAW_PLAIN_DATA);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state
            .register_plain_destination("personal", &["other"])
            .unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn plain_addressed_data_never_reaches_a_single_destination_with_that_hash() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = state.held_identity_hashes()[0];
        let single = state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination: single,
            context: WireContext::None,
        };
        let mut raw = [0u8; MTU];
        let header_len = header.write(&mut raw).unwrap();
        raw[header_len] = 0xFF;

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw[..header_len + 1]),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn in_transport_data_delivers_only_when_we_are_the_named_transport_instance() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        let mut raw_for_us = hx(&format!(
            "4800{}{}00{}",
            "4cd0cc45a7405dbd5cf9b5be1ef92f10", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));
        let mut raw_for_other = hx(&format!(
            "4800{}{}00{}",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));

        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Plain(delivered),
            ..
        } = state.ingest_packet(
            plain_data_packet(&mut raw_for_us),
            TEST_ENTROPY,
            &transporting_view(),
        )
        else {
            panic!("in-transport data named to us must deliver plainly");
        };
        assert_eq!(delivered.payload, &[0xEE]);

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw_for_other),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn an_identity_less_relay_never_accepts_in_transport_data() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        let mut raw = hx(&format!(
            "4800{}{}00{}",
            "4cd0cc45a7405dbd5cf9b5be1ef92f10", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn single_data_decrypts_in_place_and_delivers_the_plaintext() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let mut raw = sealed_single_packet(&identity, destination, b"hello-single");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-single",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );
    }

    #[test]
    fn a_replayed_single_packet_is_ignored_by_the_dedup_history() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let raw = sealed_single_packet(&identity, destination, b"hello-single");

        let mut first_copy = raw.clone();
        assert!(matches!(
            state.ingest_packet(
                plain_data_packet(&mut first_copy),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(_),
                ..
            },
        ));

        let mut replayed_copy = raw.clone();
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut replayed_copy),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_tampered_single_token_is_ignored_without_poisoning_the_real_packet() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let raw = sealed_single_packet(&identity, destination, b"hello-single");

        let mut tampered = raw.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut tampered),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );

        let mut genuine = raw.clone();
        assert!(matches!(
            state.ingest_packet(
                plain_data_packet(&mut genuine),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(_),
                ..
            },
        ));
    }

    #[test]
    fn each_single_destination_decrypts_only_under_its_own_held_identity() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity_a = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let identity_b = InMemoryNodeIdentity::from_secret_key_bytes(&second_secret_key());
        let held_a = state.hold_identity(fixed_secret_key()).unwrap();
        let held_b = state.hold_identity(second_secret_key()).unwrap();
        assert_eq!(held_a, identity_a.identity_hash());
        assert_eq!(held_b, identity_b.identity_hash());

        let dest_a = state
            .register_single_destination(
                &held_a,
                "personal",
                &["a"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let dest_b = state
            .register_single_destination(
                &held_b,
                "personal",
                &["b"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let mut to_a = sealed_single_packet(&identity_a, dest_a, b"for-a");
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut to_a),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: dest_a,
                    context: WireContext::None,
                    plaintext: b"for-a",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );

        let mut to_b = sealed_single_packet(&identity_b, dest_b, b"for-b");
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut to_b),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: dest_b,
                    context: WireContext::None,
                    plaintext: b"for-b",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );

        let mut crossed = sealed_single_packet(&identity_b, dest_a, b"crossed");
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut crossed),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_held_app_identity_does_not_answer_transport_addressed_data() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let raw = sealed_single_packet_routed(
            &identity,
            Some(TransportId::new(*held.as_bytes())),
            destination,
            b"hello-single",
        );

        let mut as_app_only = raw.clone();
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut as_app_only),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );

        state.set_transport_identity(&held).unwrap();
        let mut as_transport = raw.clone();
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut as_transport),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-single",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: None,
            },
        );
    }

    #[test]
    fn a_prove_all_delivery_carries_the_owed_proof() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let mut raw = sealed_single_packet(&identity, destination, b"prove-me");
        let packet_hash = PacketHash::of_wire_packet(&raw).unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"prove-me",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 16]),
                }),
                maybe_owed_proof: Some(ProofOwed {
                    packet_hash,
                    identity: held,
                }),
            },
        );
    }

    #[test]
    fn single_data_for_an_unregistered_destination_is_ignored() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let registered = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["other"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let unregistered = derive_destination_hash(
            &identity.identity_hash(),
            &crate::routing::announce::expand_name("personal", &["node"]).unwrap(),
        );
        assert_ne!(registered, unregistered);
        let mut raw = sealed_single_packet(&identity, unregistered, b"hello-single");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn an_echo_of_our_own_announce_takes_no_route() {
        let mut state = personal_node_announcer();
        let mut announce_buf = [0u8; MTU];
        let announce_len = state
            .write_due_self_announce(
                InstantMillis(100),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();

        let mut relayed = announce_buf[..announce_len].to_vec();
        relayed[1] = 1;
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0xA1; 16]),
                    bytes: &mut relayed,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored),
            "a transport echoing our announce back must not become a route to ourselves",
        );
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn a_node_without_transport_interfaces_learns_the_route_but_owes_no_rebroadcast() {
        use crate::interfaces::{EgressCapability, TransportCapability};

        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let mut leaf = routable_descriptor(InterfaceId::new([0xEE; 16]));
        leaf.capabilities.egress = EgressCapability::Enabled(TransportCapability::NoTransport);

        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &[leaf],
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap(),
                ),
                hops: 1,
                rebroadcast: RebroadcastDecision::NoTransportInterfaces,
            })),
        );
        assert_eq!(state.route_count(), 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);
    }

    #[test]
    fn ingest_accepts_a_real_announce_then_rejects_its_replay() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();

        let first = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(first, raw_announce_accepted(1));
        assert_eq!(state.route_count(), 1);

        let second = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            second,
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored)
        );
        assert_eq!(state.route_count(), 1);
    }

    #[test]
    fn received_hops_are_incremented_so_the_reach_boundary_matches_pathfinder_m() {
        let mut at_limit = hx(RAW_ANNOUNCE);
        at_limit[1] = 127;
        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut at_limit,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(128));

        let mut beyond = hx(RAW_ANNOUNCE);
        beyond[1] = 128;
        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut beyond,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, IngestPacketOutcome::Announce(AnnounceIngest::Ignored));
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn an_accepted_announce_is_retained_for_faithful_rebroadcast() {
        let mut raw = hx(RAW_ANNOUNCE);
        let pristine = raw.clone();
        let (header, payload) = WirePacketHeader::parse(&pristine).unwrap();
        let destination =
            DestinationHash::from_slice(&pristine[2..18]).expect("16-byte destination hash");

        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(1));

        let retained = state
            .routing_table
            .retained_announce_for(&destination)
            .expect("the accepted announce is on hand");
        assert_eq!(retained.hops, header.hops + 1);
        let mut buf = [0u8; 500];
        let n = retained.announce.to_wire(&mut buf).unwrap();
        assert_eq!(&buf[..n], payload);
    }

    #[test]
    fn a_node_without_a_transport_id_learns_the_route_but_owes_no_rebroadcast() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();

        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap(),
                ),
                hops: 1,
                rebroadcast: RebroadcastDecision::NotATransportNode,
            })),
        );
        assert_eq!(state.route_count(), 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);
    }

    #[test]
    fn a_relayed_announce_routes_via_its_transport_node_and_a_direct_one_routes_direct() {
        use crate::routing::NextHop;
        use crate::wire::PropagationType;

        let raw = hx(RAW_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let destination = header.destination;
        let relay = TransportId::new([0xBB; 16]);

        let relayed_header = WirePacketHeader {
            transport_id: Some(relay),
            propagation: PropagationType::Transport,
            hops: 1,
            ..header
        };
        let mut relayed = [0u8; MTU];
        let header_len = relayed_header.write(&mut relayed).unwrap();
        relayed[header_len..header_len + payload.len()].copy_from_slice(payload);

        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut relayed[..header_len + payload.len()],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(2));
        assert_eq!(
            state
                .routing_table
                .retained_announce_for(&destination)
                .unwrap()
                .next_hop,
            NextHop::Via(relay),
            "a relayed announce's next hop is the transport node that stamped it",
        );

        let mut direct = raw.clone();
        let mut fresh = transporting_node();
        let _ = fresh.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut direct,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            fresh
                .routing_table
                .retained_announce_for(&destination)
                .unwrap()
                .next_hop,
            NextHop::Direct,
            "an unrelayed announce is reachable directly",
        );
    }

    #[test]
    fn ingest_processes_but_does_not_accept_non_announce_bytes() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let junk = InboundPacket {
            arrived_at: InstantMillis(1),
            source_interface: InterfaceId::new([0u8; 16]),
            bytes: &mut [0x00, 0x00, 0x01, 0x02, 0x03],
        };
        let out = state.ingest_packet(junk, TEST_ENTROPY, &transporting_view());
        assert_eq!(out, IngestPacketOutcome::Ignored);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn arena_full_drops_park_the_inbound_bytes_for_retry() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state =
            EngineState::<FixedInline<4, 64, 8, 4, 512, 64, 8, 8, 8, 128, 8, 8>>::default();

        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::HeldForRetry)
        );
        assert_eq!(state.route_count(), 0);
        assert_eq!(state.held_announce_count(), 1);
    }
}

use crate::engine::proof::{ProofIngest, ProofOwed};
use crate::engine::EngineState;
use crate::routing::announce::defaults::{
    jitter_offset_for, JitterSeed, DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
};
use crate::routing::announce::held_cache::HeldAnnounces;
use crate::routing::announce::schedule::RebroadcastQueue;
use crate::routing::announce::{AnnounceAcceptanceDecision, AnnounceAcceptanceInput};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::delivery::{
    Delivery, PlainDelivery, SingleDelivery, PLAIN_DATA_MAX_RECEIVED_HOPS,
};
use crate::routing::storage::EngineStorage;
use crate::routing::upstream_app_destinations::{ProofStrategy, UpstreamAppDestinationKind};
use crate::routing::{DropCause, UpsertRouteOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceIngest {
    Accepted(AcceptedAnnounce),
    HeldForRetry,
    Ignored,
}

/// The route an accepted announce just took — what an app needs to discover
/// the peer behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedAnnounce {
    pub destination: DestinationHash,
    pub hops: u8,
    pub rebroadcast: RebroadcastDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebroadcastDecision {
    Scheduled,
    NotATransportNode,
    NoTransportInterfaces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestPacketOutcome<'p> {
    Announce(AnnounceIngest),
    Delivery {
        delivery: Delivery<'p>,
        maybe_owed_proof: Option<ProofOwed>,
    },
    Proof(ProofIngest),
    Ignored,
}

impl<S: EngineStorage> EngineState<S> {
    #[must_use]
    pub fn ingest_packet<'p>(
        &mut self,
        packet: InboundPacket<'p>,
        jitter: JitterSeed,
        interfaces: &[InterfaceDescriptor],
    ) -> IngestPacketOutcome<'p> {
        self.ingested_packet_count = self.ingested_packet_count.saturating_add(1);

        match Ingress::classify(packet) {
            Ingress::Announce {
                announce,
                received_hops,
                source_interface,
                arrived_at,
                next_hop,
            } => IngestPacketOutcome::Announce(self.ingest_announce(
                announce,
                received_hops,
                source_interface,
                arrived_at,
                next_hop,
                jitter,
                interfaces,
            )),

            Ingress::Data {
                data,
                received_hops,
                source_interface,
                arrived_at,
            } => match self.maybe_upstream_delivery(
                data,
                received_hops,
                source_interface,
                arrived_at,
            ) {
                Some((delivery, maybe_owed_proof)) => IngestPacketOutcome::Delivery {
                    delivery,
                    maybe_owed_proof,
                },
                None => IngestPacketOutcome::Ignored,
            },

            Ingress::Proof {
                payload,
                arrived_at,
            } => IngestPacketOutcome::Proof(self.ingest_proof(payload, arrived_at)),

            Ingress::LinkRequest => IngestPacketOutcome::Ignored,
            Ingress::Unparseable => IngestPacketOutcome::Ignored,
        }
    }

    fn maybe_upstream_delivery<'p>(
        &mut self,
        data: DataPacket<'p>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> Option<(Delivery<'p>, Option<ProofOwed>)> {
        if let Some(transport_id) = data.maybe_transport_id {
            if self.transport_id != Some(transport_id) {
                return None;
            }
        }

        match data.destination_type {
            DestinationType::Plain => {
                if received_hops > PLAIN_DATA_MAX_RECEIVED_HOPS {
                    return None;
                }
                self.upstream_app_destinations
                    .lookup(&data.destination, DestinationType::Plain)?;
                Some((
                    Delivery::Plain(PlainDelivery {
                        destination: data.destination,
                        context: data.context,
                        payload: data.payload,
                        arrived_at,
                        source_interface,
                    }),
                    None,
                ))
            }
            DestinationType::Single => {
                let registered = self
                    .upstream_app_destinations
                    .lookup(&data.destination, DestinationType::Single)?;
                let UpstreamAppDestinationKind::Single {
                    identity,
                    proof_strategy,
                } = registered.kind
                else {
                    return None;
                };
                let held = self.held_identities.get(&identity)?;

                let packet_hash = PacketHash::of_data_fields(
                    DestinationType::Single,
                    &data.destination,
                    data.context,
                    data.payload,
                );
                match self.packet_hash_history.remember(packet_hash) {
                    RememberPacketOutcome::AlreadyKnown => return None,
                    RememberPacketOutcome::StoredFresh
                    | RememberPacketOutcome::StoredAfterRotation => {}
                }

                let ratchet_secrets = self.self_ratchets.secrets_newest_first(&data.destination);
                let plaintext = held
                    .decrypt_in_place_with_ratchets(ratchet_secrets, data.payload)
                    .ok()?;
                let maybe_owed_proof = match proof_strategy {
                    ProofStrategy::ProveAll => Some(ProofOwed {
                        packet_hash,
                        identity,
                    }),
                    ProofStrategy::ProveNone => None,
                };
                Some((
                    Delivery::Single(SingleDelivery {
                        destination: data.destination,
                        context: data.context,
                        plaintext,
                        arrived_at,
                        source_interface,
                    }),
                    maybe_owed_proof,
                ))
            }
            DestinationType::Group | DestinationType::Link => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ingest_announce(
        &mut self,
        announce: Announce<'_>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        next_hop: NextHop,
        jitter: JitterSeed,
        interfaces: &[InterfaceDescriptor],
    ) -> AnnounceIngest {
        let decision = AnnounceAcceptanceInput {
            packet_hops: received_hops,
            announce_id: announce.announce_id,
            destination_is_self_or_upstream: self
                .upstream_app_destinations
                .lookup(&announce.destination, DestinationType::Single)
                .is_some(),
            existing_route: self.routing_table.existing_route_for(&announce.destination),
            arrived_at,
        }
        .determine_acceptance();

        if !matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
            return AnnounceIngest::Ignored;
        }

        let outcome = self.routing_table.upsert_route(
            received_hops,
            arrived_at,
            source_interface,
            next_hop,
            &announce,
        );
        match outcome {
            UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated => {
                let rebroadcast = if self.transport_id.is_none() {
                    RebroadcastDecision::NotATransportNode
                } else if interfaces
                    .iter()
                    .any(|descriptor| descriptor.capabilities.allows_transport())
                {
                    let offset = jitter_offset_for(
                        jitter,
                        &announce.destination,
                        DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                    );
                    self.pending_rebroadcasts.schedule(
                        announce.destination,
                        InstantMillis(arrived_at.0.saturating_add(offset)),
                        source_interface,
                    );
                    RebroadcastDecision::Scheduled
                } else {
                    RebroadcastDecision::NoTransportInterfaces
                };
                AnnounceIngest::Accepted(AcceptedAnnounce {
                    destination: announce.destination,
                    hops: received_hops,
                    rebroadcast,
                })
            }
            UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull) => {
                use crate::routing::announce::held_cache::{HoldReason, ParkOutcome};
                match self.held_announces_cache.park(
                    &announce,
                    arrived_at,
                    received_hops,
                    HoldReason::RoutingArenaPressure,
                    source_interface,
                    next_hop,
                ) {
                    ParkOutcome::Parked | ParkOutcome::Overwrote => AnnounceIngest::HeldForRetry,
                    ParkOutcome::CacheFull | ParkOutcome::AppDataTooLarge => {
                        AnnounceIngest::Ignored
                    }
                }
            }
            UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull) => AnnounceIngest::Ignored,
        }
    }
}
