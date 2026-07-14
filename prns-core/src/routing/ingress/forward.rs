use super::*;
use crate::routing::warmth::WarmestOf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketToForward<'p> {
    pub header: WirePacketHeader,
    pub payload: &'p [u8],
    pub fire_on: InterfaceId,
}

pub(super) struct ForwardingArrival<'i> {
    pub source_interface: InterfaceId,
    pub arrived_at: InstantMillis,
    pub interfaces: AttachedInterfaces<'i>,
}

impl PacketToForward<'_> {
    pub fn to_wire(&self, buf: &mut [u8]) -> Result<usize, WireError> {
        let header_len = self.header.write(buf)?;
        let total_len = header_len + self.payload.len();
        if buf.len() < total_len {
            return Err(WireError::BufferTooShort);
        }
        buf[header_len..total_len].copy_from_slice(self.payload);
        Ok(total_len)
    }
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn routes_via_local_client(&self, destination: &DestinationHash) -> bool {
        self.routing_table
            .forwarding_route_for(destination)
            .is_some_and(|route| {
                route.receiving_interface.kind() == Some(InterfaceKind::LocalClient)
            })
    }

    pub(super) fn maybe_forward<'p>(
        &mut self,
        header: WirePacketHeader,
        payload: &'p mut [u8],
        packet_hash: PacketHash,
        received_hops: u8,
        arrival: ForwardingArrival<'_>,
    ) -> Option<PacketToForward<'p>> {
        let ForwardingArrival {
            source_interface,
            arrived_at,
            interfaces,
        } = arrival;
        if header.destination_type != DestinationType::Single
            || header.packet_type != PacketType::Data
        {
            return None;
        }

        let route = self
            .routing_table
            .forwarding_route_for(&DestinationHash::from_address(header.address))?;

        let remaining_hops = route.hops.0;
        let forwarded_header = if remaining_hops > 1 {
            let NextHop::Via(next) = route.next_hop else {
                return None;
            };
            WirePacketHeader {
                hops: received_hops,
                transport_id: Some(next),
                ..header
            }
        } else {
            WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type: header.destination_type,
                packet_type: header.packet_type,
                hops: received_hops,
                transport_id: None,
                address: header.address,
                context: header.context,
            }
        };

        self.reverse_routes.remember(
            ReverseRouteEntry {
                proof_destination: packet_hash.proof_destination(),
                received_interface: source_interface,
                outbound_interface: route.receiving_interface,
                expires_at: InstantMillis(
                    arrived_at
                        .0
                        .saturating_add(DEFAULT_REVERSE_ROUTE_TIMEOUT_MS),
                ),
            },
            arrived_at,
        );

        let warmth = WarmestOf(&self.tunnels, &self.departed_interfaces);
        self.routing_table.note_relayed_with_warmth(
            &DestinationHash::from_address(header.address),
            arrived_at,
            interfaces,
            &warmth,
        );

        Some(PacketToForward {
            header: forwarded_header,
            payload,
            fire_on: route.receiving_interface,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::routing::ingress::testkit::iface;

    #[test]
    fn a_final_hop_forward_strips_the_transport_header_back_to_the_direct_wire() {
        let mut relay = transporting_node();
        let mut announce = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let mut in_transport = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut in_transport,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("a transport-addressed packet with a one-hop route forwards, got {out:?}");
        };
        assert_eq!(forward.fire_on, InterfaceId::new([0xB2; 8]));
        let mut wire = [0u8; BROADCAST_MTU];
        let n = forward.to_wire(&mut wire).unwrap();
        let mut expected = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET);
        expected[1] = 1;
        assert_eq!(
            &wire[..n],
            expected.as_slice(),
            "the final hop strips transport framing: the destination hears the direct wire, one hop further",
        );

        let mut replay = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let again = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut replay,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(
            again,
            IngestPacketOutcome::Ignored(IgnoreReason::Duplicate),
            "a relay forwards each packet exactly once",
        );
    }

    #[test]
    fn relaying_a_packet_slides_the_carried_routes_expiry_forward() {
        let route_view = [routable_descriptor(InterfaceId::new([0xB2; 8]))];
        let mut relay = transporting_node();
        let mut announce = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        let learned_expiry = relay
            .routing_table
            .soonest_route_expiry(AttachedInterfaces::new(&route_view))
            .expect("the announce taught exactly one route");

        let mut in_transport = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(120_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut in_transport,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert!(
            matches!(out, IngestPacketOutcome::Forward(_)),
            "the transport-addressed packet forwards across the held route, got {out:?}",
        );

        let relayed_expiry = relay
            .routing_table
            .soonest_route_expiry(AttachedInterfaces::new(&route_view))
            .expect("the carried route survives the relay");
        assert_eq!(
            relayed_expiry.0,
            learned_expiry.0 + (120_000 - 500),
            "relaying slid the carried route's expiry forward by the gap since its announce, so it cannot age out mid-flow",
        );
    }

    #[test]
    fn a_mid_path_forward_swaps_the_transport_id_to_the_next_relay() {
        use crate::wire::PropagationType;

        let next_relay = TransportId::new([0xBB; 16]);
        let mut relay = transporting_node();

        let raw = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let relayed_header = WirePacketHeader {
            transport_id: Some(next_relay),
            propagation: PropagationType::Transport,
            hops: 1,
            ..header
        };
        let mut relayed = [0u8; BROADCAST_MTU];
        let header_len = relayed_header.write(&mut relayed).unwrap();
        relayed[header_len..header_len + payload.len()].copy_from_slice(payload);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut relayed[..header_len + payload.len()],
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let mut in_transport = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut in_transport,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("a transport-addressed packet with a multi-hop route forwards, got {out:?}");
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let n = forward.to_wire(&mut wire).unwrap();
        let mut expected = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET_VIA_TRANSPORT);
        expected[1] = 1;
        expected[2..18].copy_from_slice(next_relay.as_bytes());
        assert_eq!(
            &wire[..n],
            expected.as_slice(),
            "mid-path the only bytes that change are the hop count and the next relay's id",
        );
    }

    #[test]
    fn a_local_clients_direct_data_is_carried_out_to_its_route() {
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
        let mut relay = transporting_node();
        let mut announce = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: iface(0xB2),
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let mut direct = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET);
        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: app,
                bytes: &mut direct,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("an app sharing our instance has its direct data carried out, got {out:?}");
        };
        assert_eq!(
            forward.fire_on,
            iface(0xB2),
            "the local client's packet rides the route it could not reach itself",
        );
    }

    #[test]
    fn a_strangers_direct_data_to_a_routed_destination_is_still_dropped() {
        let mut relay = transporting_node();
        let mut announce = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: iface(0xB2),
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let mut direct = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET);
        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: iface(0xA1),
                bytes: &mut direct,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
            "carrying a stranger's direct data would make us an open relay; only the named \
             transport instance or a local-client app is carried",
        );
    }

    #[test]
    fn a_packet_for_a_destination_on_a_local_client_is_carried_inward() {
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"nomadnet");
        let mut relay = transporting_node();
        let mut announce = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: app,
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let mut in_transport = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: iface(0xA1),
                bytes: &mut in_transport,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("a packet for an app on our instance is carried inward to it, got {out:?}");
        };
        assert_eq!(
            forward.fire_on, app,
            "the destination announced at zero hops is carried in over its own interface",
        );
    }

    #[test]
    fn a_proof_rides_the_reverse_route_home_exactly_once() {
        let mut relay = transporting_node();
        let mut announce = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        let mut in_transport = bytes_from_hex(RNS_1_3_5_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut in_transport,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("the data leg must forward first");
        };
        let proof_destination = PacketHash::of_data_fields(
            forward.header.destination_type,
            &forward.header.address,
            forward.header.context,
            forward.payload,
        )
        .proof_destination();

        let proof_header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: crate::wire::PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Proof,
            hops: 0,
            transport_id: None,
            address: proof_destination.to_address(),
            context: WireContext::None,
        };
        let mut proof_wire = [0u8; BROADCAST_MTU];
        let header_len = proof_header.write(&mut proof_wire).unwrap();
        proof_wire[header_len..header_len + 64].fill(0xAB);
        let proof_len = header_len + 64;

        let mut wrong_lane = proof_wire;
        let mut right_lane = proof_wire;

        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut right_lane[..proof_len],
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        let IngestPacketOutcome::Forward(returned) = out else {
            panic!("the proof must ride the reverse route, got {out:?}");
        };
        assert_eq!(
            returned.fire_on,
            InterfaceId::new([0xA1; 8]),
            "the proof leaves on the interface the data packet arrived from",
        );
        let mut wire = [0u8; BROADCAST_MTU];
        let n = returned.to_wire(&mut wire).unwrap();
        let mut expected = std::vec::Vec::new();
        expected.extend_from_slice(&proof_wire[..proof_len]);
        expected[1] = 1;
        assert_eq!(&wire[..n], expected.as_slice());

        let out = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(3_000),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut wrong_lane[..proof_len],
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(
            out,
            IngestPacketOutcome::Proof(crate::engine::ProofIngest::Ignored),
            "reverse rows pop on use: the second copy finds no path home",
        );
    }
}
